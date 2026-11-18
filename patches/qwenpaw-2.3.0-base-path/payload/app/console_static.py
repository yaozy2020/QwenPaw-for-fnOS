# -*- coding: utf-8 -*-
"""Serve the Console's prebuilt representations without runtime compression."""

from __future__ import annotations

import math
import mimetypes
import os
import re
import stat

import anyio
from fastapi.responses import Response
from starlette.datastructures import Headers
from starlette.exceptions import HTTPException
from starlette.types import Scope

from ..hub.static_files import CompressedStaticFiles

_HASHED_ASSET = re.compile(r".+-[A-Za-z0-9_-]{8}\.[A-Za-z0-9]+$")
_INDEX_CACHE_CONTROL = "no-cache, no-store, must-revalidate"


def _encoding_preferences(scope: Scope) -> list[tuple[str | None, str]]:
    raw = ",".join(Headers(scope=scope).getlist("accept-encoding"))
    qualities: dict[str, float] = {}
    for entry in raw.split(","):
        parts = entry.split(";")
        name = parts[0].strip().lower()
        if not name:
            continue
        quality = 1.0
        for parameter in parts[1:]:
            key, separator, value = parameter.strip().partition("=")
            if key.strip().lower() == "q":
                try:
                    parsed = float(value) if separator else 0.0
                except ValueError:
                    parsed = 0.0
                quality = min(
                    quality,
                    parsed if math.isfinite(parsed) and 0 <= parsed <= 1 else 0.0,
                )
        qualities[name] = min(quality, qualities.get(name, 1.0))

    wildcard = qualities.get("*", 0.0)
    ranked = [
        (qualities.get(name, wildcard), index, name, suffix)
        for index, (name, suffix) in enumerate((("br", ".br"), ("gzip", ".gz")))
        if qualities.get(name, wildcard) > 0
    ]
    if "identity" in qualities and qualities["identity"] > 0:
        ranked.append((qualities["identity"], 2, None, ""))
    ranked.sort(key=lambda item: (-item[0], item[1]))
    result = [(name, suffix) for _, _, name, suffix in ranked]
    if "identity" not in qualities and qualities.get("*") != 0:
        result.append((None, ""))
    return result


class ConsoleStaticFiles(CompressedStaticFiles):
    """Keep StaticFiles validation and conditional responses for every encoding."""

    def _representation(self, path: str, preferences):
        _, original = self.lookup_path(path)
        if original is None or not stat.S_ISREG(original.st_mode):
            raise HTTPException(status_code=404)
        for encoding, suffix in preferences:
            full_path, metadata = self.lookup_path(path + suffix)
            if metadata is not None and stat.S_ISREG(metadata.st_mode):
                return full_path, metadata, encoding
        raise HTTPException(status_code=406, headers={"Vary": "Accept-Encoding"})

    async def get_response(self, path: str, scope: Scope) -> Response:
        if scope["method"] not in ("GET", "HEAD"):
            raise HTTPException(status_code=405, headers={"Allow": "GET, HEAD"})
        if path.lower().endswith((".br", ".gz")):
            raise HTTPException(status_code=404)
        try:
            full_path, metadata, encoding = await anyio.to_thread.run_sync(
                self._representation, path, _encoding_preferences(scope),
            )
        except (OSError, ValueError):
            raise HTTPException(status_code=404) from None

        response = self.file_response(full_path, metadata, scope)
        media_type, _ = mimetypes.guess_type(path)
        response.headers["Content-Type"] = media_type or "application/octet-stream"
        if encoding is not None:
            response.headers["Content-Encoding"] = encoding
        response.headers.add_vary_header("Accept-Encoding")
        filename = os.path.basename(path)
        if filename.lower().endswith((".html", ".htm")):
            response.headers["Cache-Control"] = _INDEX_CACHE_CONTROL
            response.headers["Pragma"] = "no-cache"
            response.headers["Expires"] = "0"
        elif _HASHED_ASSET.fullmatch(filename):
            response.headers["Cache-Control"] = "public, max-age=31536000, immutable"
        else:
            response.headers["Cache-Control"] = "public, max-age=300"
        return response
