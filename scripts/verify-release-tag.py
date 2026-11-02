#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
verify-release-tag.py — 校验 Release Tag 与 manifest 映射规则

规则映射：
  v2.1.0-rc.2 -> manifest 2.1.0-2 -> prerelease=true
  v2.1.0      -> manifest 2.1.0   -> prerelease=false
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def parse_tag(tag: str) -> tuple[str, bool]:
    raw = tag.strip()
    if raw.startswith("v") or raw.startswith("V"):
        raw = raw[1:]

    # 匹配 RC 标签: 2.1.0-rc.2 -> manifest: 2.1.0-2, prerelease: True
    m_rc = re.match(r"^(\d+\.\d+\.\d+)-rc\.(\d+)$", raw)
    if m_rc:
        base, rc = m_rc.groups()
        return f"{base}-{rc}", True

    # 匹配正式标签: 2.1.0 -> manifest: 2.1.0, prerelease: False
    m_release = re.match(r"^(\d+\.\d+\.\d+)$", raw)
    if m_release:
        return m_release.group(1), False

    # 兼容存量构建标签: 2.1.0-7 -> manifest: 2.1.0-7, prerelease: True
    m_legacy = re.match(r"^(\d+\.\d+\.\d+-\d+)$", raw)
    if m_legacy:
        return m_legacy.group(1), True

    raise ValueError(f"无法识别的 Tag 格式: '{tag}'。期望格式如 'v2.1.0' 或 'v2.1.0-rc.2'")


def read_manifest_version(manifest_path: Path) -> str:
    for line in manifest_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line.startswith("version"):
            return line.split("=", 1)[1].strip()
    raise RuntimeError("manifest 中未找到 version 字段")


def main():
    if len(sys.argv) < 2:
        print("用法: python3 scripts/verify-release-tag.py <tag> [--github-output]")
        return 2

    tag = sys.argv[1]
    github_output_mode = "--github-output" in sys.argv

    try:
        expected_manifest, is_prerelease = parse_tag(tag)
    except ValueError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 1

    manifest_file = ROOT / "manifest"
    if not manifest_file.exists():
        print(f"ERROR: 找不到 manifest 文件: {manifest_file}", file=sys.stderr)
        return 1

    actual_manifest = read_manifest_version(manifest_file)
    print(f"Tag: {tag}")
    print(f"期望 manifest 版本: {expected_manifest}")
    print(f"实际 manifest 版本: {actual_manifest}")
    print(f"Prerelease 状态: {is_prerelease}")

    if actual_manifest != expected_manifest:
        print(
            f"ERROR: manifest 版本漂移！tag '{tag}' 对应 manifest 必须为 '{expected_manifest}'，实际为 '{actual_manifest}'",
            file=sys.stderr,
        )
        return 1

    if github_output_mode:
        import os
        gh_output = os.environ.get("GITHUB_OUTPUT")
        if gh_output:
            with open(gh_output, "a", encoding="utf-8") as f:
                f.write(f"manifest_version={actual_manifest}\n")
                f.write(f"prerelease={str(is_prerelease).lower()}\n")
                f.write(f"tag_name={tag}\n")
            print("已成功写入 GITHUB_OUTPUT")

    print("PASS: Tag 与 manifest 映射严格匹配通过。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
