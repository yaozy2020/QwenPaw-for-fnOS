#!/bin/bash
# 原生配置回调集成测试：真实 release supervisor + qwenpaw-test-runtime，仅操作临时 TRIM 根。
# 不启动假 HTTP 服务，不调用 curl/Python；失败显式累计，不能吞掉前置错误。

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SUPERVISOR_RELEASE="$ROOT/supervisor/target/release/qwenpaw-supervisor"
TEST_RUNTIME_RELEASE="$ROOT/supervisor/target/release/qwenpaw-test-runtime"

missing=0
for binary in "$SUPERVISOR_RELEASE" "$TEST_RUNTIME_RELEASE"; do
    if [ ! -x "$binary" ]; then
        printf 'FAIL: 缺少可执行 release 产物: %s\n' "$binary"
        missing=1
    fi
done
for source in config_callback config_init upgrade_init main; do
    if [ ! -f "$ROOT/cmd/$source" ]; then
        printf 'FAIL: 缺少平台入口: %s\n' "$ROOT/cmd/$source"
        missing=1
    fi
done
for command_name in sha256sum awk stat readlink flock; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
        printf 'FAIL: 测试所需命令不可用: %s\n' "$command_name"
        missing=1
    fi
done
[ "$missing" -eq 0 ] || exit 1

SUPERVISOR_HASH_LINE="$(sha256sum -- "$SUPERVISOR_RELEASE")" || exit 1
RUNTIME_HASH_LINE="$(sha256sum -- "$TEST_RUNTIME_RELEASE")" || exit 1
read -r SUPERVISOR_SHA256 _ <<< "$SUPERVISOR_HASH_LINE"
read -r TEST_RUNTIME_SHA256 _ <<< "$RUNTIME_HASH_LINE"
printf 'release supervisor: %s\nrelease supervisor SHA256: %s\n' "$SUPERVISOR_RELEASE" "$SUPERVISOR_SHA256"
printf 'test-runtime: %s\ntest-runtime SHA256: %s\n' "$TEST_RUNTIME_RELEASE" "$TEST_RUNTIME_SHA256"

T="$(mktemp -d "${TMPDIR:-/tmp}/qwenpaw-config-callback.XXXXXX")" || {
    echo 'FAIL: 无法创建隔离临时目录'
    exit 1
}
PKG="$T/pkg"
VAR="$T/var"
PKGHOME="$T/home"
PKGETC="$T/etc"
SHARE_ROOT="$T/share"
VENV="$PKGHOME/venv"
PYTHON_BIN="$VENV/bin/python3"
UI_CONFIG="$PKG/ui/config"
SUP_BIN="$PKG/bin/qwenpaw-supervisor"
CALLBACK="$PKG/cmd/config_callback"
SOCKET="$VAR/supervisor-control.sock"
TEMP_LOGFILE="$T/trim-temp.log"
FORBIDDEN_TOOLS="$T/forbidden-tools.log"
DENY_BIN="$T/deny-bin"

fail=0
cleaned=0
keep_fixture=0
SUP_PID=""
SUP_STARTTIME=""
SUP_EXE=""
SENTINEL_PID=""
SENTINEL_STARTTIME=""
SENTINEL_EXE=""
CURRENT_KERNEL_PID=""
CURRENT_KERNEL_STARTTIME=""

ok() { printf '  [  OK ] %s\n' "$1"; }
bad() { printf '  [FAIL] %s\n' "$1"; fail=1; }
show_output() {
    if [ -s "$1" ]; then
        while IFS= read -r line; do printf '         %s\n' "$line"; done < "$1"
    fi
}

process_starttime() {
    local pid="$1" raw rest
    local -a fields
    [ -r "/proc/$pid/stat" ] || return 1
    raw="$(<"/proc/$pid/stat")" || return 1
    rest="${raw##*) }"
    read -r -a fields <<< "$rest"
    [ "${#fields[@]}" -ge 20 ] || return 1
    [[ "${fields[19]}" =~ ^[0-9]+$ ]] || return 1
    printf '%s\n' "${fields[19]}"
}

