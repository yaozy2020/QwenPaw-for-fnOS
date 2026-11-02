#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
sync-version.py — 从 manifest 同步版本号到派生位置（README badge）

设计：manifest 是版本号唯一真相源（SSOT）。
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def read_manifest_version() -> str:
    for line in (ROOT / "manifest").read_text(encoding="utf-8").splitlines():
        if line.strip().startswith("version"):
            return line.split("=", 1)[1].strip()
    raise RuntimeError("manifest 中未找到 version")


def cargo_version_target() -> str:
    """Cargo.toml 用 PEP 440 风格版本号(Cargo 不认 fnOS 的 2.1.0-8 构建后缀),
    这里由 manifest 派生出 'x.y.z' 或 'x.y.z-rc.N' 形式。"""
    return version.split("-")[0] if "-" not in version else version


def sync_readme(version: str) -> bool:
    readme = ROOT / "README.md"
    if not readme.exists():
        return False
    text = readme.read_text(encoding="utf-8")
    new = re.sub(
        r"badge/version-(.+?)-blue",
        f"badge/version-{version}-blue",
        text,
    )
    if new != text:
        readme.write_text(new, encoding="utf-8")
        return True
    return False


def main():
    version = read_manifest_version()
    check = "--check" in sys.argv
    print(f"manifest version = {version}")
    print(f"mode = {'check' if check else 'sync'}")
    print("-" * 60)

    targets = []
    readme = ROOT / "README.md"
    if readme.exists():
        text = readme.read_text(encoding="utf-8")
        m = re.search(r"badge/version-(.+?)-blue", text)
        cur = m.group(1) if m else "(missing)"
        ok = (cur == version)
        targets.append(("README.md", cur, ok))
        if not check and not ok:
            sync_readme(version)

    # Cargo.toml 的 crate 版本与 manifest 对齐(SSOT 缺口曾经不一致)
    cargo = ROOT / "supervisor" / "Cargo.toml"
    if cargo.exists():
        ct = cargo.read_text(encoding="utf-8")
        m = re.search(r'^version = "(.+?)"$', ct, flags=re.M)
        cur = m.group(1) if m else "(missing)"
        # 允许 x.y.z-rc.N 这种带预发布后缀的形式;构建号(N)不进 crate 版本
        want = version if "-" in version and not re.match(r"^\d+\.\d+\.\d+-\d+$", version) else version.split("-")[0]
        if re.match(r"^\d+\.\d+\.\d+-\d+$", version):
            # manifest 形如 2.1.0-8:构建号不回写 crate 版本,沿用当前预发布串
            want = cur if re.match(r"^\d+\.\d+\.\d+(-rc\.\d+)?$", cur) else version.split("-")[0]
        ok = (cur == want)
        targets.append(("supervisor/Cargo.toml", cur, ok))
        if not check and not ok and m:
            cargo.write_text(
                re.sub(r'^version = "(.+?)"$', f'version = "{want}"', ct, count=1, flags=re.M),
                encoding="utf-8",
            )

    for name, cur, ok in targets:
        status = "OK" if ok else "DRIFT"
        print(f"  [ {status:>5} ] {name:<40} {cur}")
    print("-" * 60)

    if check:
        all_ok = all(t[2] for t in targets)
        if all_ok:
            print(f"PASS: 所有版本号已同步至 {version}。")
            return 0
        print(f"FAIL: 存在版本号漂移，请运行 'python3 scripts/sync-version.py' 修复。")
        return 1

    print(f"DONE: 已同步至 {version}。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
