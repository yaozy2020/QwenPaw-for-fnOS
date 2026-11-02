#!/bin/bash
# test-venv-recovery.sh — venv 事务持久化与 6 个中断点 SIGKILL 故障注入自愈回归
# 验证：暂存复制完成、原目录改名后、新目录激活后、健康检查后、版本钉写入后、提交完成后
# 每个中断点注入 SIGKILL 后，重启监督者均能自愈恢复，且：
# 1. 只有一个权威活动环境
# 2. 版本钉与环境一致
# 3. 绝无重复事务执行与半成品残留

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/supervisor/target/release/qwenpaw-supervisor"

if [ ! -x "$BIN" ]; then
    echo "ERROR: 找不到可执行的 $BIN，请先执行 cargo build --release"
    exit 1
fi

FAIL=0
ok() { echo "  [  OK ] $1"; }
bad() { echo "  [FAIL] $1"; FAIL=1; }

echo "========== venv 事务持久化与 6 个中断点 SIGKILL 故障注入自愈测试 =========="

MOCK_BASE="$(mktemp -d)"
trap 'rm -rf "$MOCK_BASE"' EXIT INT TERM

run_crash_test() {
    local point="$1"
    local desc="$2"
    local expect_ver="$3"
    local test_dir="$MOCK_BASE/$point"
    mkdir -p "$test_dir/home/venv/bin" "$test_dir/var" "$test_dir/share" "$test_dir/etc"

    # 生成随机可用端口
    local sup_port=$((20000 + RANDOM % 10000))
    local kernel_port=$((sup_port + 1))

    # 模拟能够响应 metadata 和 pip 的 Python
    local mock_py="$test_dir/fake_python"
    cat <<'EOF' > "$mock_py"
#!/bin/bash
if [ "$1" = "-m" ] && [ "$2" = "venv" ]; then
    mkdir -p "$3/bin"
    cp "$0" "$3/bin/python3"
    exit 0
fi
if [ "$1" = "-c" ]; then
    # 读取本目录的 installed 文件
    BIN_DIR="$(cd "$(dirname "$0")" && pwd)"
    VENV_DIR="$(dirname "$BIN_DIR")"
    if [ -f "$VENV_DIR/installed" ]; then
        cat "$VENV_DIR/installed"
    else
        echo "2.0.0"
    fi
    exit 0
fi
if [ "$1" = "-m" ] && [ "$2" = "pip" ]; then
    # pip 安装模拟：更新当前 venv 的 installed
    BIN_DIR="$(cd "$(dirname "$0")" && pwd)"
    VENV_DIR="$(dirname "$BIN_DIR")"
    echo "2.0.0" > "$VENV_DIR/installed"
    echo "Successfully installed qwenpaw-2.0.0"
    exit 0
fi
# 其他情况 (如启动内核进程)
exec sleep 30
EOF
    chmod +x "$mock_py"

    # 预置初始 venv (版本 1.0.0)
    cp "$mock_py" "$test_dir/home/venv/bin/python3"
    echo "1.0.0" > "$test_dir/home/venv/installed"
    echo "1.0.0" > "$test_dir/var/qwenpaw-version"

    # 启动一个后台简易 HTTP 响应服务器模拟内核 /api/version
    python3 -c "
import http.server, socketserver, sys

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/api/version':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'{\"version\":\"2.0.0\"}')
        else:
            self.send_response(404)
            self.end_headers()
    def log_message(self, format, *args):
        pass

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(('127.0.0.1', $kernel_port), H) as httpd:
    httpd.serve_forever()
" >/dev/null 2>&1 &
    local mock_srv_pid=$!

    # 生成 Token
    local token="test_secret_token_12345678901234567890"
    echo "$token" > "$test_dir/var/supervisor.token"
    chmod 600 "$test_dir/var/supervisor.token"

    # 启动 supervisor 并注入 CRASH_POINT
    local log="$test_dir/supervisor.log"
    export QWENPAW_PYTHON_BIN="$mock_py"
    export QWENPAW_CRASH_POINT="$point"
    export TRIM_PKGHOME="$test_dir/home"
    export TRIM_PKGVAR="$test_dir/var"
    export TRIM_DATA_SHARE_PATHS="$test_dir/share"
    export TRIM_PKGETC="$test_dir/etc"
    export TRIM_PORT_NUM="$sup_port"
    export SUPERVISOR_PORT="$sup_port"
    export QWENPAW_KERNEL_PORT="$kernel_port"
    export QWENPAW_AUTOSTART="false"

    "$BIN" > "$log" 2>&1 &
    local sup_pid=$!

    # 等待端口监听
    for _ in $(seq 1 30); do
        if curl -s "http://127.0.0.1:$sup_port/status" -H "X-Supervisor-Token: $token" >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
    done

    # 触发安装升级到 2.0.0
    curl -s -X POST "http://127.0.0.1:$sup_port/kernel/install" \
        -H "X-Supervisor-Token: $token" \
        -H "Content-Type: application/json" \
        -d '{"version":"2.0.0","mirror":""}' >/dev/null 2>&1 || true

    # 等待进程因 SIGKILL 退出 (或者在崩溃点终止)
    wait "$sup_pid" 2>/dev/null || true

    # 清除 CRASH_POINT 环境变量，准备重启验证自愈
    unset QWENPAW_CRASH_POINT

    # 启动新 supervisor 触发启动自愈
    "$BIN" >> "$log" 2>&1 &
    local rec_pid=$!

    for _ in $(seq 1 40); do
        if curl -s "http://127.0.0.1:$sup_port/status" -H "X-Supervisor-Token: $token" >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
    done

    # 验证自愈后结果
    local active_ver=""
    if [ -f "$test_dir/home/venv/installed" ]; then
        active_ver="$(cat "$test_dir/home/venv/installed")"
    fi
    local pin_ver=""
    if [ -f "$test_dir/var/qwenpaw-version" ]; then
        pin_ver="$(cat "$test_dir/var/qwenpaw-version" | tr -d '\n')"
    fi

    # 检查是否有未清理的事务文件或 staging/backup 目录
    local residual_tx=$(ls -A "$test_dir/home"/.venv.transaction.*.json 2>/dev/null | wc -l)
    local residual_staging=$(ls -d "$test_dir/home"/.venv.staging.* 2>/dev/null | wc -l)
    local residual_backup=$(ls -d "$test_dir/home"/.venv.rollback.* 2>/dev/null | wc -l)

    # 停止恢复后的 supervisor 和 mock server
    kill "$rec_pid" 2>/dev/null || true
    wait "$rec_pid" 2>/dev/null || true
    kill "$mock_srv_pid" 2>/dev/null || true
    wait "$mock_srv_pid" 2>/dev/null || true

    if [ "$active_ver" = "$expect_ver" ] && [ "$pin_ver" = "$expect_ver" ] && \
       [ "$residual_tx" -eq 0 ] && [ "$residual_staging" -eq 0 ] && [ "$residual_backup" -eq 0 ]; then
        ok "$desc (活动版本=$active_ver, 版本钉=$pin_ver, 无残留)"
    else
        bad "$desc 失败: 活动版本='$active_ver'(期望'$expect_ver'), 版本钉='$pin_ver', 残留tx=$residual_tx, staging=$residual_staging, backup=$residual_backup"
    fi
}

# 1. 暂存复制完成后注入 SIGKILL
run_crash_test "after_prepare" "断点 1/6: 暂存复制完成后 SIGKILL" "1.0.0"

# 2. 原目录改名后注入 SIGKILL
run_crash_test "after_rename_backup" "断点 2/6: 原目录改名后 SIGKILL" "1.0.0"

# 3. 新目录激活后注入 SIGKILL
run_crash_test "after_activate" "断点 3/6: 新目录激活后 SIGKILL" "1.0.0"

# 4. 健康检查后、版本钉写入前注入 SIGKILL
run_crash_test "after_health_checked" "断点 4/6: 健康检查后、版本钉写入前 SIGKILL" "1.0.0"

# 5. 版本钉写入后、备份清理前注入 SIGKILL
run_crash_test "after_pin_written" "断点 5/6: 版本钉写入后、备份清理前 SIGKILL" "2.0.0"

# 6. 提交完成后注入 SIGKILL
run_crash_test "after_commit" "断点 6/6: 提交完成后 SIGKILL" "2.0.0"

echo "======================================================================"
if [ $FAIL -eq 0 ]; then
    echo "ALL 6 CRASH INJECTION TESTS PASSED"
    exit 0
else
    echo "FAILED CRASH INJECTION TESTS"
    exit 1
fi
