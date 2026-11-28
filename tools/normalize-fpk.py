#!/usr/bin/env python3
"""Normalize the two tar archives emitted by the pinned fnpack for reproducible FPKs."""

import gzip
import hashlib
import io
import os
import sys
import tarfile
from pathlib import Path


def normalize_tar(data, epoch, replacements=None, sort_entries=False):
    replacements = replacements or {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as source:
        entries = []
        for member in source.getmembers():
            payload = replacements.get(member.name)
            if member.isdir():
                # fnpack creates archive directories under the host's default ACL.
                member.mode = 0o755
            elif payload is not None and member.isfile():
                # fnpack generates manifest and app.tgz as ordinary data files.
                member.mode = 0o644
            if payload is None and member.isfile():
                payload = source.extractfile(member).read()
            if payload is not None:
                member.size = len(payload)
            member.uid = 0
            member.gid = 0
            member.uname = ""
            member.gname = ""
            member.mtime = epoch
            member.pax_headers = {}
            entries.append((member, payload))
    if sort_entries:
        entries.sort(key=lambda entry: entry[0].name)
    output = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as target:
            for member, payload in entries:
                target.addfile(member, io.BytesIO(payload) if payload is not None else None)
    return output.getvalue()


def normalize_fpk(source, destination):
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    outer = source.read_bytes()
    with tarfile.open(fileobj=io.BytesIO(outer), mode="r:gz") as archive:
        app = archive.extractfile("app.tgz").read()
        manifest = archive.extractfile("manifest").read().decode("utf-8")
    normalized_app = normalize_tar(app, epoch, sort_entries=True)
    checksum = hashlib.md5(normalized_app).hexdigest()
    lines = []
    found_checksum = False
    for line in manifest.splitlines():
        key, sep, _ = line.partition("=")
        if sep and key.strip() == "checksum":
            line = f"{key}= {checksum}"
            found_checksum = True
        lines.append(line)
    if not found_checksum:
        lines.append(f"checksum                   = {checksum}")
    normalized_manifest = ("\n".join(lines) + "\n").encode("utf-8")
    destination.write_bytes(normalize_tar(
        outer,
        epoch,
        replacements={"app.tgz": normalized_app, "manifest": normalized_manifest},
    ))


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("Usage: normalize-fpk.py SOURCE.fpk DESTINATION.fpk")
    normalize_fpk(Path(sys.argv[1]), Path(sys.argv[2]))