process_identity_matches() {
    local pid="$1" expected_starttime="$2" expected_exe="$3"
    local raw rest actual_exe
    local -a fields
    [ -r "/proc/$pid/stat" ] || return 1
    raw="$(<"/proc/$pid/stat")" || return 1
    rest="${raw##*) }"
    read -r -a fields <<< "$rest"
    [ "${#fields[@]}" -ge 20 ] || return 1
    [ "${fields[19]}" = "$expected_starttime" ] || return 1
    [[ "${fields[0]}" != Z && "${fields[0]}" != X ]] || return 1
    actual_exe="$(readlink "/proc/$pid/exe")" || return 1
    actual_exe="${actual_exe% (deleted)}"
    [ "$actual_exe" = "$expected_exe" ]
}

capture_supervisor_identity() {
    local recorded_pid recorded_starttime
    [ -r "$VAR/supervisor.pid" ] && [ -r "$VAR/supervisor.pid.starttime" ] || return 1
    recorded_pid="$(tr -d '[:space:]' < "$VAR/supervisor.pid")" || return 1
    recorded_starttime="$(tr -d '[:space:]' < "$VAR/supervisor.pid.starttime")" || return 1
    [[ "$recorded_pid" =~ ^[1-9][0-9]*$ && "$recorded_starttime" =~ ^[0-9]+$ ]] || return 1
    SUP_EXE="$(readlink -f -- "$SUP_BIN")" || return 1
    process_identity_matches "$recorded_pid" "$recorded_starttime" "$SUP_EXE" || return 1
    SUP_PID="$recorded_pid"
    SUP_STARTTIME="$recorded_starttime"
}

capture_sentinel_identity() {
    SENTINEL_EXE="$(readlink -f -- "$(command -v sleep)")" || return 1
    SENTINEL_STARTTIME="$(process_starttime "$SENTINEL_PID")" || return 1
    process_identity_matches "$SENTINEL_PID" "$SENTINEL_STARTTIME" "$SENTINEL_EXE"
}

config_snapshot() {
    local path metadata digest
    for path in \
        "$VAR/service_port" \
        "$VAR/qwenpaw-log-level" \
        "$VAR/pip-mirror" \
        "$VAR/settings-transaction.json" \
        "$UI_CONFIG"; do
        if [ -e "$path" ]; then
            metadata="$(stat -c '%d:%i:%a:%s:%y' -- "$path")" || return 1
            digest="$(sha256sum -- "$path")" || return 1
            printf '%s|%s|%s\n' "$path" "$metadata" "${digest%% *}"
        else
            printf '%s|MISSING\n' "$path"
        fi
    done
}

no_service_artifacts() {
    local path
    for path in \
        "$VAR/config-restart.lock" \
        "$VAR/supervisor.lock" \
        "$VAR/supervisor.pid" \
        "$VAR/supervisor.pid.starttime" \
        "$VAR/supervisor.log" \
        "$VAR/kernel.pid" \
        "$SOCKET"; do
        if [ -e "$path" ] || [ -L "$path" ]; then return 1; fi
    done
    return 0
}

port_in_use() {
    local port="$1" hex
    printf -v hex '%04X' "$port"
    awk -v wanted="$hex" '
        FNR == 1 { next }
        {
            split($2, endpoint, ":")
            if (toupper(endpoint[2]) == wanted) found = 1
        }
        END { exit found ? 0 : 1 }
    ' /proc/net/tcp /proc/net/tcp6 2>/dev/null
}

