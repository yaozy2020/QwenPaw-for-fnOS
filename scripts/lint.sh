#!/bin/bash
# lint.sh — 七层门禁
#   [1/7] bash -n 全量与删除/签名路径检查
#   [2/7] shellcheck error 级检查（本地缺失时准备 pinned 版本）
#   [3/7] 内嵌 spawn 脚本按运行时展开语义验证
#   [4/7] 真实 native API/index CGI 集成回归（需先构建 release binaries）
#   [5/7] 前端安装任务归属回归与 esbuild 编译
#   [6/7] 监督者身份校验、安全停止与真实生命周期回归
#   [7/7] 平台配置回调客户端回归
# 用法: bash scripts/lint.sh
# 注意: 不用 set -e（工程铁律），失败显式 exit 1

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHELLCHECK_VER="0.10.0"
fail=0

echo "========== lint: 语法与静态检查 =========="

# ── [1/3] bash -n 全量 ──
echo "[1/7] bash -n ..."
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

# 全局删除门禁：生命周期回调只允许删除 TRIM_* 约束内的路径。
# 历史事故：测试以临时 TRIM_DATA_SHARE_PATHS 运行 uninstall_callback，
# 回调内硬编码的全局路径删除绕过了隔离，误删真实应用数据。
echo "  [gate] uninstall_callback 全局路径删除检查 ..."
if grep -nE '[^#]*rm[[:space:]]+-rf[[:space:]]+"/?vol' "$ROOT/cmd/uninstall_callback" >/dev/null; then
    echo "  [FAIL] cmd/uninstall_callback 含硬编码 /vol* 删除（会绕过 TRIM_* 隔离）"
    fail=1
else
    echo "  [  OK ] 无硬编码 /vol* 删除"
fi

# 产物签名门禁：.sha256 内必须是纯文件名。
# 历史事故：build.sh 用 sha256sum <绝对路径> 生成签名，哈希值对但文件名部分
# 带上了构建机的绝对路径，用户下载后 `sha256sum -c` 因该路径不存在而失败。
echo "  [gate] build.sh 的 .sha256 必须携带纯文件名 ..."
if grep -qE 'sha256sum[[:space:]]+"\$OUTPUT"' "$ROOT/build.sh"; then
    echo "  [FAIL] build.sh 直接对 \$OUTPUT(绝对路径) 调用 sha256sum，签名里会带构建机路径"
    fail=1
else
    echo "  [  OK ] .sha256 以纯文件名生成"
fi

# ── [L0] 生产管理边界：配置核心隔离 HTTP，生产 Python 业务逐步清零 ──
echo "[L0] 管理核心边界 ..."
if python3 "$ROOT/scripts/test-management-boundary.py"; then
    echo "  [  OK ] 管理核心静态边界检查通过"
else
    echo "  [FAIL] 管理核心静态边界检查失败"
    fail=1
fi

# ── [2/3] shellcheck（error 级门禁；warning 仅报告不拦截） ──
echo "[2/7] shellcheck v${SHELLCHECK_VER} ..."
SC_BIN="$(command -v shellcheck 2>/dev/null || true)"
if [ -z "$SC_BIN" ]; then
    CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/qwenpaw-build"
    mkdir -p "$CACHE_DIR"
    SC_BIN="$CACHE_DIR/shellcheck"
    if [ ! -x "$SC_BIN" ]; then
        TARBALL="$CACHE_DIR/shellcheck-${SHELLCHECK_VER}.tar.xz"
        if curl -sL --max-time 120 -o "$TARBALL" \
            "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VER}/shellcheck-v${SHELLCHECK_VER}.linux.x86_64.tar.xz" \
            && tar -xJf "$TARBALL" -C "$CACHE_DIR" "shellcheck-v${SHELLCHECK_VER}/shellcheck" 2>/dev/null; then
            mv "$CACHE_DIR/shellcheck-v${SHELLCHECK_VER}/shellcheck" "$SC_BIN"
            chmod +x "$SC_BIN"
            rm -f "$TARBALL"
        fi
    fi
fi
if [ -n "$SC_BIN" ] && [ -x "$SC_BIN" ]; then
    TARGETS=(cmd/main cmd/install_callback cmd/upgrade_callback cmd/config_callback cmd/uninstall_callback cmd/install_init cmd/upgrade_init cmd/config_init cmd/uninstall_init app/ui/api.cgi app/ui/index.cgi build.sh scripts/preflight.sh scripts/audit-fpk.sh scripts/test-lifecycle.sh scripts/test-venv-recovery.sh scripts/test-config-callback.sh)
    if "$SC_BIN" -S error "${TARGETS[@]/#/"$ROOT/"}" 2>&1; then
        echo "  [  OK ] error 级问题为 0（warning 级见上方输出，不拦截）"
    else
        echo "  [FAIL] shellcheck error 级问题需修复"
        fail=1
    fi
