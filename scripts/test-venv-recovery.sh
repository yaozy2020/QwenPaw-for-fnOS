#!/bin/bash
# venv 事务持久化与七个中断点 SIGKILL 故障注入自愈回归。
# 仅使用临时 TRIM 根；受管内核及其健康端点由 qwenpaw-test-runtime 自身提供。
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${QWENPAW_TEST_SUP_BIN:-$ROOT/supervisor/target/release/qwenpaw-supervisor}"
HELPER="${QWENPAW_TEST_RUNTIME_BIN:-$ROOT/supervisor/target/release/qwenpaw-test-runtime}"

absolute_path() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$PWD" "$1" ;;
    esac
}
BIN="$(absolute_path "$BIN")"
HELPER="$(absolute_path "$HELPER")"

for required in python3 curl mktemp sha256sum stat tr grep awk; do
    if ! command -v "$required" >/dev/null 2>&1; then
        printf 'ERROR: 缺少测试依赖: %s\n' "$required" >&2
        exit 1
    fi
done

if [ ! -x "$BIN" ]; then
    printf 'ERROR: 找不到可执行的监督者 binary: %s\n' "$BIN" >&2
    exit 1
fi
if [ ! -x "$HELPER" ]; then
    printf 'ERROR: 找不到可执行的原生测试替身 binary: %s\n' "$HELPER" >&2
    exit 1
fi

BIN_ID="$(stat -Lc '%d:%i' -- "$BIN")" || {
    printf 'ERROR: 无法读取监督者 binary 身份: %s\n' "$BIN" >&2
    exit 1
}
HELPER_ID="$(stat -Lc '%d:%i' -- "$HELPER")" || {
    printf 'ERROR: 无法读取测试替身 binary 身份: %s\n' "$HELPER" >&2
    exit 1
}
BINARY_HASHES="$(sha256sum -- "$BIN" "$HELPER")" || {
    printf 'ERROR: 无法计算监督者/测试替身 SHA256\n' >&2
    exit 1
}
printf '%s\n' '========== venv 事务持久化与七个 SIGKILL 恢复测试 =========='
printf 'supervisor SHA256: %s\n' "${BINARY_HASHES%% *}"
printf 'test-runtime SHA256: %s\n' "$(printf '%s\n' "$BINARY_HASHES" | awk 'NR == 2 { print $1 }')"
printf 'supervisor binary: %s\ntest-runtime binary: %s\n' "$BIN" "$HELPER"

MOCK_BASE=""
DIAG_BASE=""
FAIL=0

