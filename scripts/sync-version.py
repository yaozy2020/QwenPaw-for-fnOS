#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
sync-version.py — 从 manifest 同步版本号到派生位置（README badge）

设计：manifest 是版本号唯一真相源（SSOT）。
"""
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def read_manifest_version() -> str:
    for line in (ROOT / "manifest").read_text(encoding="utf-8").splitlines():
        if line.strip().startswith("version"):
            return line.split("=", 1)[1].strip()
    raise RuntimeError("manifest 中未找到 version")


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
    # tag 用于 rc 序号下界判定:发布时由 RELEASE_TAG / GITHUB_REF 注入
    tag = os.environ.get("RELEASE_TAG", "")
    if not tag:
        ref = os.environ.get("GITHUB_REF", "")
        if ref.startswith("refs/tags/"):
            tag = ref[len("refs/tags/"):]
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
        # manifest 可能是 x.y.z、x.y.z-rc.N 或 x.y.z-N(构建号)。
        # 构建号不写进 crate 版本(Cargo 不接受该形式),但主干 x.y.z 必须一致,
        # 不能像以前那样在 x.y.z-N 形态下直接 want=cur —— 那等于放弃校验。
        base = version.split("-")[0]
        m_cur = re.match(r"^(\d+\.\d+\.\d+)(?:-(.+))?$", cur)
        if not m_cur:
            ok = False
            want = f"{base}[-rc.N]"
        else:
            cur_base, cur_suffix = m_cur.group(1), m_cur.group(2)
            # ① 主干必须一致
            ok = cur_base == base
            want = base
            if not ok:
                want = f"{base}[-rc.N]"
            elif cur_suffix:
                # ② 允许 rc.N 预发布串,但序号不得低于 tag 的 rc 序号
                m_tag_rc = re.search(r"-rc\.(\d+)$", tag) if tag else None
                m_cur_rc = re.match(r"^rc\.(\d+)$", cur_suffix)
                if m_cur_rc and m_tag_rc:
                    if int(m_cur_rc.group(1)) < int(m_tag_rc.group(1)):
                        ok = False
                        want = f"{base}-rc.{m_tag_rc.group(1)}"
        targets.append(("supervisor/Cargo.toml", cur, ok))

        # Cargo.lock 必须与 Cargo.toml 同步,否则后续所有 --locked 构建
        # (build.sh/ci/release)都会报 "lock file needs to be updated",
        # 失败在远离成因处爆发(审查 构建 P2-1)。
        lock = ROOT / "supervisor" / "Cargo.lock"
        if lock.exists():
            lt = lock.read_text(encoding="utf-8")
            m_lock = re.search(r'name = "qwenpaw-supervisor"\s*\nversion = "([^"]+)"', lt)
            if m_lock:
                lock_cur = m_lock.group(1)
                want_lock = cur if ok else want
                lock_ok = lock_cur == want_lock
                targets.append(("supervisor/Cargo.lock", lock_cur, lock_ok))
                if not check and not lock_ok:
                    lock.write_text(
                        re.sub(
                            r'(name = "qwenpaw-supervisor"\s*\nversion = )"[^"]+"',
                            lambda mm: mm.group(1) + f'"{want_lock}"',
                            lt, count=1,
                        ),
                        encoding="utf-8",
                    )

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