else
    if [ "${STRICT_CI:-0}" = "1" ]; then
        echo "  [FAIL] STRICT_CI 模式下不允许跳过 shellcheck 门禁"
        fail=1
    else
        echo "  [SKIP] shellcheck 不可用（网络/二进制缺失），跳过本层"
    fi
fi

# ── [3/4] 内嵌 spawn 脚本展开验证 ──
# v2 起 spawn 逻辑迁入监督者(kernel.rs process_group),瘦桥无内嵌启动脚本,
# 本层常态提取 0 段 —— 保留作哨兵:若未来重新出现内嵌启动脚本即自动激活。
echo "[3/7] 内嵌 spawn 脚本展开验证 ..."
API_CGI="$ROOT/app/ui/api.cgi"
WORK="$(mktemp -d)" || exit 1
trap 'rm -rf "$WORK"' EXIT INT TERM

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

# ── [4/7] native CGI 集成回归（使用已构建的 release binaries）──
echo "[4/7] native API/index CGI 回归 ..."
for test_script in test-native-api-cgi.py test-native-index-cgi.py; do
    if python3 "$ROOT/scripts/$test_script"; then
        echo "  [  OK ] $test_script"
    else
        echo "  [FAIL] $test_script"
        fail=1
    fi
done

# ── [5/7] 控制台前端编译(esbuild 即语法门禁;内联 JS 语法错误曾令控制台
# 全动态渲染静默失效停在静态默认 —— 编译不过是同一事故类别的门禁) ──
echo "[5/7] 控制台前端编译 ..."
WEB_DIR="$ROOT/web"
if [ -f "$WEB_DIR/src/main.ts" ] && [ -f "$WEB_DIR/src/index.html" ]; then
    NODE_BIN="$(command -v node 2>/dev/null || true)"
    [ -z "$NODE_BIN" ] && [ -x /var/apps/nodejs_v24/target/bin/node ] && NODE_BIN=/var/apps/nodejs_v24/target/bin/node
    if [ -n "$NODE_BIN" ]; then
        # Node 与 npm 常随自定义安装一起放在 PATH 外的同一目录。
        PATH="$(dirname "$NODE_BIN"):$PATH"
        export PATH
    fi
    if [ -n "$NODE_BIN" ] && [ -x "$WEB_DIR/node_modules/.bin/esbuild" ]; then
        if "$NODE_BIN" --test "$WEB_DIR"/test/*.test.mjs; then
            echo "  [  OK ] 前端安装任务归属回归通过"
        else
            echo "  [FAIL] 前端安装任务归属回归失败"
            fail=1
        fi
        if (cd "$WEB_DIR" && BUILD_ID=lint "$NODE_BIN" build.mjs >/dev/null 2>&1); then
            echo "  [  OK ] esbuild 编译通过(单文件产物 dist/index.html)"
        else
            echo "  [FAIL] esbuild 编译失败:"
            (cd "$WEB_DIR" && BUILD_ID=lint "$NODE_BIN" build.mjs)
            fail=1
        fi
    else
        if [ "${STRICT_CI:-0}" = "1" ]; then
            echo "  [FAIL] STRICT_CI 模式下不允许跳过 esbuild 前端编译门禁"
            fail=1
        else
            echo "  [SKIP] esbuild 不可用(缺 node 或 web/node_modules,先 npm install)"
        fi
    fi
else
    if [ "${STRICT_CI:-0}" = "1" ]; then
        echo "  [FAIL] STRICT_CI 模式下 web/src 缺失"
        fail=1
    else
        echo "  [SKIP] web/src 缺失"
    fi
fi

# ── [6/7] 监督者生命周期身份校验回归（P1-01/P1-03，勿删）──
echo "[6/7] 监督者生命周期回归 ..."
if bash "$ROOT/scripts/test-lifecycle.sh"; then
    echo "  [  OK ] 身份校验/安全停止/停止宽限回归通过"
else
    echo "  [FAIL] 监督者生命周期回归失败（陈旧 PID 误杀或宽限不足）"
    fail=1
fi

echo "[7/7] 平台配置客户端回归 ..."
if bash "$ROOT/scripts/test-config-callback.sh"; then
    echo "  [  OK ] 配置回调只提交监督者事务,失败不退回 Shell 写盘"
else
    echo "  [FAIL] 配置回调回归失败"
    fail=1
fi

echo "========================================="
if [ $fail -eq 0 ]; then
    echo "lint: ALL CHECKS PASSED"
    exit 0
fi
echo "lint: FAIL"
exit 1
