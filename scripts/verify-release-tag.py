#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
verify-release-tag.py — 校验 Release Tag 与 manifest 映射规则

规则映射：
  v2.1.0-rc.2 -> manifest 2.1.0-N (N 单调递增) -> prerelease=true
  v2.1.0      -> manifest 2.1.0                 -> prerelease=false

RC 序号不能直接当 FPK 构建号：那会让 rc.2 的构建号小于 rc.1 的 -7，
触发 fnOS 的"版本倒退、拒绝覆盖升级"。构建号由 manifest 独立维护，必须单调递增。
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def parse_tag(tag: str) -> tuple[str, bool]:
    raw = tag.strip()
    if raw.startswith("v") or raw.startswith("V"):
        raw = raw[1:]

    # RC 标签：只判定 prerelease，具体构建号以 manifest 为准
    m_rc = re.match(r"^(\d+\.\d+\.\d+)-rc\.(\d+)$", raw)
    if m_rc:
        return m_rc.group(1), True

    # 匹配正式标签: 2.1.0 -> manifest: 2.1.0, prerelease: False
    m_release = re.match(r"^(\d+\.\d+\.\d+)$", raw)
    if m_release:
        return m_release.group(1), False

    # 兼容存量构建标签: 2.1.0-7 -> manifest: 2.1.0-7, prerelease: True
    m_legacy = re.match(r"^(\d+\.\d+\.\d+-\d+)$", raw)
    if m_legacy:
        return m_legacy.group(1), True

    raise ValueError(f"无法识别的 Tag 格式: '{tag}'。期望格式如 'v2.1.0' 或 'v2.1.0-rc.2'")


def previous_build_number() -> int | None:
    """从已有 git 标签解析出最大的 FPK 构建号,用于保证构建号单调递增。
    本地/CI 无 git 或无比对基线时返回 None(跳过检查)。"""
    try:
        out = subprocess.check_output(
            ["git", "tag", "-l", "v*"],
            cwd=ROOT, encoding="utf-8", stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    numbers = [int(m.group(1)) for m in
               (re.match(r"^v\d+\.\d+\.\d+-(\d+)$", t.strip()) for t in out.split())
               if m]
    return max(numbers) if numbers else None


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

    # manifest 的 release 部分必须与 tag 一致：2.1.0-rc.2 与 2.1.0 都要求 manifest 以 2.1.0 开头
    if actual_manifest != expected_manifest and not actual_manifest.startswith(expected_manifest + "-"):
        print(
            f"ERROR: manifest 版本漂移！tag '{tag}' 要求 manifest 以 '{expected_manifest}' 开头，实际为 '{actual_manifest}'",
            file=sys.stderr,
        )
        return 1

    # FPK 构建号必须单调递增，否则 fnOS 判定为旧版并拒绝覆盖升级
    m_build = re.search(r"-(\d+)$", actual_manifest)
    if m_build:
        build_no = int(m_build.group(1))
        prev = previous_build_number()
        if prev is not None and build_no <= prev:
            print(
                f"ERROR: FPK 构建号倒退！manifest={actual_manifest} (build={build_no}) "
                f"必须大于已有最大构建号 {prev}。fnOS 会把较小版本视为旧版并拒绝升级。",
                file=sys.stderr,
            )
            return 1
        print(f"构建号单调性: 上一个 {prev if prev is not None else '(无)'} → 本次 {build_no}")

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
