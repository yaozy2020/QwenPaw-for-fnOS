#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
gen-audit.py — 自动生成 AUDIT_REPORT.md

设计：用代码事实替代人写文档，避免过时。
数据源：
  - manifest（版本号、依赖、changelog）
  - cmd/*（生命周期脚本）
  - wizard/*（向导清单）
  - app/qwenpaw/code/src/*（上游源码统计）
  - git log --tags（最近发布）
"""
import os
import re
import subprocess
import sys
from datetime import datetime, timezone, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TZ = timezone(timedelta(hours=8))


def read_manifest() -> dict:
    data = {}
    for line in (ROOT / "manifest").read_text(encoding="utf-8").splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            data[k.strip()] = v.strip()
    return data


def list_cmd_scripts() -> list[str]:
    return sorted(p.name for p in (ROOT / "cmd").glob("*") if p.is_file())


def list_wizard_files() -> list[str]:
    d = ROOT / "wizard"
    if not d.exists():
        return []
    return sorted(p.name for p in d.glob("*") if p.is_file())


def count_upstream_python() -> tuple[int, int]:
    """返回 (py文件数, 总行数)。"""
    src = ROOT / "app" / "qwenpaw" / "code" / "src"
    if not src.exists():
        return 0, 0
    files = list(src.rglob("*.py"))
    lines = 0
    for f in files:
        try:
            lines += sum(1 for _ in f.open("r", encoding="utf-8", errors="ignore"))
        except Exception:
            pass
    return len(files), lines


def count_plugins() -> int:
    p = ROOT / "app" / "qwenpaw" / "code" / "plugins"
    if not p.exists():
        return 0
    return sum(1 for _ in p.rglob("*.py"))


def list_workflows() -> list[str]:
    d = ROOT / ".github" / "workflows"
    if not d.exists():
        return []
    return sorted(p.name for p in d.glob("*.yml"))


def recent_tags(n: int = 5) -> list[tuple[str, str]]:
    """读取最近 n 个 git tag（按版本号降序）。"""
    if os.environ.get("AUDIT_NO_GIT") == "1":
        return []
    try:
        out = subprocess.check_output(
            ["git", "tag", "-l"],
            cwd=ROOT, encoding="utf-8", stderr=subprocess.DEVNULL,
        )
        tags = [t.strip() for t in out.splitlines() if t.strip()]
        # 按版本号降序排序
        def key(t):
            try:
                return [int(x) for x in t.lstrip("v").split(".")]
            except Exception:
                return [0]
        tags.sort(key=key, reverse=True)
        result = []
        for tag in tags[:n]:
            try:
                date_out = subprocess.check_output(
                    ["git", "log", "-1", "--format=%ci", tag],
                    cwd=ROOT, encoding="utf-8", stderr=subprocess.DEVNULL,
                ).strip()
                date = date_out.split(" ", 1)[0] if date_out else ""
            except Exception:
                date = ""
            result.append((tag, date))
        return result
    except Exception:
        return []


def render() -> str:
    m = read_manifest()
    version = m.get("version", "unknown")
    cmd_scripts = list_cmd_scripts()
    wizard_files = list_wizard_files()
    py_files, py_lines = count_upstream_python()
    plugin_files = count_plugins()
    workflows = list_workflows()
    tags = recent_tags()
    now = datetime.now(TZ).strftime("%Y-%m-%d %H:%M:%S %z")

    lines: list[str] = []
    a = lines.append

    a("# QwenPaw FPK — 审计报告")
    a("")
    a("> ⚠️ 本文件由 `scripts/gen-audit.py` 自动生成，请勿手工编辑。")
    a("> 数据源：manifest / cmd / wizard / app / .github / git tags。")
    a("")
    a(f"## 当前版本：v{version}")
    a("")
    a(f"- **生成时间**：{now}")
    a(f"- **应用名**：{m.get('appname', '?')}")
    a(f"- **显示名**：{m.get('display_name', '?')}")
    a(f"- **依赖**：{m.get('install_dep_apps', '?')}")
    a(f"- **服务端口**：{m.get('service_port', '?')}")
    a(f"- **fnOS 最低版本**：{m.get('os_min_version', '?')}")
    a("")
    a("## 工程统计")
    a("")
    a("| 项目 | 数据 |")
    a("|:-----|:-----|")
    a(f"| 生命周期脚本 | {len(cmd_scripts)} 个 |")
    a(f"| 向导清单 | {len(wizard_files)} 个 |")
    a(f"| 上游 Python 文件 | {py_files} 个，{py_lines:,} 行 |")
    a(f"| 上游 Plugins | {plugin_files} 个 .py |")
    a(f"| CI Workflow | {len(workflows)} 个 |")
    a("")
    a("## 生命周期脚本清单")
    a("")
    a("| 脚本 | 说明 |")
    a("|:-----|:-----|")
    desc_map = {
        "config_callback": "配置变更后回调（fnOS 控制台修改配置时触发）",
        "config_init": "配置初始化",
        "install_callback": "安装完成回调",
        "install_init": "安装前环境检查",
        "main": "应用启动 / stop / status 入口",
        "uninstall_callback": "卸载完成回调",
        "uninstall_init": "卸载前清理",
        "upgrade_callback": "升级完成回调",
        "upgrade_init": "升级前环境检查",
    }
    for s in cmd_scripts:
        a(f"| `cmd/{s}` | {desc_map.get(s, '-')} |")
    a("")
    a("## 向导清单")
    a("")
    if wizard_files:
        a("| 文件 | 阶段 |")
        a("|:-----|:-----|")
        for w in wizard_files:
            a(f"| `wizard/{w}` | {w} |")
    else:
        a("（无）")
    a("")
    a("## CI/CD")
    a("")
    if workflows:
        a("| Workflow | 触发 |")
        a("|:---------|:-----|")
        wf_desc = {
            "ci.yml": "PR 触发，跑 preflight + 前端构建验证",
            "release.yml": "tag 触发，fnpackup 容器构建并自动上传 Release",
        }
        for w in workflows:
            a(f"| `{w}` | {wf_desc.get(w, '-')} |")
    else:
        a("（无 CI workflow）")
    a("")
    a("## 最近发布记录")
    a("")
    if tags:
        a("| 版本 | 发布日期 |")
        a("|:-----|:----:|")
        for t, d in tags:
            a(f"| `{t}` | {d or '?'} |")
    else:
        a("（无 git tag）")
    a("")
    a("---")
    a("")
    a("生成命令：`python3 scripts/gen-audit.py`")
    a("")
    return "\n".join(lines)


def main():
    check = "--check" in sys.argv
    out = ROOT / "AUDIT_REPORT.md"
    new = render()
    old = out.read_text(encoding="utf-8") if out.exists() else ""

    def _stable(s: str) -> str:
        return re.sub(r"^- \*\*生成时间\*\*：.*$", "- **生成时间**：__DYNAMIC__", s, flags=re.M)

    if check:
        if _stable(new) != _stable(old):
            print("AUDIT_REPORT.md is OUT OF DATE; run `python3 scripts/gen-audit.py` to regenerate.", file=sys.stderr)
            return 1
        print("AUDIT_REPORT.md content is up-to-date (timestamp ignored).")
        return 0

    if new == old:
        print("AUDIT_REPORT.md is up-to-date.")
        return 0

    out.write_text(new, encoding="utf-8")
    print(f"AUDIT_REPORT.md regenerated ({len(new)} bytes).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
