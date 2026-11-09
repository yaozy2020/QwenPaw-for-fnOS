#!/bin/bash
# lint.sh — 静态边界、Shell 与前端、生命周期及配置回归门禁
# 用法: bash scripts/lint.sh
# 注意: 不用 set -e（工程铁律），失败显式 exit 1

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHELLCHECK_VER="0.10.0"
fail=0

echo "========== lint: 语法与静态检查 =========="

echo "[1/6] bash -n ..."
for f in "$ROOT"/cmd/* "$ROOT"/build.sh "$ROOT"/scripts/*.sh; do
    [ -f "$f" ] || continue
    if bash -n "$f" 2>/dev/null; then
        echo "  [  OK ] ${f#"$ROOT"/}"
    else
        echo "  [FAIL] ${f#"$ROOT"/}"
        bash -n "$f"
        fail=1
    fi
done

# 生命周期回调只允许删除 TRIM_* 约束内的路径。
echo "  [gate] uninstall_callback 全局路径删除检查 ..."
if grep -nE '[^#]*rm[[:space:]]+-rf[[:space:]]+"/?vol' "$ROOT/cmd/uninstall_callback" >/dev/null; then
    echo "  [FAIL] cmd/uninstall_callback 含硬编码 /vol* 删除（会绕过 TRIM_* 隔离）"
    fail=1
else
    echo "  [  OK ] 无硬编码 /vol* 删除"
fi

# .sha256 内必须携带纯文件名，避免写入构建机绝对路径。
echo "  [gate] build.sh 的 .sha256 必须携带纯文件名 ..."
if grep -qE 'sha256sum[[:space:]]+"\$OUTPUT"' "$ROOT/build.sh"; then
    echo "  [FAIL] build.sh 直接对 \$OUTPUT(绝对路径) 调用 sha256sum"
    fail=1
else
    echo "  [  OK ] .sha256 以纯文件名生成"
fi

echo "[boundary] 管理核心与 Gateway-only 物料边界 ..."
if python3 "$ROOT/scripts/test-management-boundary.py"; then
    echo "  [  OK ] 管理核心静态边界检查通过"
else
    echo "  [FAIL] 管理核心静态边界检查失败"
    fail=1
fi

echo "[2/6] shellcheck v${SHELLCHECK_VER} ..."
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
    TARGETS=(cmd/main cmd/install_callback cmd/upgrade_callback cmd/config_callback cmd/uninstall_callback cmd/install_init cmd/upgrade_init cmd/config_init cmd/uninstall_init build.sh scripts/preflight.sh scripts/audit-fpk.sh scripts/test-lifecycle.sh scripts/test-venv-recovery.sh scripts/test-config-callback.sh)
    if "$SC_BIN" -S error "${TARGETS[@]/#/$ROOT/}" 2>&1; then
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

echo "[3/6] 控制台前端编译与安装任务归属回归 ..."
WEB_DIR="$ROOT/web"
if [ -f "$WEB_DIR/src/main.ts" ] && [ -f "$WEB_DIR/src/index.html" ]; then
    NODE_BIN="$(command -v node 2>/dev/null || true)"
    [ -z "$NODE_BIN" ] && [ -x /var/apps/nodejs_v24/target/bin/node ] && NODE_BIN=/var/apps/nodejs_v24/target/bin/node
    if [ -n "$NODE_BIN" ]; then
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

echo "[4/6] 监督者生命周期身份校验回归 ..."
if bash "$ROOT/scripts/test-lifecycle.sh"; then
    echo "  [  OK ] 身份校验/安全停止/停止宽限回归通过"
else
    echo "  [FAIL] 监督者生命周期回归失败（陈旧 PID 误杀或宽限不足）"
    fail=1
fi

echo "[5/6] 平台配置客户端回归 ..."
if bash "$ROOT/scripts/test-config-callback.sh"; then
    echo "  [  OK ] 配置回调只提交监督者事务,失败不退回 Shell 写盘"
else
    echo "  [FAIL] 配置回调回归失败"
    fail=1
fi

# Native UDS API fixture is retained as the backend regression entry.
echo "[6/6] Gateway UDS test source syntax ..."
if python3 -m ast "$ROOT/scripts/test-gateway-uds.py" >/dev/null; then
    echo "  [  OK ] Gateway UDS Python 源码语法通过"
else
    echo "  [FAIL] Gateway UDS Python 源码语法错误"
    fail=1
fi

echo "========================================="
if [ "$fail" -eq 0 ]; then
    echo "lint: ALL CHECKS PASSED"
    exit 0
fi
echo "lint: FAIL"
exit 1