free_port() {
    local excluded candidate blocked attempt
    for ((attempt = 0; attempt < 512; attempt++)); do
        candidate=$((20000 + RANDOM % 40000))
        [ "$candidate" -ne 19090 ] && [ "$candidate" -ne 19091 ] || continue
        blocked=0
        for excluded in "$@"; do
            [ "$candidate" -ne "$excluded" ] || blocked=1
        done
        [ "$blocked" -eq 0 ] || continue
        if ! port_in_use "$candidate"; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

wait_for_runtime() {
    local expected_port="$1" expected_level="$2" previous_pid="${3:-}" iteration
    local pid starttime kernel_pid
    for ((iteration = 0; iteration < 200; iteration++)); do
        if [ -r "$VENV/.qwenpaw-test-runtime.pid" ] && [ -r "$VAR/kernel.pid" ]; then
            read -r pid starttime < "$VENV/.qwenpaw-test-runtime.pid"
            kernel_pid="$(tr -d '[:space:]' < "$VAR/kernel.pid")"
            if [[ "$pid" =~ ^[1-9][0-9]*$ && "$starttime" =~ ^[0-9]+$ ]] \
                && [ "$kernel_pid" = "$pid" ] \
                && [ "$pid" != "$previous_pid" ] \
                && process_identity_matches "$pid" "$starttime" "$(readlink -f -- "$PYTHON_BIN")" \
                && tr '\0' '\n' < "/proc/$pid/cmdline" > "$T/runtime.argv" \
                && awk -v wanted_port="$expected_port" -v wanted_level="$expected_level" '
                    previous == "--port" && $0 == wanted_port { port_ok = 1 }
                    previous == "--log-level" && $0 == wanted_level { level_ok = 1 }
                    { previous = $0 }
                    END { exit !(port_ok && level_ok) }
                ' "$T/runtime.argv"; then
                CURRENT_KERNEL_PID="$pid"
                CURRENT_KERNEL_STARTTIME="$starttime"
                return 0
            fi
        fi
        sleep 0.05
    done
    return 1
}

assert_desktop_port_absent() {
    awk '
        /"qwenpaw\.main"[[:space:]]*:/ { in_main = 1; next }
        /"qwenpaw\.console"[[:space:]]*:/ { in_main = 0 }
        in_main && index($0, "\"port\":") { found = 1 }
        END { exit found ? 1 : 0 }
    ' "$UI_CONFIG"
}

assert_desktop_sentinels() {
    grep -Fq '"root-sentinel": "preserve-root"' "$UI_CONFIG" \
        && grep -Fq '"entry-sentinel": "preserve-main"' "$UI_CONFIG" \
        && grep -Fq '"console-sentinel": "preserve-console"' "$UI_CONFIG" \
        && grep -Fq '"type": "iframe"' "$UI_CONFIG" \
        && grep -Fq '"url": "/app/qwenpaw/"' "$UI_CONFIG" \
        && grep -Fq '"gatewayPrefix": "/app/qwenpaw"' "$UI_CONFIG" \
        && grep -Fq '"gatewaySocket": "app.sock"' "$UI_CONFIG" \
        && grep -Fq '"allUsers": false' "$UI_CONFIG" \
        && grep -Fq '"accessPerm": "editable"' "$UI_CONFIG" \
        && awk '
            index($0, "\"qwenpaw.main\":") { in_main = 1; next }
            index($0, "\"qwenpaw.console\":") { in_main = 0 }
            in_main && index($0, "\"url\": \"/app/qwenpaw/web/\"") { web = 1 }
            in_main && index($0, "\"gatewayPrefix\": \"/app/qwenpaw\"") { prefix = 1 }
            in_main && index($0, "\"gatewaySocket\": \"app.sock\"") { socket = 1 }
            in_main && index($0, "\"noDisplay\": false") { visible = 1 }
            END { exit !(web && prefix && socket && visible) }
        ' "$UI_CONFIG"
}

run_env() { env "${BASE_ENV[@]}" "$@"; }
run_main() { run_env bash "$PKG/cmd/main" "$@"; }
run_native_service() { run_env "$SUP_BIN" service "$@"; }
run_callback() {
    local logfile="$1"
    shift
    run_env "TRIM_TEMP_LOGFILE=$logfile" "$@" bash "$CALLBACK"
}

check_forbidden_tools() {
    if [ -s "$FORBIDDEN_TOOLS" ]; then
        bad "回调调用了被禁止的 curl/Python 链: $(tr '\n' ' ' < "$FORBIDDEN_TOOLS")"
        return 1
    fi
    return 0
}

check_error_log() {
    local label="$1"
    if [ -s "$TEMP_LOGFILE" ] && ! grep -Fq '0123456789abcdef0123456789abcdef' "$TEMP_LOGFILE"; then
        ok "$label: 错误写入 TRIM_TEMP_LOGFILE，未泄漏测试令牌"
    else
        bad "$label: TRIM_TEMP_LOGFILE 缺少错误或包含令牌"
    fi
}

service_lock_is_held() {
    local rc=0
    flock -n "$VAR/supervisor.lock" -c true > "$T/lock-probe.out" 2>&1 || rc=$?
    [ "$rc" -eq 1 ]
}

wait_for_supervisor_exit() {
    local iteration
    for ((iteration = 0; iteration < 100; iteration++)); do
        if ! process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then return 0; fi
        sleep 0.05
    done
    return 1
}

stop_supervisor_safely() {
    local rc=0 iteration
    [ -n "$SUP_PID" ] || return 0
    if process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
        run_native_service stop > "$T/service-stop.out" 2>&1 || rc=$?
        if process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
            if [ "$rc" -ne 0 ]; then
                if process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
                    kill -TERM "$SUP_PID" 2>/dev/null || true
                fi
            fi
            for ((iteration = 0; iteration < 100; iteration++)); do
                process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE" || break
                sleep 0.05
            done
        fi
        if process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
            if process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
                kill -KILL "$SUP_PID" 2>/dev/null || true
            fi
            for ((iteration = 0; iteration < 40; iteration++)); do
                process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE" || break
                sleep 0.05
            done
        fi
    fi
    if process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
        bad "无法停止已核验身份的隔离监督者 pid=$SUP_PID"
        keep_fixture=1
        return 1
    fi
    if [ -e "$VAR/supervisor.pid" ] && [ -n "$SUP_PID" ]; then
        local marker_pid
        marker_pid="$(tr -d '[:space:]' < "$VAR/supervisor.pid" 2>/dev/null)"
        if [ "$marker_pid" = "$SUP_PID" ]; then
            bad '监督者退出后仍留有其 PID 登记'
        fi
    fi
    return 0
}

cleanup_fixture() {
    [ "$cleaned" -eq 0 ] || return
    cleaned=1
    stop_supervisor_safely || keep_fixture=1

    if [ -n "$SENTINEL_PID" ]; then
        if process_identity_matches "$SENTINEL_PID" "$SENTINEL_STARTTIME" "$SENTINEL_EXE"; then
            ok '无关哨兵在监督者停止后仍存活且身份未变'
            kill -TERM "$SENTINEL_PID" 2>/dev/null || true
            wait "$SENTINEL_PID" 2>/dev/null || true
        else
            bad '无关哨兵未能存活到监督者停止之后，或其进程身份发生变化'
            if kill -0 "$SENTINEL_PID" 2>/dev/null; then keep_fixture=1; fi
        fi
    fi

    if [ "$keep_fixture" -eq 0 ]; then
        rm -rf -- "$T" || bad "清理临时根失败: $T"
    else
        printf '  [WARN ] 保留临时根以避免影响未核验进程: %s\n' "$T"
    fi
}
trap cleanup_fixture EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if ! mkdir -p "$PKG/bin" "$PKG/cmd" "$PKG/ui" "$VAR" "$VENV/bin" "$PKGETC" "$SHARE_ROOT" "$DENY_BIN"; then
    bad '无法建立临时 TRIM 目录'
    exit 1
fi
if ! cp -- "$SUPERVISOR_RELEASE" "$SUP_BIN" \
    || ! cp -- "$TEST_RUNTIME_RELEASE" "$PYTHON_BIN" \
    || ! cp -- "$ROOT/cmd/config_callback" "$CALLBACK" \
    || ! cp -- "$ROOT/cmd/config_init" "$PKG/cmd/config_init" \
    || ! cp -- "$ROOT/cmd/upgrade_init" "$PKG/cmd/upgrade_init" \
    || ! cp -- "$ROOT/cmd/main" "$PKG/cmd/main"; then
    bad '复制 release 二进制或平台入口失败'
    exit 1
fi
if ! chmod 0755 "$SUP_BIN" "$PYTHON_BIN" "$CALLBACK" "$PKG/cmd/config_init" "$PKG/cmd/upgrade_init" "$PKG/cmd/main"; then
    bad '设置临时二进制/入口执行位失败'
    exit 1
fi
COPIED_SUPERVISOR_LINE="$(sha256sum -- "$SUP_BIN")" || exit 1
COPIED_RUNTIME_LINE="$(sha256sum -- "$PYTHON_BIN")" || exit 1
read -r COPIED_SUPERVISOR_SHA256 _ <<< "$COPIED_SUPERVISOR_LINE"
read -r COPIED_RUNTIME_SHA256 _ <<< "$COPIED_RUNTIME_LINE"
if [ "$COPIED_SUPERVISOR_SHA256" != "$SUPERVISOR_SHA256" ] \
    || [ "$COPIED_RUNTIME_SHA256" != "$TEST_RUNTIME_SHA256" ]; then
    bad '临时包内二进制 SHA256 与 release 来源不一致'
    exit 1
fi
printf 'fixture supervisor SHA256: %s\nfixture test-runtime SHA256: %s\n' \
    "$COPIED_SUPERVISOR_SHA256" "$COPIED_RUNTIME_SHA256"

SUP_PORT="$(free_port)" || { bad '无法分配动态监督者端口'; exit 1; }
INITIAL_PORT="$(free_port "$SUP_PORT")" || { bad '无法分配动态初始内核端口'; exit 1; }
BATCH_PORT="$(free_port "$SUP_PORT" "$INITIAL_PORT")" || { bad '无法分配动态批量配置端口'; exit 1; }
PORT_ONLY_PORT="$(free_port "$SUP_PORT" "$INITIAL_PORT" "$BATCH_PORT")" || { bad '无法分配动态单字段端口'; exit 1; }
printf 'isolated supervisor port: %s\ninitial kernel port: %s\nbatch kernel port: %s\nsingle-field kernel port: %s\n' \
    "$SUP_PORT" "$INITIAL_PORT" "$BATCH_PORT" "$PORT_ONLY_PORT"

TOKEN='0123456789abcdef0123456789abcdef'
printf '%s\n' "$INITIAL_PORT" > "$VAR/service_port"
printf 'trace\n' > "$VAR/qwenpaw-log-level"
printf 'https://mirrors.aliyun.com/pypi/simple/\n' > "$VAR/pip-mirror"
printf '%s\n' "$TOKEN" > "$VAR/supervisor-token"
chmod 0600 "$VAR/supervisor-token"
printf '2.1.0-test\n' > "$VENV/installed"
cat > "$UI_CONFIG" <<EOF
{
  ".url": {
    "qwenpaw.main": {
      "title": "QwenPaw",
      "port": "$INITIAL_PORT",
      "url": "/app/qwenpaw/web/",
      "gatewayPrefix": "/app/qwenpaw",
      "gatewaySocket": "app.sock",
      "noDisplay": false,
      "entry-sentinel": "preserve-main"
    },
    "qwenpaw.console": {
      "type": "iframe",
      "url": "/app/qwenpaw/",
      "gatewayPrefix": "/app/qwenpaw",
      "gatewaySocket": "app.sock",
      "allUsers": false,
      "control": { "accessPerm": "editable" },
      "port": "15666",
      "console-sentinel": "preserve-console"
    }
  },
  "root-sentinel": "preserve-root"
}
EOF

for forbidden in curl python python3; do
    cat > "$DENY_BIN/$forbidden" <<'EOF'
#!/bin/sh
printf '%s\n' "${0##*/}" >> "${QWENPAW_FORBIDDEN_TOOLS:?}"
exit 127
EOF
    chmod 0755 "$DENY_BIN/$forbidden" || { bad "无法建立 $forbidden 拦截器"; exit 1; }
done
: > "$FORBIDDEN_TOOLS"
: > "$TEMP_LOGFILE"

BASE_ENV=(
    "TRIM_APPDEST=$PKG"
    "TRIM_PKGVAR=$VAR"
    "TRIM_PKGHOME=$PKGHOME"
    "TRIM_PKGETC=$PKGETC"
    "TRIM_DATA_SHARE_PATHS=$SHARE_ROOT"
    "TRIM_SERVICE_PORT=$INITIAL_PORT"
    "SUPERVISOR_PORT=$SUP_PORT"
    "SUPERVISOR_AUTOSTART=0"
    "QWENPAW_WEB_PROXY_PORT=off"
    "QWENPAW_PYTHON_BIN=$PYTHON_BIN"
    "QWENPAW_TEST_CONTROL=$T"
    "QWENPAW_FORBIDDEN_TOOLS=$FORBIDDEN_TOOLS"
    "QWENPAW_SUP_URL="
    "QWENPAW_CRASH_POINT="
    "QWENPAW_ENABLE_CRASH_POINT="
    "PATH=$DENY_BIN:$PATH"
)

sleep 300 &
SENTINEL_PID=$!
for ((iteration = 0; iteration < 50; iteration++)); do
    if capture_sentinel_identity; then break; fi
    sleep 0.02
done
if ! capture_sentinel_identity; then
    bad '无法登记并核验无关哨兵进程身份'
    exit 1
fi
ok "已登记无关哨兵 pid=$SENTINEL_PID 的 starttime/executable"

# 只读初始化回显必须经过固定原生 CLI，且不得改动任何配置字节或元数据。
before="$(config_snapshot)" || { bad '无法读取初始化前配置快照'; exit 1; }
rc=0
run_env bash "$PKG/cmd/config_init" > "$T/config-init.out" 2>&1 || rc=$?
if [ "$rc" -eq 0 ] \
    && grep -Fxq "wizard_port=$INITIAL_PORT" "$T/config-init.out" \
    && grep -Fxq "service_port=$INITIAL_PORT" "$T/config-init.out" \
    && grep -Fxq 'wizard_log_level=debug' "$T/config-init.out"; then
    ok 'config-init 原生回显动态端口与 trace→debug'
else
    bad "config-init 原生回显/退出异常: rc=$rc"
    show_output "$T/config-init.out"
fi
after="$(config_snapshot)" || after='snapshot-error'
if [ "$before" = "$after" ] && no_service_artifacts; then
    ok 'config-init 只读：配置快照不变且未启动服务'
else
    bad 'config-init 改写了配置或创建了服务状态'
fi
check_forbidden_tools

before="$(config_snapshot)" || { bad '无法读取升级初始化前配置快照'; exit 1; }
rc=0
run_env bash "$PKG/cmd/upgrade_init" > "$T/upgrade-init.out" 2>&1 || rc=$?
if [ "$rc" -eq 0 ] && grep -Fxq 'wizard_pypi_mirror=aliyun' "$T/upgrade-init.out"; then
    ok 'upgrade-init 原生回显已配置的阿里云镜像'
else
    bad "upgrade-init 原生回显/退出异常: rc=$rc"
    show_output "$T/upgrade-init.out"
fi
after="$(config_snapshot)" || after='snapshot-error'
if [ "$before" = "$after" ] && no_service_artifacts; then
    ok 'upgrade-init 只读：配置快照不变且未启动服务'
else
    bad 'upgrade-init 改写了配置或创建了服务状态'
fi
check_forbidden_tools

# 空字段及非法字段都在服务尚未启动时运行；配置文件零写入、服务零启动。
before="$(config_snapshot)" || { bad '无法读取空回调前配置快照'; exit 1; }
: > "$TEMP_LOGFILE"
: > "$FORBIDDEN_TOOLS"
rc=0
run_callback "$TEMP_LOGFILE" wizard_port= wizard_log_level= > "$T/callback-empty.out" 2>&1 || rc=$?
after="$(config_snapshot)" || after='snapshot-error'
if [ "$rc" -eq 0 ] && [ "$before" = "$after" ] && no_service_artifacts; then
    ok '空字段回调成功且配置零写入、服务零启动'
else
    bad "空字段回调异常: rc=$rc 或配置/服务状态发生变化"
    show_output "$T/callback-empty.out"
fi
check_forbidden_tools

for invalid in port-low port-reserved-supervisor port-reserved-platform port-high log-level; do
    case "$invalid" in
        port-low) port_value=80; level_value= ;;
        port-reserved-supervisor) port_value="$SUP_PORT"; level_value= ;;
        port-reserved-platform) port_value=19091; level_value= ;;
        port-high) port_value=65536; level_value= ;;
        log-level) port_value=; level_value=verbose ;;
    esac
    before="$(config_snapshot)" || { bad "无法读取 $invalid 前配置快照"; break; }
    : > "$TEMP_LOGFILE"
    : > "$FORBIDDEN_TOOLS"
    rc=0
    run_callback "$TEMP_LOGFILE" "wizard_port=$port_value" "wizard_log_level=$level_value" \
        > "$T/callback-$invalid.out" 2>&1 || rc=$?
    after="$(config_snapshot)" || after='snapshot-error'
    if [ "$rc" -ne 0 ] && [ "$before" = "$after" ] && no_service_artifacts; then
        ok "非法输入 $invalid 失败且配置零写入、服务零启动"
    else
        bad "非法输入 $invalid 未满足失败/零写入/零启动: rc=$rc"
        show_output "$T/callback-$invalid.out"
    fi
    check_error_log "非法输入 $invalid"
    check_forbidden_tools
