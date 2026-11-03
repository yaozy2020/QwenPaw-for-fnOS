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


def _git(*args: str) -> str | None:
    try:
        return subprocess.check_output(
            ["git", *args], cwd=ROOT, encoding="utf-8", stderr=subprocess.DEVNULL
        )
    except (OSError, subprocess.CalledProcessError):
        return None


def previous_build_number(
    release_base: str, exclude_tag: str = "", seq_re: str = r"(\d+\.\d+\.\d+)-(\d+)"
) -> int | None:
    """返回同一 release 主干(x.y.z)下已发布过的最大 FPK 构建号。

    基线来源是"各 tag 指向提交里的 manifest 版本",而不是 tag 名本身 ——
    tag 用 v2.1.0-rc.2 这类语义名,构建号写在 manifest 里,
    只从 tag 名提取会永远零匹配,让整条检查形同虚设。

    比较必须限定在同 x.y.z 域内:2.2.0-1 相对 2.1.0-8 是合法的构建号重置,
    跨域比较会把它误判为倒退。
    """
    tags = _git("tag", "-l", "v*")
    if tags is None:
        return None
    numbers: list[int] = []
    for tag in (t.strip() for t in tags.split() if t.strip()):
        # 排除正在发布的这个 tag 自己:它已指向当前 manifest,否则必然"自我倒退"
        if tag == exclude_tag:
            continue
        content = _git("show", f"{tag}:manifest")
        if not content:
            continue
        m = re.search(r"^version\s*=\s*(\S+)\s*$", content, flags=re.M)
        if not m:
            continue
        mb = re.fullmatch(seq_re, m.group(1))
        if mb and mb.group(1) == release_base:
            numbers.append(int(mb.group(2)))
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

    # FPK 构建号必须单调递增，否则 fnOS 判定为旧版并拒绝覆盖升级。
    # rc 预发布串同样受序号约束 —— 静默跳过会让"rc 序号倒退"无门禁
    # (审查 构建 P2-5)。rc 序号与构建号是两个独立序列,分域比较。
    m_build = re.search(r"-(\d+)$", actual_manifest)
    m_rc = re.search(r"-rc\.(\d+)$", actual_manifest)
    if m_build:
        build_no = int(m_build.group(1))
        release_base = actual_manifest.rsplit("-", 1)[0]
        prev = previous_build_number(release_base, tag)
        if prev is not None and build_no <= prev:
            print(
                f"ERROR: FPK 构建号倒退！manifest={actual_manifest} (build={build_no}) "
                f"必须大于已有最大构建号 {prev}。fnOS 会把较小版本视为旧版并拒绝升级。",
                file=sys.stderr,
            )
            return 1
        if prev is None:
            print(f"构建号单调性: {release_base} 域内无历史基线, 跳过比较 (本次 {build_no})")
        else:
            print(f"构建号单调性: {release_base} 域内上一个 {prev} → 本次 {build_no}")
    elif m_rc:
        rc_no = int(m_rc.group(1))
        release_base = actual_manifest.rsplit("-rc.", 1)[0]
        prev = previous_build_number(
            release_base, tag, seq_re=r"(\d+\.\d+\.\d+)-rc\.(\d+)"
        )
        if prev is not None and rc_no <= prev:
            print(
                f"ERROR: rc 序号倒退！manifest={actual_manifest} (rc={rc_no}) "
                f"必须大于已有最大 rc 序号 {prev}。",
                file=sys.stderr,
            )
            return 1
        # 该主干若已有构建号形态的正式发布,rc 序号系列视为已结束:
        # rc.N 与构建号 M 是两个序列,放行会让"rc 序号倒退/倒挂"无门禁
        prev_build = previous_build_number(release_base, tag)
        if prev_build is not None:
            print(
                f"ERROR: {release_base} 已存在构建号形态发布(最大 {prev_build}),"
                f"rc 序号系列已结束 —— 请使用构建号形态(如 {release_base}-{prev_build + 1})",
                file=sys.stderr,
            )
            return 1
        if prev is None:
            print(f"构建号单调性: {release_base} 域内无 rc 基线, 跳过比较 (本次 rc={rc_no})")
        else:
            print(f"构建号单调性: {release_base} 域内上一个 rc {prev} → 本次 {rc_no}")
    else:
        # 正式 tag 不携带构建号:fnOS 覆盖升级按版本串比较,2.1.0 相对
        # 2.1.0-9 可能被判倒退 —— 项目约定 manifest 恒带构建号,偏离即拒绝
        print(
            f"ERROR: 正式版 manifest 必须携带构建号(如 {expected_manifest}-N),"
            f"实际为 '{actual_manifest}'。无构建号版本在 fnOS 覆盖升级时可能被判倒退。",
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