cleanup_base() {
    local status=$? unsafe=0 marker
    trap - EXIT
    if [ -n "$MOCK_BASE" ] && [ -d "$MOCK_BASE" ]; then
        for marker in "$MOCK_BASE"/*/cleanup-failed; do
            if [ -f "$marker" ]; then
                unsafe=1
                break
            fi
        done
        if [ "$unsafe" -ne 0 ]; then
            printf 'ERROR: 有用例未能确认所有受管进程已停止，保留隔离目录: %s\n' "$MOCK_BASE" >&2
            status=1
        elif ! rm -rf -- "$MOCK_BASE"; then
            printf 'ERROR: 无法清理临时测试目录: %s\n' "$MOCK_BASE" >&2
            status=1
        fi
    fi
    exit "$status"
}

preserve_failure_diagnostics() {
    local source="$1" label="$2" target file
    if [ -z "$DIAG_BASE" ]; then
        if [ -n "${QWENPAW_TEST_DIAG_DIR:-}" ]; then
            if ! mkdir -p -- "$QWENPAW_TEST_DIAG_DIR"; then
                printf 'ERROR: 无法创建诊断日志目录: %s\n' "$QWENPAW_TEST_DIAG_DIR" >&2
                return 1
            fi
            DIAG_BASE="$(mktemp -d "$QWENPAW_TEST_DIAG_DIR/venv-recovery.XXXXXX")" || return 1
        else
            DIAG_BASE="$(mktemp -d "${TMPDIR:-/tmp}/qwenpaw-venv-recovery-diagnostics.XXXXXX")" || return 1
        fi
    fi
    target="$DIAG_BASE/$label"
    if ! mkdir -p -- "$target"; then
        printf 'ERROR: 无法创建用例诊断目录: %s\n' "$target" >&2
        return 1
    fi
    for file in \
        "$source/supervisor.log" "$source/var/supervisor.log" \
        "$source/var/qwenpaw-upgrade.log.pip" "$source/var/qwenpaw.log" \
        "$source/var/qwenpaw-version" "$source/install-response.json" \
        "$source/status-response.json" "$source/recovery-status.json" \
        "$source"/home/.venv.transaction.*.json "$source"/init-* \
        "$source"/pip-* "$source/share/.qwenpaw/config.json"; do
        if [ -f "$file" ] && ! cp -- "$file" "$target/"; then
            printf 'ERROR: 无法保存诊断文件: %s\n' "$file" >&2
            return 1
        fi
    done
    printf '  failure diagnostics: %s\n' "$target" >&2
}

MOCK_BASE="$(mktemp -d "${TMPDIR:-/tmp}/qwenpaw-venv-recovery.XXXXXX")" || {
    printf 'ERROR: 无法创建临时测试目录\n' >&2
    exit 1
}
trap cleanup_base EXIT

proc_rest() {
    local stat_line
    IFS= read -r stat_line < "/proc/$1/stat" || return 1
    printf '%s' "${stat_line##*) }"
}

proc_state() {
    local rest
    rest="$(proc_rest "$1")" || return 1
    printf '%s\n' "${rest%% *}"
}

proc_start_time() {
    local rest
    rest="$(proc_rest "$1")" || return 1
    set -- $rest
    [ "$#" -ge 20 ] || return 1
    printf '%s\n' "${20}"
}

proc_alive() {
    local state
    state="$(proc_state "$1" 2>/dev/null)" || return 1
    [ -n "$state" ] && [ "$state" != Z ] && [ "$state" != X ]
}

proc_identity_matches() {
    local pid="$1" expected_id="$2" test_root="$3" expected_start="$4"
    local actual_id actual_start
    actual_id="$(stat -Lc '%d:%i' -- "/proc/$pid/exe" 2>/dev/null)" || return 1
    [ "$actual_id" = "$expected_id" ] || return 1
    actual_start="$(proc_start_time "$pid" 2>/dev/null)" || return 1
    if [ -n "$expected_start" ] && [ "$actual_start" != "$expected_start" ]; then
        return 1
    fi
    tr '\0' '\n' < "/proc/$pid/environ" 2>/dev/null | grep -Fqx -- "TRIM_PKGHOME=$test_root/home" || return 1
    return 0
}

supervisor_identity_matches() {
    proc_identity_matches "$1" "$BIN_ID" "$2" "$3"
}

runtime_identity_matches() {
    local pid="$1" expected_exe_id="$2" test_root="$3" expected_start="$4" cmdline
    proc_identity_matches "$pid" "$expected_exe_id" "$test_root" "$expected_start" || return 1
    cmdline="$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)" || return 1
    case "$cmdline" in
        *"-m qwenpaw app"*) return 0 ;;
        *) return 1 ;;
    esac
}

port_accepts_connection() {
    python3 - "$1" <<'PYPORT'
import socket, sys
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.settimeout(0.25)
try:
    sock.connect(("127.0.0.1", int(sys.argv[1])))
except OSError:
    sys.exit(1)
else:
    sys.exit(0)
finally:
    sock.close()
PYPORT
}

assert_port_closed() {
    local port="$1" label="$2"
    for _ in 1 2 3 4 5; do
        if port_accepts_connection "$port"; then
            printf '  [FAIL] %s: 端口 %s 在受管进程停止后仍可连接\n' "$label" "$port" >&2
            return 1
        fi
        sleep 0.1
    done
    return 0
}

choose_port_pair() {
    local attempt candidate kernel
    PORT_SUP=""
    PORT_KERNEL=""
    for ((attempt = 1; attempt <= 40; attempt++)); do
        candidate=$((20000 + RANDOM % 10000))
        kernel=$((candidate + 1))
        if python3 - "$candidate" "$kernel" <<'PYPROBE'
import socket, sys
sockets = []
try:
    for raw_port in sys.argv[1:]:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sockets.append(sock)
        sock.bind(("127.0.0.1", int(raw_port)))
except OSError:
    sys.exit(1)
finally:
    for sock in sockets:
        sock.close()
sys.exit(0)
PYPROBE
        then
            PORT_SUP="$candidate"
            PORT_KERNEL="$kernel"
            return 0
        fi
    done
    return 1
}

wait_http_ready() {
    local port="$1" token="$2" response_file="$3" pid="$4" attempt code
    for ((attempt = 1; attempt <= 40; attempt++)); do
        if code="$(curl --silent --show-error --connect-timeout 1 --max-time 2 \
            --output "$response_file" --write-out '%{http_code}' \
            -H "X-Supervisor-Token: $token" "http://127.0.0.1:$port/status" 2>/dev/null)"; then
            if [[ "$code" == 2?? ]] && [ -s "$response_file" ]; then
                return 0
            fi
        fi
        if ! proc_alive "$pid"; then
            break
        fi
        sleep 0.1
    done
    return 1
}

OWNED_SUP_PID=""
OWNED_SUP_ROOT=""
OWNED_SUP_START=""
SUP_WAIT_STATUS=""
RUNTIME_PID=""
RUNTIME_START=""
RUNTIME_EXE_ID=""
RUNTIME_ROOT=""

stop_owned_supervisor() {
    local pid="$OWNED_SUP_PID" test_root="$OWNED_SUP_ROOT" start="$OWNED_SUP_START" status
    [ -n "$pid" ] || return 0
    if proc_alive "$pid"; then
        if ! supervisor_identity_matches "$pid" "$test_root" "$start"; then
            if proc_alive "$pid"; then
                printf '  [FAIL] supervisor PID %s 身份不匹配，拒绝发送信号\n' "$pid" >&2
                return 1
            fi
        else
            start="$(proc_start_time "$pid")" || start=""
            if [ -z "$start" ]; then
                if proc_alive "$pid"; then
                    printf '  [FAIL] 无法读取 supervisor PID %s 的 starttime，拒绝发送信号\n' "$pid" >&2
                    return 1
                fi
            elif supervisor_identity_matches "$pid" "$test_root" "$start"; then
                if ! kill -KILL "$pid" 2>/dev/null && proc_alive "$pid"; then
                    printf '  [FAIL] 无法停止已核验身份的 supervisor PID %s\n' "$pid" >&2
                    return 1
                fi
            elif proc_alive "$pid"; then
                printf '  [FAIL] supervisor PID %s 身份在停止前变化，拒绝发送信号\n' "$pid" >&2
                return 1
            fi
        fi
    fi
    if wait "$pid" 2>/dev/null; then
        status=0
    else
        status=$?
    fi
    SUP_WAIT_STATUS="$status"
    OWNED_SUP_PID=""
    OWNED_SUP_ROOT=""
    OWNED_SUP_START=""
    return 0
}

read_runtime_identity() {
    local marker="$1"
    [ -r "$marker" ] || return 1
    read -r RUNTIME_PID RUNTIME_START < "$marker" || return 1
    [[ "$RUNTIME_PID" =~ ^[0-9]+$ && "$RUNTIME_START" =~ ^[0-9]+$ ]] || return 1
    return 0
}

stop_runtime() {
    local pid="$1" start="$2" expected_exe_id="$3" test_root="$4" port="$5" attempt
    [ -n "$pid" ] || return 0
    if proc_alive "$pid"; then
        if ! runtime_identity_matches "$pid" "$expected_exe_id" "$test_root" "$start"; then
            if proc_alive "$pid"; then
                printf '  [FAIL] 受管替身 PID %s 身份不匹配，拒绝发送信号\n' "$pid" >&2
                return 1
            fi
        else
            if ! kill -KILL "$pid" 2>/dev/null && proc_alive "$pid"; then
                printf '  [FAIL] 无法停止已核验身份的受管替身 PID %s\n' "$pid" >&2
                return 1
            fi
            for ((attempt = 1; attempt <= 50; attempt++)); do
                if ! proc_alive "$pid"; then
                    break
                fi
                if ! runtime_identity_matches "$pid" "$expected_exe_id" "$test_root" "$start"; then
                    if ! proc_alive "$pid"; then
                        break
                    fi
                    printf '  [FAIL] 受管替身 PID %s 停止期间身份改变，拒绝继续发送信号\n' "$pid" >&2
                    return 1
                fi
                sleep 0.1
            done
            if proc_alive "$pid"; then
                printf '  [FAIL] 受管替身 PID %s 未在期限内停止\n' "$pid" >&2
                return 1
            fi
        fi
    fi
    assert_port_closed "$port" "受管替身清理"
}

cleanup_run() {
    local status=$? cleanup_failed=0
    trap - EXIT
    if [ -n "$OWNED_SUP_PID" ]; then
        if ! stop_owned_supervisor; then
            cleanup_failed=1
        fi
    fi
    if [ -z "$RUNTIME_PID" ] && [ -n "${RUN_TEST_DIR:-}" ] && \
       [ -r "$RUN_TEST_DIR/home/venv/.qwenpaw-test-runtime.pid" ]; then
        if ! read_runtime_identity "$RUN_TEST_DIR/home/venv/.qwenpaw-test-runtime.pid"; then
            printf '  [FAIL] 无法读取受管替身 PID 身份记录\n' >&2
            cleanup_failed=1
        else
            RUNTIME_ROOT="$RUN_TEST_DIR"
        fi
    fi
    if [ -n "$RUNTIME_PID" ]; then
        if [ -z "$RUNTIME_EXE_ID" ] && [ -n "${RUN_TEST_DIR:-}" ] && \
           [ -e "$RUN_TEST_DIR/home/venv/bin/python3" ]; then
            RUNTIME_EXE_ID="$(stat -Lc '%d:%i' -- "$RUN_TEST_DIR/home/venv/bin/python3")" || RUNTIME_EXE_ID=""
        fi
        if [ -z "$RUNTIME_EXE_ID" ] || \
           ! stop_runtime "$RUNTIME_PID" "$RUNTIME_START" "$RUNTIME_EXE_ID" "$RUNTIME_ROOT" "$RUN_KERNEL_PORT"; then
            cleanup_failed=1
        fi
    fi
    if [ "$cleanup_failed" -ne 0 ]; then
        printf '  [FAIL] 测试清理未完整完成\n' >&2
        if [ -n "${RUN_TEST_DIR:-}" ] && [ -d "$RUN_TEST_DIR" ]; then
            : > "$RUN_TEST_DIR/cleanup-failed"
        fi
        status=1
    fi
    if [ "$status" -ne 0 ] && [ -n "${RUN_TEST_DIR:-}" ] && [ -d "$RUN_TEST_DIR" ]; then
        if ! preserve_failure_diagnostics "$RUN_TEST_DIR" "${point:-unknown}"; then
            printf '  [FAIL] 无法保存本例失败诊断日志\n' >&2
        fi
    fi
    exit "$status"
}

verify_install_response() {
    python3 - "$1" <<'PYRESP'
import json, pathlib, sys
payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
if payload.get("ok") is not True or payload.get("success") is not True:
    raise SystemExit("安装请求未被成功受理: " + repr(payload))
PYRESP
}

verify_endpoint_version() {
    local port="$1" expected="$2" body_file="$3"
    curl --silent --show-error --fail --connect-timeout 1 --max-time 3 \
        "http://127.0.0.1:$port/api/version" -o "$body_file" 2>/dev/null || return 1
    python3 - "$body_file" "$expected" <<'PYVERSION'
import json, pathlib, sys
payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
if payload.get("version") != sys.argv[2]:
    raise SystemExit(f"/api/version={payload.get('version')!r}, expected {sys.argv[2]!r}")
PYVERSION
}

verify_initialization() {
    python3 - "$1" "$2" "$3" <<'PYINIT'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
expected = int(sys.argv[2])
stage = sys.argv[3]
count_path = root / "init-count"
count = int(count_path.read_text().strip()) if count_path.is_file() else 0
if count != expected:
    raise SystemExit(f"init-count={count}, expected {expected} at {stage}")
invocations_path = root / "init-invocations.jsonl"
invocations = [json.loads(line) for line in invocations_path.read_text().splitlines()] if invocations_path.is_file() else []
if len(invocations) != expected:
    raise SystemExit(f"init invocation records={len(invocations)}, expected {expected} at {stage}")
started_path = root / "init-started"
started = int(started_path.read_text().strip()) if started_path.is_file() else 0
if started != expected:
    raise SystemExit(f"init-started={started}, expected {expected} at {stage}")
share = root / "share" / ".qwenpaw"
config = share / "config.json"
if stage == "before-recovery":
    if expected and (not config.is_file() or config.read_text() != "{}\n"):
        raise SystemExit(f"init did not create expected config under {config}")
    if not expected and config.exists():
        raise SystemExit("QwenPaw config appeared before the injected point")
if expected:
    record = invocations[0]
    if record.get("count") != 1 or record.get("argv") != ["-m", "qwenpaw", "init", "--defaults"]:
        raise SystemExit(f"unexpected initialization invocation: {record!r}")
    environment = record.get("env", {})
    if environment.get("QWENPAW_TEST_CONTROL") != str(root):
        raise SystemExit("init invocation was not recorded in this isolated test directory")
    if environment.get("QWENPAW_WORKING_DIR") != str(share):
        raise SystemExit(f"init working directory escaped isolated share: {environment.get('QWENPAW_WORKING_DIR')!r}")
    if not (root / "pip-started").is_file():
        raise SystemExit("test runtime did not record its isolated pip call")
PYINIT
}

verify_recovery_data() {
    python3 - "$1" "$2" <<'PYDATA'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
point = sys.argv[2]
config = root / "share" / ".qwenpaw" / "config.json"
expected = json.dumps({"recovery_sentinel": point}, separators=(",", ":")) + "\n"
if not config.is_file() or config.read_text() != expected:
    raise SystemExit(f"recovery changed or lost existing QwenPaw data in {config}")
PYDATA
}

verify_crash_state() {
    python3 - "$1" "$2" <<'PYCRASH'
import glob, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
point = sys.argv[2]
home = root / "home"
active = home / "venv"
pin = root / "var" / "qwenpaw-version"
txs = sorted(glob.glob(str(home / ".venv.transaction.*.json")))
if len(txs) != 1:
    raise SystemExit(f"expected one transaction record, found {txs!r}")
record = json.loads(pathlib.Path(txs[0]).read_text())
phases = {
    "after_prepare": "prepared",
    "after_rename_backup": "backed_up",
    "after_activate": "activated",
    "after_health_checked": "health_checked",
    "after_pin_written": "committed",
    "after_commit": "committed",
    "after_spawn_before_commit": "activated",
}
if record.get("phase") != phases[point]:
    raise SystemExit(f"transaction phase={record.get('phase')!r}, expected {phases[point]!r}")
if pathlib.Path(record["active"]) != active or pathlib.Path(record["pin_path"]) != pin:
    raise SystemExit("transaction paths escaped the isolated test root")
staged = pathlib.Path(record["staged"])
backup = pathlib.Path(record["backup"])
active_version = (active / "installed").read_text().strip() if (active / "installed").is_file() else None
backup_version = (backup / "installed").read_text().strip() if (backup / "installed").is_file() else None
pin_version = pin.read_text().strip() if pin.is_file() else None
expected = {
    "after_prepare": ("prepared", "1.0.0", None, True, False, "1.0.0"),
    "after_rename_backup": ("backed_up", None, "1.0.0", True, True, "1.0.0"),
    "after_activate": ("activated", "2.0.0", "1.0.0", False, True, "1.0.0"),
    "after_health_checked": ("health_checked", "2.0.0", "1.0.0", False, True, "1.0.0"),
    "after_pin_written": ("committed", "2.0.0", "1.0.0", False, True, "2.0.0"),
    "after_commit": ("committed", "2.0.0", None, False, False, "2.0.0"),
    "after_spawn_before_commit": ("activated", "2.0.0", "1.0.0", False, True, "1.0.0"),
}[point]
_, want_active, want_backup, want_staged, want_backup_dir, want_pin = expected
if active_version != want_active or backup_version != want_backup or pin_version != want_pin:
    raise SystemExit(
        f"crash state mismatch: active={active_version!r}, backup={backup_version!r}, pin={pin_version!r}"
    )
if staged.exists() != want_staged or backup.exists() != want_backup_dir:
    raise SystemExit(
        f"crash directories mismatch: staged={staged.exists()}, backup={backup.exists()}"
    )
marker = active / ".qwenpaw-test-runtime.pid"
health_log = active / ".qwenpaw-test-runtime-health"
needs_runtime = point in {
    "after_health_checked", "after_pin_written", "after_commit", "after_spawn_before_commit"
}
if needs_runtime:
    if not marker.is_file() or not health_log.is_file():
        raise SystemExit("the managed runtime did not bind and serve the kernel health endpoint")
    identity = marker.read_text().split()
    if len(identity) != 2 or not identity[0].isdigit() or not identity[1].isdigit():
        raise SystemExit("invalid managed runtime identity marker")
    if not any(line.strip() == f"pid={identity[0]} version=2.0.0" for line in health_log.read_text().splitlines()):
        raise SystemExit("KernelManager health check did not receive the installed version from the managed runtime")
elif marker.exists() or health_log.exists():
    raise SystemExit("managed runtime started before this crash injection point")
PYCRASH
}

run_crash_test() (
    local point="$1" desc="$2" expect_ver="$3"
    local test_dir="" token="test_secret_token_12345678901234567890"
    local sup_pid="" sup_ready=0 install_curl_rc=0 install_http_code=""
    local response_file="" install_response="$MOCK_BASE/$point/install-response.json"
    local expected_runtime=0 runtime_marker="" runtime_alive=0
    local status_response="$MOCK_BASE/$point/status-response.json"
    local installed_version="" pin_version="" tx_count=0 staging_count=0 backup_count=0
    local expected_init=0 attempt
    case "$point" in
        after_health_checked|after_pin_written|after_commit|after_spawn_before_commit)
            expected_init=1
            ;;
    esac
    RUN_TEST_DIR=""
    RUN_KERNEL_PORT=""
    OWNED_SUP_PID=""
    OWNED_SUP_ROOT=""
    OWNED_SUP_START=""
    SUP_WAIT_STATUS=""
    RUNTIME_PID=""
    RUNTIME_START=""
    RUNTIME_EXE_ID=""
    RUNTIME_ROOT=""
    trap cleanup_run EXIT

    if ! choose_port_pair; then
        printf '  [FAIL] %s: 连续 40 次端口前置探测失败，未执行用例\n' "$desc" >&2
        exit 1
    fi
    test_dir="$MOCK_BASE/$point"
    RUN_TEST_DIR="$test_dir"
    RUN_KERNEL_PORT="$PORT_KERNEL"
    if ! mkdir -p "$test_dir/home/venv/bin" "$test_dir/var" "$test_dir/share" \
        "$test_dir/etc" "$test_dir/appdest/ui"; then
        printf '  [FAIL] %s: 无法创建隔离测试目录\n' "$desc" >&2
        exit 1
    fi
    if ! cp -- "$HELPER" "$test_dir/home/venv/bin/python3"; then
        printf '  [FAIL] %s: 无法安装原生测试替身到初始 venv\n' "$desc" >&2
        exit 1
    fi
    if ! printf '1.0.0\n' > "$test_dir/home/venv/installed" || \
       ! printf '1.0.0\n' > "$test_dir/var/qwenpaw-version"; then
        printf '  [FAIL] %s: 无法写入隔离初始版本\n' "$desc" >&2
        exit 1
    fi
    if ! printf '%s\n' "$token" > "$test_dir/var/supervisor-token" || \
       ! chmod 600 "$test_dir/var/supervisor-token"; then
        printf '  [FAIL] %s: 无法创建隔离 supervisor token\n' "$desc" >&2
        exit 1
    fi
    if ! printf '{\n  ".url": {\n    "qwenpaw.main": {"title": "QwenPaw", "icon": "images/icon.png", "type": "url", "protocol": "http", "port": "%s", "url": "/", "allUsers": true}\n  }\n}\n' \
        "$PORT_KERNEL" > "$test_dir/appdest/ui/config"; then
        printf '  [FAIL] %s: 无法写入隔离 UI 配置\n' "$desc" >&2
        exit 1
    fi

    export QWENPAW_PYTHON_BIN="$HELPER"
    export QWENPAW_TEST_CONTROL="$test_dir"
    export QWENPAW_CRASH_POINT="$point"
    export QWENPAW_ENABLE_CRASH_POINT=1
    export TRIM_PKGHOME="$test_dir/home"
    export TRIM_PKGVAR="$test_dir/var"
    export TRIM_DATA_SHARE_PATHS="$test_dir/share"
    export TRIM_PKGETC="$test_dir/etc"
    export SUPERVISOR_PORT="$PORT_SUP"
    export TRIM_SERVICE_PORT="$PORT_KERNEL"
    export TRIM_APPDEST="$test_dir/appdest"
    export SUPERVISOR_AUTOSTART=0

    (cd "$test_dir" && exec "$BIN") > "$test_dir/supervisor.log" 2>&1 &
    sup_pid=$!
    OWNED_SUP_PID="$sup_pid"
    OWNED_SUP_ROOT="$test_dir"
    for ((attempt = 1; attempt <= 50; attempt++)); do
        if supervisor_identity_matches "$sup_pid" "$test_dir" ""; then
            OWNED_SUP_START="$(proc_start_time "$sup_pid")" || OWNED_SUP_START=""
            if [ -n "$OWNED_SUP_START" ] && supervisor_identity_matches "$sup_pid" "$test_dir" "$OWNED_SUP_START"; then
                break
            fi
        fi
        if ! proc_alive "$sup_pid"; then
            break
        fi
        sleep 0.02
    done
    if [ -z "$OWNED_SUP_START" ]; then
        printf '  [FAIL] %s: 无法核验新启动 supervisor 的进程身份\n' "$desc" >&2
        if [ -e "/proc/$sup_pid/stat" ]; then
            printf '  [DIAG] supervisor pid=%s expected-exe=%s actual-exe=%s expected-start=%s actual-start=%s home-match=%s\n' \
                "$sup_pid" "$BIN_ID" "$(stat -Lc '%d:%i' -- "/proc/$sup_pid/exe" 2>/dev/null || printf unavailable)" \
                "$OWNED_SUP_START" "$(proc_start_time "$sup_pid" 2>/dev/null || printf unavailable)" \
                "$(if tr '\0' '\n' < "/proc/$sup_pid/environ" 2>/dev/null | grep -Fqx -- "TRIM_PKGHOME=$test_dir/home"; then printf yes; else printf no; fi)" >&2
        fi
        exit 1
    fi

    if wait_http_ready "$PORT_SUP" "$token" "$status_response" "$sup_pid"; then
        sup_ready=1
    fi
    if [ "$sup_ready" -ne 1 ]; then
        printf '  [FAIL] %s: supervisor 未在期限内就绪\n' "$desc" >&2
        exit 1
    fi

    if install_http_code="$(curl --silent --show-error --connect-timeout 2 --max-time 10 \
        --output "$install_response" --write-out '%{http_code}' -X POST \
        "http://127.0.0.1:$PORT_SUP/kernel/install" \
        -H "X-Supervisor-Token: $token" -H 'Content-Type: application/json' \
        -d '{"version":"2.0.0","mirror":""}' 2>/dev/null)"; then
        install_curl_rc=0
        if [[ "$install_http_code" == 2?? ]] && [ -s "$install_response" ]; then
            if ! verify_install_response "$install_response"; then
                printf '  [FAIL] %s: 安装请求未成功受理 (HTTP %s)\n' "$desc" "$install_http_code" >&2
                exit 1
            fi
        elif [[ "$install_http_code" == 4?? || "$install_http_code" == 5?? ]]; then
            printf '  [FAIL] %s: 安装请求被拒绝 (HTTP %s)\n' "$desc" "$install_http_code" >&2
            exit 1
        fi
    else
        install_curl_rc=$?
    fi

    local exited=0
    for ((attempt = 1; attempt <= 300; attempt++)); do
        if ! proc_alive "$sup_pid"; then
            exited=1
            break
        fi
        if ! supervisor_identity_matches "$sup_pid" "$test_dir" "$OWNED_SUP_START"; then
            if ! proc_alive "$sup_pid"; then
                exited=1
                break
            fi
            printf '  [FAIL] %s: 等待断点时 supervisor 身份变化，拒绝发送信号\n' "$desc" >&2
            exit 1
        fi
        sleep 0.1
    done
    if [ "$exited" -ne 1 ]; then
        printf '  [FAIL] %s: 30 秒内未到达 SIGKILL 注入点\n' "$desc" >&2
        printf '  [DIAG] install response: %s\n' "$(cat "$install_response" 2>/dev/null || printf unavailable)" >&2
        printf '  [DIAG] install status:\n' >&2
        curl --silent --show-error --connect-timeout 1 --max-time 2 \
            -H "X-Supervisor-Token: $token" \
            "http://127.0.0.1:$PORT_SUP/kernel/install-status" >&2 2>/dev/null || true
        printf '\n  [DIAG] supervisor log tail:\n' >&2
        tail -n 60 "$test_dir/var/supervisor.log" >&2 2>/dev/null || true
        printf '  [DIAG] pip log tail:\n' >&2
        tail -n 60 "$test_dir/var/qwenpaw-upgrade.log.pip" >&2 2>/dev/null || true
        if ! stop_owned_supervisor; then
            printf '  [FAIL] %s: 未能安全清理超时 supervisor\n' "$desc" >&2
        fi
        exit 1
    fi
    if wait "$sup_pid" 2>/dev/null; then
        SUP_WAIT_STATUS=0
    else
        SUP_WAIT_STATUS=$?
    fi
    OWNED_SUP_PID=""
    OWNED_SUP_ROOT=""
    OWNED_SUP_START=""
    if [ "$SUP_WAIT_STATUS" -ne 137 ]; then
        printf '  [FAIL] %s: supervisor 退出码=%s，预期 SIGKILL (137)\n' "$desc" "$SUP_WAIT_STATUS" >&2
        exit 1
    fi

    runtime_marker="$test_dir/home/venv/.qwenpaw-test-runtime.pid"
    case "$point" in
        after_health_checked|after_pin_written|after_commit|after_spawn_before_commit)
            expected_runtime=1
            if ! read_runtime_identity "$runtime_marker"; then
                printf '  [FAIL] %s: 缺少受管替身自身写入的 PID/starttime 身份记录\n' "$desc" >&2
                exit 1
            fi
            RUNTIME_ROOT="$test_dir"
            RUNTIME_EXE_ID="$(stat -Lc '%d:%i' -- "$test_dir/home/venv/bin/python3")" || RUNTIME_EXE_ID=""
            if [ -z "$RUNTIME_EXE_ID" ] || \
               ! runtime_identity_matches "$RUNTIME_PID" "$RUNTIME_EXE_ID" "$test_dir" "$RUNTIME_START"; then
                printf '  [FAIL] %s: 健康服务 PID 与受管替身身份不匹配\n' "$desc" >&2
                exit 1
            fi
            ;;
    esac

    if ! verify_crash_state "$test_dir" "$point"; then
        printf '  [FAIL] %s: 事务状态未证明已到达指定注入点\n' "$desc" >&2
        exit 1
    fi
    if ! verify_initialization "$test_dir" "$expected_init" before-recovery; then
        printf '  [FAIL] %s: 初始化路径/调用次数与断点不符\n' "$desc" >&2
        exit 1
    fi
    if ! mkdir -p "$test_dir/share/.qwenpaw" || \
       ! printf '{"recovery_sentinel":"%s"}\n' "$point" > "$test_dir/share/.qwenpaw/config.json"; then
        printf '  [FAIL] %s: 无法建立恢复期间应保留的隔离用户数据\n' "$desc" >&2
        exit 1
    fi
    if [ "$expected_runtime" -eq 1 ]; then
        if ! verify_endpoint_version "$PORT_KERNEL" "2.0.0" "$test_dir/version-before-recovery.json"; then
            printf '  [FAIL] %s: 受管替身 /api/version 未返回实际 installed 版本 2.0.0\n' "$desc" >&2
            exit 1
        fi
    fi

    unset QWENPAW_CRASH_POINT
    (cd "$test_dir" && exec "$BIN") >> "$test_dir/supervisor.log" 2>&1 &
    sup_pid=$!
    OWNED_SUP_PID="$sup_pid"
    OWNED_SUP_ROOT="$test_dir"
    OWNED_SUP_START=""
    for ((attempt = 1; attempt <= 50; attempt++)); do
        if supervisor_identity_matches "$sup_pid" "$test_dir" ""; then
            OWNED_SUP_START="$(proc_start_time "$sup_pid")" || OWNED_SUP_START=""
            if [ -n "$OWNED_SUP_START" ] && supervisor_identity_matches "$sup_pid" "$test_dir" "$OWNED_SUP_START"; then
                break
            fi
        fi
        if ! proc_alive "$sup_pid"; then
            break
        fi
        sleep 0.02
    done
    if [ -z "$OWNED_SUP_START" ]; then
        printf '  [FAIL] %s: 无法核验恢复 supervisor 的进程身份\n' "$desc" >&2
        exit 1
    fi
    response_file="$test_dir/recovery-status.json"
    if ! wait_http_ready "$PORT_SUP" "$token" "$response_file" "$sup_pid"; then
        printf '  [FAIL] %s: 启动恢复后的 supervisor 未就绪\n' "$desc" >&2
        exit 1
    fi
    if ! verify_initialization "$test_dir" "$expected_init" after-recovery || \
       ! verify_recovery_data "$test_dir" "$point"; then
        printf '  [FAIL] %s: 恢复重复初始化或覆盖了既有 QwenPaw 数据\n' "$desc" >&2
        exit 1
    fi

    if [ -f "$test_dir/home/venv/installed" ]; then
        installed_version="$(tr -d '\n' < "$test_dir/home/venv/installed")" || {
            printf '  [FAIL] %s: 无法读取恢复后的 installed 版本\n' "$desc" >&2
            exit 1
        }
    fi
    if [ -f "$test_dir/var/qwenpaw-version" ]; then
        pin_version="$(tr -d '\n' < "$test_dir/var/qwenpaw-version")" || {
            printf '  [FAIL] %s: 无法读取恢复后的版本钉\n' "$desc" >&2
            exit 1
        }
    fi
    local -a tx_files=("$test_dir"/home/.venv.transaction.*.json)
    local -a staging_dirs=("$test_dir"/home/.venv.staging.*)
    local -a backup_dirs=("$test_dir"/home/.venv.rollback.*)
    [ -e "${tx_files[0]}" ] || tx_files=()
    [ -e "${staging_dirs[0]}" ] || staging_dirs=()
    [ -e "${backup_dirs[0]}" ] || backup_dirs=()
    tx_count="${#tx_files[@]}"
    staging_count="${#staging_dirs[@]}"
    backup_count="${#backup_dirs[@]}"
    if [ "$installed_version" != "$expect_ver" ] || [ "$pin_version" != "$expect_ver" ] || \
       [ "$tx_count" -ne 0 ] || [ "$staging_count" -ne 0 ] || [ "$backup_count" -ne 0 ]; then
        printf '  [FAIL] %s: 恢复不一致 active=%s pin=%s tx=%s staging=%s backup=%s\n' \
            "$desc" "$installed_version" "$pin_version" "$tx_count" "$staging_count" "$backup_count" >&2
        exit 1
    fi

    case "$point" in
        after_pin_written|after_commit)
            if ! proc_alive "$RUNTIME_PID" || \
               ! runtime_identity_matches "$RUNTIME_PID" "$RUNTIME_EXE_ID" "$test_dir" "$RUNTIME_START"; then
                printf '  [FAIL] %s: 已提交用例的受管内核未在恢复后继续运行\n' "$desc" >&2
                exit 1
            fi
            if ! verify_endpoint_version "$PORT_KERNEL" "$expect_ver" "$test_dir/version-after-recovery.json"; then
                printf '  [FAIL] %s: 恢复后的健康端点与实际 installed 版本不符\n' "$desc" >&2
                exit 1
            fi
            ;;
        after_health_checked|after_spawn_before_commit)
            if proc_alive "$RUNTIME_PID"; then
                if ! runtime_identity_matches "$RUNTIME_PID" "$RUNTIME_EXE_ID" "$test_dir" "$RUNTIME_START" && \
                   proc_alive "$RUNTIME_PID"; then
                    printf '  [FAIL] %s: 回滚后同 PID 已變更身份，拒绝信号且测试失败\n' "$desc" >&2
                    exit 1
                fi
                if proc_alive "$RUNTIME_PID"; then
                    printf '  [FAIL] %s: 回滚后仍有中断安装启动的受管内核\n' "$desc" >&2
                    exit 1
                fi
            fi
            if ! assert_port_closed "$PORT_KERNEL" "$desc 恢复回滚"; then
                exit 1
            fi
            ;;
        *)
            if ! assert_port_closed "$PORT_KERNEL" "$desc 未启动内核"; then
                exit 1
            fi
            ;;
    esac

    if ! stop_owned_supervisor; then
        printf '  [FAIL] %s: 恢复 supervisor 清理失败\n' "$desc" >&2
        exit 1
    fi
    if [ "$SUP_WAIT_STATUS" -ne 137 ]; then
        printf '  [FAIL] %s: 恢复 supervisor SIGKILL 清理退出码=%s，预期 137\n' "$desc" "$SUP_WAIT_STATUS" >&2
        exit 1
    fi
    if [ -n "$RUNTIME_PID" ]; then
        if ! stop_runtime "$RUNTIME_PID" "$RUNTIME_START" "$RUNTIME_EXE_ID" "$test_dir" "$PORT_KERNEL"; then
            printf '  [FAIL] %s: 受管内核身份核验/清理失败\n' "$desc" >&2
            exit 1
        fi
    fi

    printf '  [  OK ] %s (active=%s, pin=%s, 指定事务状态与受管健康服务已核验，无残留)\n' \
        "$desc" "$installed_version" "$pin_version"
    exit 0
)

run_crash_test "after_prepare" "断点 1/7: 暂存复制完成后 SIGKILL" "1.0.0" || FAIL=1
run_crash_test "after_rename_backup" "断点 2/7: 原目录改名后 SIGKILL" "1.0.0" || FAIL=1
run_crash_test "after_activate" "断点 3/7: 新目录激活后 SIGKILL" "1.0.0" || FAIL=1
run_crash_test "after_health_checked" "断点 4/7: 健康检查后、版本钉写入前 SIGKILL" "1.0.0" || FAIL=1
run_crash_test "after_pin_written" "断点 5/7: 版本钉写入后、备份清理前 SIGKILL" "2.0.0" || FAIL=1
run_crash_test "after_commit" "断点 6/7: 提交完成后 SIGKILL" "2.0.0" || FAIL=1
run_crash_test "after_spawn_before_commit" "断点 7/7: 内核已拉起、未提交时 SIGKILL" "1.0.0" || FAIL=1

printf '%s\n' '======================================================================'
if [ "$FAIL" -eq 0 ]; then
    printf '%s\n' 'ALL 7 CRASH INJECTION TESTS PASSED'
    exit 0
fi
printf '%s\n' 'FAILED CRASH INJECTION TESTS'
if [ -n "$DIAG_BASE" ]; then
    printf 'failure diagnostics: %s\n' "$DIAG_BASE" >&2
fi
exit 1