done

# 真实平台 service 以 release supervisor 启动；测试内核由 release test-runtime 提供。
rc=0
run_main start > "$T/service-start.out" 2>&1 || rc=$?
if [ "$rc" -eq 0 ] && capture_supervisor_identity; then
    ok "真实 release supervisor 已启动且 PID/starttime/executable 身份核验通过 (pid=$SUP_PID)"
else
    bad "隔离监督者启动或身份核验失败: rc=$rc"
    show_output "$T/service-start.out"
fi

service_ready=0
if [ -n "$SUP_PID" ] && process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
    service_ready=1
    if service_lock_is_held; then
        ok '真实服务持有 supervisor.lock 独占锁'
    else
        bad '服务已登记但 supervisor.lock 未被持有'
    fi

    if [ -S "$SOCKET" ]; then
        socket_mode="$(stat -c '%a' -- "$SOCKET")"
        socket_uid="$(stat -c '%u' -- "$SOCKET")"
        if [ "$socket_mode" = 600 ] && [ "$socket_uid" = "$(id -u)" ]; then
            ok 'private control UDS 为当前测试用户所有且权限 0600'
        else
            bad "private control UDS 权限/属主异常: mode=$socket_mode uid=$socket_uid"
        fi
        uds_ready=1
    else
        bad "真实监督者未创建固定私有 UDS: $SOCKET"
        uds_ready=0
    fi
