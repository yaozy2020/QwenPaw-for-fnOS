#!/bin/bash
# lint.sh — 语法与静态检查门禁（三层）
#   [1/3] bash -n 全量
#   [2/3] shellcheck（error 级为门禁，warning 仅报告；本地无二进制时自动下载 pinned 版本）
#   [3/3] 内嵌 spawn 脚本展开验证 —— api.cgi 中以 nohup bash -c "..." / local cmd="..."
#         形式内嵌的启动脚本是"运行时展开"代码，bash -n 与 shellcheck 都查不出其中的
#         变量误展开（历史事故：awk $2 被 nohup 外层展开成空），必须按真实展开语义
#         生成内层脚本后单独验证，并断言每条启动路径都携带内核启动必需的环境变量。
# 用法: bash scripts/lint.sh
# 注意: 不用 set -e（工程铁律），失败显式 exit 1

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHELLCHECK_VER="0.10.0"
fail=0

echo "========== lint: 语法与静态检查 =========="

# ── [1/3] bash -n 全量 ──
echo "[1/3] bash -n ..."
for f in "$ROOT"/cmd/* "$ROOT"/app/ui/*.cgi "$ROOT"/build.sh "$ROOT"/scripts/*.sh; do
    [ -f "$f" ] || continue
    if bash -n "$f" 2>/dev/null; then
        echo "  [  OK ] ${f#"$ROOT"/}"
    else
        echo "  [FAIL] ${f#"$ROOT"/}"
        bash -n "$f"
        fail=1
    fi
done

# ── [2/3] shellcheck（error 级门禁；warning 仅报告不拦截） ──
echo "[2/3] shellcheck v${SHELLCHECK_VER} ..."
SC_BIN="$(command -v shellcheck 2>/dev/null || true)"
[ -z "$SC_BIN" ] && [ -x /tmp/shellcheck ] && SC_BIN=/tmp/shellcheck
if [ -z "$SC_BIN" ]; then
    TARBALL="/tmp/shellcheck-${SHELLCHECK_VER}.tar.xz"
    if curl -sL --max-time 120 -o "$TARBALL" \
        "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VER}/shellcheck-v${SHELLCHECK_VER}.linux.x86_64.tar.xz" \
        && tar -xJf "$TARBALL" -C /tmp "shellcheck-v${SHELLCHECK_VER}/shellcheck" 2>/dev/null; then
        mv "/tmp/shellcheck-v${SHELLCHECK_VER}/shellcheck" /tmp/shellcheck
        SC_BIN=/tmp/shellcheck
    fi
fi
if [ -n "$SC_BIN" ] && [ -x "$SC_BIN" ]; then
    # 活跃脚本；scripts/update-qwenpaw.sh 为遗留脚本不参与门禁
    TARGETS=(cmd/main cmd/install_callback cmd/upgrade_callback cmd/config_callback cmd/uninstall_callback app/ui/api.cgi app/ui/index.cgi build.sh)
    if "$SC_BIN" -S error "${TARGETS[@]/#/"$ROOT/"}" 2>&1; then
        echo "  [  OK ] error 级问题为 0（warning 级见上方输出，不拦截）"
    else
        echo "  [FAIL] shellcheck error 级问题需修复"
        fail=1
    fi
else
    echo "  [SKIP] shellcheck 不可用（网络/二进制缺失），跳过本层"
fi

# ── [3/3] 内嵌 spawn 脚本展开验证 ──
echo "[3/3] 内嵌 spawn 脚本展开验证 ..."
API_CGI="$ROOT/app/ui/api.cgi"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

python3 - "$API_CGI" "$WORK/spawn_harness.sh" "$WORK/names.txt" <<'PY'
import re, sys

src = open(sys.argv[1]).read().splitlines()
harness_path, names_path = sys.argv[2], sys.argv[3]
out, names = [], []
i, n = 0, 0
while i < len(src):
    line = src[i]
    if re.match(r'\s*nohup bash -c "$', line):
        n += 1
        name = f'inner_{n}'
        names.append(name)
        out.append(f'{name}="')
        i += 1
        while i < len(src):
            s = src[i]
            stripped = s.strip()
            # 收尾行形如 `" &` 或 `" >/dev/null 2>&1 &`（change_port 块）
            if stripped.startswith('"') and stripped.endswith('&'):
                out.append('"')
                i += 1
                break
            out.append(s)
            i += 1
        continue
    m = re.match(r'\s*local cmd="(.*)"\s*$', line)
    if m:
        n += 1
        name = f'cmd_{n}'
        names.append(name)
        out.append(f'{name}="{m.group(1)}"')
        i += 1
        continue
    i += 1

vars_lines = [l for l in src if re.match(r'^[A-Z_]+="', l)]
stub = [
    'get_service_port(){ echo 18088; }',
    'check_process(){ return 0; }',
    'upgrade_cmd="true"',
    'new_port="18099"',
]
open(harness_path, 'w').write('\n'.join(vars_lines + stub + out) + '\n')
open(names_path, 'w').write('\n'.join(names) + '\n')
print(f'  提取内嵌 spawn 脚本 {n} 段')
PY

if [ ! -f "$WORK/spawn_harness.sh" ]; then
    echo "  [FAIL] 内嵌脚本提取失败"
    exit 1
fi

# 在同一 shell 内 source（子进程变量传不回来），逐段展开验证
source "$WORK/spawn_harness.sh"
while read -r vname; do
    [ -z "$vname" ] && continue
    script="$(eval "printf '%s' \"\${$vname}\"")"
    label="$vname"
    if [ -z "$script" ]; then
        echo "  [FAIL] $label 展开为空"
        fail=1
        continue
    fi
    if ! bash -n <<< "$script" 2>/dev/null; then
        echo "  [FAIL] $label 展开后语法错误:"
        bash -n <<< "$script"
        fail=1
    fi
    case "$script" in
        *QWENPAW_DEPENDENCY_STATE_DIR*) : ;;
        *)
            echo "  [FAIL] $label 缺少 QWENPAW_DEPENDENCY_STATE_DIR（内核 b2+ 启动必需）"
            fail=1
            ;;
    esac
    if [ $fail -eq 0 ]; then
        echo "  [  OK ] $label（展开后 $(printf '%s' "$script" | wc -c) 字符）"
    fi
done < "$WORK/names.txt"

echo "========================================="
if [ $fail -eq 0 ]; then
    echo "lint: ALL CHECKS PASSED"
    exit 0
fi
echo "lint: FAIL"
exit 1
