#!/usr/bin/env python3
"""Fetch the current Linux x86 fnpack from the official fnOS download page."""

import hashlib
import json
import re
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.request import HTTPRedirectHandler, build_opener

PAGE = "https://developer.fnnas.com/docs/cli/fnpack/"
DOWNLOAD = re.compile(r"https://static2\.fnnas\.com/fnpack/fnpack-(\d+\.\d+\.\d+)-linux-amd64")
# Digest measured on the official 2026-11-19 release. Later versions are resolved from the same CDN.
KNOWN_DIGESTS = {"1.3.3": "1464d6aa9b995ebecb263c5ec60f0f4a7cbf45e0ce887836755fd65abd647d1e"}


class DownloadLinks(HTMLParser):
    def __init__(self):
        super().__init__()
        self.releases = {}

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            url = dict(attrs).get("href") or ""
            match = DOWNLOAD.fullmatch(url)
            if match:
                self.releases[match[1]] = url


def latest_release(html):
    links = DownloadLinks()
    links.feed(html)
    if not links.releases:
        raise ValueError("官方 fnpack 页面缺少 Linux amd64 正式版下载链接")
    version = max(links.releases, key=lambda v: tuple(map(int, v.split("."))))
    return version, links.releases[version]


class OfficialSourceOnly(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError(f"官方 fnpack 地址发生重定向，请核对来源: {newurl}")


def resolve(directory, expected=None):
    client = build_opener(OfficialSourceOnly())
    with client.open(PAGE, timeout=30) as response:
        version, url = latest_release(response.read().decode("utf-8"))
    with client.open(url, timeout=60) as response:
        binary = response.read()
    digest = hashlib.sha256(binary).hexdigest()
    record = {"version": version, "url": url, "sha256": digest}
    if version in KNOWN_DIGESTS and digest != KNOWN_DIGESTS[version]:
        raise ValueError(f"官方 fnpack {version} 与该版本已记录的 SHA256 不一致")
    if expected and record != json.loads(expected.read_text()):
        raise ValueError("本次发布期间官方 fnpack 版本或文件已变化，请重新构建")
    tool = directory / "fnpack"
    tool.write_bytes(binary)
    tool.chmod(0o700)
    help_text = subprocess.check_output([tool.resolve(), "--help"], text=True, timeout=10)
    if f"Version {version}" not in help_text.splitlines():
        raise ValueError(f"下载的 fnpack 自报版本与官方页面 {version} 不一致")
    (directory / "fnpack.json").write_text(json.dumps(record, indent=2) + "\n")
    print(f"fnpack {version}  SHA256 {digest}  {url}")


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        sys.exit("Usage: resolve-fnpack.py EMPTY_WORK_DIR [EXPECTED_FNPACK_JSON]")
    try:
        resolve(Path(sys.argv[1]), Path(sys.argv[2]) if len(sys.argv) == 3 else None)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(f"fnpack: {error}")