else
    service_ready=0
    uds_ready=0
fi

if [ "$service_ready" -eq 1 ] && [ "$uds_ready" -eq 1 ]; then
    : > "$FORBIDDEN_TOOLS"
    : > "$TEMP_LOGFILE"
    rc=0
    run_callback "$TEMP_LOGFILE" "wizard_port=$BATCH_PORT" wizard_log_level=warning \
        > "$T/callback-batch.out" 2>&1 || rc=$?
    if [ "$rc" -eq 0 ] \
        && [ "$(<"$VAR/service_port")" = "$BATCH_PORT" ] \
        && [ "$(<"$VAR/qwenpaw-log-level")" = warning ] \
        && assert_desktop_port_absent "$BATCH_PORT" \
        && assert_desktop_sentinels \
        && process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
        ok '薄 config_callback 经真实 UDS 批量应用端口/日志，并同步桌面配置'
    else
        bad "真实 UDS 批量配置失败或落盘/桌面同步不一致: rc=$rc"
        show_output "$T/callback-batch.out"
    fi
    check_forbidden_tools
    if wait_for_runtime "$BATCH_PORT" warning; then
        ok "批量设置由真实 test-runtime 在新端口提供健康服务 (pid=$CURRENT_KERNEL_PID)"
    else
        bad '批量设置未由 test-runtime 在新端口/日志级别下运行'
    fi
    batch_runtime_pid="$CURRENT_KERNEL_PID"

    : > "$FORBIDDEN_TOOLS"
    : > "$TEMP_LOGFILE"
    rc=0
    run_callback "$TEMP_LOGFILE" "wizard_port=$PORT_ONLY_PORT" wizard_log_level= \
        > "$T/callback-port-only.out" 2>&1 || rc=$?
    if [ "$rc" -eq 0 ] \
        && [ "$(<"$VAR/service_port")" = "$PORT_ONLY_PORT" ] \
        && [ "$(<"$VAR/qwenpaw-log-level")" = warning ] \
        && assert_desktop_port_absent "$PORT_ONLY_PORT" \
        && assert_desktop_sentinels \
        && process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
        ok '薄 config_callback 单字段端口经真实 UDS 更新，未覆盖日志级别/桌面其他字段'
    else
        bad "真实 UDS 单字段端口配置失败: rc=$rc"
        show_output "$T/callback-port-only.out"
    fi
    check_forbidden_tools
    if wait_for_runtime "$PORT_ONLY_PORT" warning "$batch_runtime_pid"; then
        ok "单字段端口变更后 test-runtime 在新端口重启 (pid=$CURRENT_KERNEL_PID)"
    else
        bad '单字段端口变更未产生预期的新 test-runtime 进程'
    fi
    port_runtime_pid="$CURRENT_KERNEL_PID"

    : > "$FORBIDDEN_TOOLS"
    : > "$TEMP_LOGFILE"
    rc=0
    run_callback "$TEMP_LOGFILE" wizard_port= wizard_log_level=debug \
        > "$T/callback-log-only.out" 2>&1 || rc=$?
    if [ "$rc" -eq 0 ] \
        && [ "$(<"$VAR/service_port")" = "$PORT_ONLY_PORT" ] \
        && [ "$(<"$VAR/qwenpaw-log-level")" = debug ] \
        && assert_desktop_port_absent "$PORT_ONLY_PORT" \
        && assert_desktop_sentinels \
        && process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE"; then
        ok '薄 config_callback 单字段日志级别经真实 UDS 更新，端口与桌面其他字段保持'
    else
        bad "真实 UDS 单字段日志配置失败: rc=$rc"
        show_output "$T/callback-log-only.out"
    fi
    check_forbidden_tools
    if wait_for_runtime "$PORT_ONLY_PORT" debug "$port_runtime_pid"; then
        ok "单字段日志变更后 test-runtime 收到 debug argv 并重新健康 (pid=$CURRENT_KERNEL_PID)"
    else
        bad '单字段日志变更未由新 test-runtime 进程以 debug 运行'
    fi

    # Socket 路径故意不可达，但服务 PID/starttime/executable 与独占锁仍保持。
    before="$(config_snapshot)" || { bad '无法读取 UDS 不可达测试前配置快照'; before='snapshot-error'; }
    if ! mv -- "$SOCKET" "$T/supervisor-control.sock.hidden"; then
        bad '无法隔离真实 UDS 路径，不能执行不可达场景'
    elif [ ! -S "$SOCKET" ] \
        && [ -S "$T/supervisor-control.sock.hidden" ] \
        && process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE" \
        && service_lock_is_held; then
        : > "$TEMP_LOGFILE"
        : > "$FORBIDDEN_TOOLS"
        rc=0
        run_callback "$TEMP_LOGFILE" "wizard_port=$BATCH_PORT" wizard_log_level=error \
            > "$T/callback-uds-unreachable.out" 2>&1 || rc=$?
        after="$(config_snapshot)" || after='snapshot-error'
        if [ "$rc" -ne 0 ] \
            && [ "$before" = "$after" ] \
            && [ ! -e "$SOCKET" ] \
            && process_identity_matches "$SUP_PID" "$SUP_STARTTIME" "$SUP_EXE" \
            && service_lock_is_held; then
            ok '服务持锁但 UDS 不可达时失败：配置零双写且服务未被替换/重启'
        else
            bad "UDS 不可达时未 fail-closed: rc=$rc，可能发生离线双写或服务替换"
            show_output "$T/callback-uds-unreachable.out"
        fi
        check_error_log 'UDS 不可达失败'
        check_forbidden_tools
    else
        bad 'UDS 路径隔离后服务身份/所有权锁未保持'
    fi
else
    echo '  [INFO ] UDS 在线配置用例因真实私有 UDS 缺失而未执行；前置 FAIL 会阻止全绿'
fi

if [ -n "$SENTINEL_PID" ] && process_identity_matches "$SENTINEL_PID" "$SENTINEL_STARTTIME" "$SENTINEL_EXE"; then
    ok '全部配置回调期间无关哨兵进程保持存活'
else
    bad '配置回调影响了无关哨兵进程或其身份发生变化'
fi

cleanup_fixture
trap - EXIT INT TERM
printf '=========================================\n'
if [ "$fail" -eq 0 ]; then
    echo 'test-config-callback: ALL CHECKS PASSED'
    exit 0
fi
echo 'test-config-callback: FAIL'
exit 1
