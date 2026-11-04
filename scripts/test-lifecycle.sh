#!/bin/bash
# test-lifecycle.sh — 原生生命周期回归与多进程竞争覆盖。
# 可独立运行：bash scripts/test-lifecycle.sh；lint.sh 最后一层自动调用。
# 全部数据和进程均隔离在临时环境；使用真实 release 监督者与专用 runtime。
# 注意：不用 set -e（工程铁律），失败显式计数并 exit 1。

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

proc_alive_state() {
    local stat rest
    stat="$(cat "/proc/${1}/stat" 2>/dev/null)" || return 1
    rest="${stat##*) }"
    case "${rest%% *}" in
        R|S|D|T) return 0 ;;
        *) return 1 ;;
    esac
}

proc_start_ticks() {
    local stat rest
    local -a fields
    stat="$(cat "/proc/${1}/stat" 2>/dev/null)" || return 1
    rest="${stat##*) }"
    read -r -a fields <<< "$rest"
    [ "${#fields[@]}" -ge 20 ] || return 1
    [[ "${fields[19]}" =~ ^[0-9]+$ ]] || return 1
    printf '%s\n' "${fields[19]}"
}

T="$(mktemp -d)"
# EXIT 清理只对本测试记录并复核身份的进程发送信号。
life_pids=()
life_starttimes=()
life_runtime_pids=()
life_runtime_starttimes=()
life_cli_pids=()
life_cli_starttimes=()
life_cli_exeids=()
L_DEST=""
L_VAR=""
L_BIN_ID=""
L_RUNTIME_ID=""
L_TIMEOUT_ID=""
SENTINEL_PID=""
SENTINEL_STARTTIME=""
SENTINEL_EXE_ID=""
cleanup() {
    local status=$? i p attempt
    trap - EXIT INT TERM

    for i in "${!life_cli_pids[@]}"; do
        p="${life_cli_pids[$i]}"
        if declare -F life_cli_identity_matches >/dev/null \
            && life_cli_identity_matches "$p" "${life_cli_starttimes[$i]}" "${life_cli_exeids[$i]}"; then
            kill -TERM "$p" 2>/dev/null || true
        fi
    done
    for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
        local cli_remaining=0
        for i in "${!life_cli_pids[@]}"; do
            p="${life_cli_pids[$i]}"
            if declare -F life_cli_identity_matches >/dev/null \
                && life_cli_identity_matches "$p" "${life_cli_starttimes[$i]}" "${life_cli_exeids[$i]}"; then
                cli_remaining=1
            fi
        done
        [ "$cli_remaining" -eq 0 ] && break
        sleep 0.1
    done
    for i in "${!life_cli_pids[@]}"; do
        p="${life_cli_pids[$i]}"
        if declare -F life_cli_identity_matches >/dev/null \
            && life_cli_identity_matches "$p" "${life_cli_starttimes[$i]}" "${life_cli_exeids[$i]}"; then
            kill -KILL "$p" 2>/dev/null || true
        fi
        wait "$p" 2>/dev/null || true
    done

    if [ -n "$L_VAR" ] && [ "${L_ENV_READY:-0}" -eq 1 ] \
        && declare -F life_run >/dev/null; then
        declare -F life_collect_owned_processes >/dev/null && life_collect_owned_processes || true
        life_run 30 stop >/dev/null 2>&1 || true
        declare -F life_collect_owned_processes >/dev/null && life_collect_owned_processes || true
    fi

    for i in "${!life_pids[@]}"; do
        p="${life_pids[$i]}"
        if declare -F life_supervisor_identity_matches >/dev/null \
            && life_supervisor_identity_matches "$p" "${life_starttimes[$i]}"; then
            kill -TERM "$p" 2>/dev/null || true
        fi
    done
    for i in "${!life_runtime_pids[@]}"; do
        p="${life_runtime_pids[$i]}"
        if declare -F life_runtime_identity_matches >/dev/null \
            && life_runtime_identity_matches "$p" "${life_runtime_starttimes[$i]}"; then
            kill -TERM "$p" 2>/dev/null || true
        fi
    done

    for attempt in 1 2 3 4 5 6 7 8 9 10; do
        local remaining=0
        for i in "${!life_pids[@]}"; do
            p="${life_pids[$i]}"
            if declare -F life_supervisor_identity_matches >/dev/null \
                && life_supervisor_identity_matches "$p" "${life_starttimes[$i]}"; then
                remaining=1
            fi
        done
        for i in "${!life_runtime_pids[@]}"; do
            p="${life_runtime_pids[$i]}"
            if declare -F life_runtime_identity_matches >/dev/null \
                && life_runtime_identity_matches "$p" "${life_runtime_starttimes[$i]}"; then
                remaining=1
            fi
        done
        [ "$remaining" -eq 0 ] && break
        sleep 0.1
    done

    for i in "${!life_pids[@]}"; do
        p="${life_pids[$i]}"
        if declare -F life_supervisor_identity_matches >/dev/null \
            && life_supervisor_identity_matches "$p" "${life_starttimes[$i]}"; then
            kill -KILL "$p" 2>/dev/null || true
        fi
    done
    for i in "${!life_runtime_pids[@]}"; do
        p="${life_runtime_pids[$i]}"
        if declare -F life_runtime_identity_matches >/dev/null \
            && life_runtime_identity_matches "$p" "${life_runtime_starttimes[$i]}"; then
            kill -KILL "$p" 2>/dev/null || true
        fi
    done
    for i in "${!life_cli_pids[@]}"; do
        p="${life_cli_pids[$i]}"
        if declare -F life_cli_identity_matches >/dev/null \
            && life_cli_identity_matches "$p" "${life_cli_starttimes[$i]}" "${life_cli_exeids[$i]}"; then
            kill -KILL "$p" 2>/dev/null || true
        fi
        wait "$p" 2>/dev/null || true
    done

    if [ -n "$SENTINEL_PID" ] && declare -F life_sentinel_identity_matches >/dev/null \
        && life_sentinel_identity_matches "$SENTINEL_PID" "$SENTINEL_STARTTIME" "$SENTINEL_EXE_ID"; then
        kill -TERM "$SENTINEL_PID" 2>/dev/null || true
        wait "$SENTINEL_PID" 2>/dev/null || true
    fi
    exec 3>&- 2>/dev/null || true
    if ! rm -rf -- "$T"; then
        echo "ERROR: 无法清理临时测试目录: $T" >&2
        status=1
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
fail=0

ok() { echo "  [  OK ] $1"; }
bad() { echo "  [FAIL] $1"; fail=1; }

# ── [6] 向导初始化回显 (config_init / upgrade_init) ──
TEST_RELEASE_BIN="${QWENPAW_TEST_SUP_BIN:-$ROOT/supervisor/target/release/qwenpaw-supervisor}"
case "$TEST_RELEASE_BIN" in
    /*) ;;
    *) TEST_RELEASE_BIN="$PWD/$TEST_RELEASE_BIN" ;;
esac
TEST_RELEASE_READY=1
if [ ! -f "$TEST_RELEASE_BIN" ] || [ ! -x "$TEST_RELEASE_BIN" ]; then
    TEST_RELEASE_READY=0
fi

copy_test_release() {
    local target="$1/app/bin/qwenpaw-supervisor"
    [ "$TEST_RELEASE_READY" -eq 1 ] || return 1
    mkdir -p "${target%/*}" \
        && cp -- "$TEST_RELEASE_BIN" "$target" \
        && chmod 755 "$target" \
        && cmp -s -- "$TEST_RELEASE_BIN" "$target"
}

pick_test_ports() {
    python3 - <<'PYPORTS'
import socket
ports = set()
while len(ports) < 2:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    ports.add(sock.getsockname()[1])
    sock.close()
print(*ports)
PYPORTS
}

TEST_PORTS="$(pick_test_ports)"
read -r INIT_SUP_PORT INIT_SERVICE_PORT <<< "$TEST_PORTS"
INIT_READY=1
if [ -z "$INIT_SUP_PORT" ] || [ -z "$INIT_SERVICE_PORT" ] || [ "$INIT_SUP_PORT" = "$INIT_SERVICE_PORT" ]; then
    bad "无法为薄入口测试分配独立动态端口"
    INIT_READY=0
fi
INIT_DEST="${T}/init_appdest"
if [ "$TEST_RELEASE_READY" -eq 1 ] && ! copy_test_release "$INIT_DEST"; then
    bad "无法把当前 release supervisor 安装到 init 临时包第一候选"
    INIT_READY=0
fi
if [ "$TEST_RELEASE_READY" -ne 1 ]; then
    echo "  [SKIP] release supervisor 缺失，拒绝调用薄入口 fallback 候选"
    INIT_READY=0
fi

if [ "$INIT_READY" -eq 1 ]; then
    # 6.1 config_init 默认回显 (无落盘文件时)
    out6a="$(env TRIM_PKGVAR="${T}/cfg_var1" TRIM_APPDEST="$INIT_DEST" \
        SUPERVISOR_PORT="$INIT_SUP_PORT" TRIM_SERVICE_PORT="$INIT_SERVICE_PORT" \
        bash "$ROOT/cmd/config_init")"
    if echo "$out6a" | grep -q "wizard_port=18088" && echo "$out6a" | grep -q "service_port=18088" && echo "$out6a" | grep -q "wizard_log_level=info"; then
        ok "config_init 默认回显正常 (wizard_port/service_port/wizard_log_level)"
    else
        bad "config_init 默认回显异常: $out6a"
    fi

    # 6.2 config_init 自定义落盘值回显 (动态端口与 debug 日志级别)
    mkdir -p "${T}/cfg_var2"
    printf '%s\n' "$INIT_SERVICE_PORT" > "${T}/cfg_var2/service_port"
    echo "debug" > "${T}/cfg_var2/qwenpaw-log-level"
    out6b="$(env TRIM_PKGVAR="${T}/cfg_var2" TRIM_APPDEST="$INIT_DEST" \
        SUPERVISOR_PORT="$INIT_SUP_PORT" TRIM_SERVICE_PORT="$INIT_SERVICE_PORT" \
        bash "$ROOT/cmd/config_init")"
    if echo "$out6b" | grep -q "wizard_port=${INIT_SERVICE_PORT}" \
        && echo "$out6b" | grep -q "service_port=${INIT_SERVICE_PORT}" \
        && echo "$out6b" | grep -q "wizard_log_level=debug"; then
        ok "config_init 动态端口与日志级别回显正常 (避免设置弹窗覆盖重置)"
    else
        bad "config_init 自定义回显异常: $out6b"
    fi

    # 6.3 upgrade_init 镜像源回显
    mkdir -p "${T}/up_var"
    echo "https://pypi.tuna.tsinghua.edu.cn/simple" > "${T}/up_var/pip-mirror"
    out6c="$(env TRIM_PKGVAR="${T}/up_var" TRIM_APPDEST="$INIT_DEST" \
        SUPERVISOR_PORT="$INIT_SUP_PORT" TRIM_SERVICE_PORT="$INIT_SERVICE_PORT" \
        bash "$ROOT/cmd/upgrade_init")"
    if [ "$out6c" = "wizard_pypi_mirror=tsinghua" ]; then
        ok "upgrade_init 已存清华源正常回显"
    else
        bad "upgrade_init 清华源回显异常: $out6c"
    fi
    echo "https://mirrors.aliyun.com/pypi/simple/" > "${T}/up_var/pip-mirror"
    out6d="$(env TRIM_PKGVAR="${T}/up_var" TRIM_APPDEST="$INIT_DEST" \
        SUPERVISOR_PORT="$INIT_SUP_PORT" TRIM_SERVICE_PORT="$INIT_SERVICE_PORT" \
        bash "$ROOT/cmd/upgrade_init")"
    if [ "$out6d" = "wizard_pypi_mirror=aliyun" ]; then
        ok "upgrade_init 已存阿里源正常回显"
    else
        bad "upgrade_init 阿里源回显异常: $out6d"
    fi

    # 6.5 官方源回显(pypi.org 落盘后必须回显为 official,不能回落成默认 aliyun)
    echo "https://pypi.org/simple" > "${T}/up_var/pip-mirror"
    out6e="$(env TRIM_PKGVAR="${T}/up_var" TRIM_APPDEST="$INIT_DEST" \
        SUPERVISOR_PORT="$INIT_SUP_PORT" TRIM_SERVICE_PORT="$INIT_SERVICE_PORT" \
        bash "$ROOT/cmd/upgrade_init")"
    if [ "$out6e" = "wizard_pypi_mirror=official" ]; then
        ok "upgrade_init 官方源正常回显"
    else
        bad "upgrade_init 官方源回显异常(应为 official): $out6e"
    fi
fi

# ── [7] 配置修改回调 (config_callback)：拒绝非法输入且不启动服务 ──
CB_DEST="${T}/cb_dest"
CB_VAR="${T}/cb_var"
CB_PORTS="$(pick_test_ports)"
read -r CB_SUP_PORT CB_SERVICE_PORT <<< "$CB_PORTS"
CB_READY=1
if [ -z "$CB_SUP_PORT" ] || [ -z "$CB_SERVICE_PORT" ] || [ "$CB_SUP_PORT" = "$CB_SERVICE_PORT" ]; then
    bad "无法为 config_callback 临时包分配独立动态端口"
    CB_READY=0
fi
if ! mkdir -p "${CB_DEST}/cmd" "${CB_DEST}/ui" "${CB_VAR}" \
    "${T}/cb_home" "${T}/cb_etc" "${T}/cb_share"; then
    bad "无法创建 config_callback 临时包"
    CB_READY=0
fi
if [ "$TEST_RELEASE_READY" -eq 1 ]; then
    if ! cp -- "$ROOT/cmd/config_callback" "${CB_DEST}/cmd/config_callback" \
        || ! copy_test_release "$CB_DEST"; then
        bad "无法准备包含 release supervisor 第一候选的 config_callback 临时包"
        CB_READY=0
    fi
else
    echo "  [SKIP] release supervisor 缺失，拒绝调用 config_callback fallback 候选"
    CB_READY=0
fi
cat > "${CB_DEST}/ui/config" <<EOF
{
  ".url": {
    "qwenpaw.main": {"title": "QwenPaw", "port": "${CB_SERVICE_PORT}", "url": "/"}
  },
  "sentinel": "keep-me"
}
EOF
printf '%s\n' "$CB_SERVICE_PORT" > "${CB_VAR}/service_port"
printf 'info\n' > "${CB_VAR}/qwenpaw-log-level"

cb_hashes() {
    sha256sum "${CB_VAR}/service_port" "${CB_VAR}/qwenpaw-log-level" \
        "${CB_DEST}/ui/config" 2>/dev/null
}
cb_no_service_artifacts() {
    local path
    for path in supervisor.pid supervisor.pid.starttime supervisor.lock \
        config-restart.lock kernel.pid; do
        [ ! -e "${CB_VAR}/${path}" ] || return 1
    done
}
cb_before="$(cb_hashes)"
cb_run() {
    env TRIM_PKGVAR="${CB_VAR}" TRIM_APPDEST="${CB_DEST}" \
        TRIM_PKGHOME="${T}/cb_home" TRIM_PKGETC="${T}/cb_etc" \
        TRIM_DATA_SHARE_PATHS="${T}/cb_share" SUPERVISOR_AUTOSTART=0 \
        SUPERVISOR_PORT="$CB_SUP_PORT" TRIM_SERVICE_PORT="$CB_SERVICE_PORT" \
        "$@" bash "${CB_DEST}/cmd/config_callback"
}

RESERVED_PORTS="$(python3 - "$ROOT/supervisor/src/platform.rs" <<'PYRESERVED'
import re, sys
source = open(sys.argv[1], encoding="utf-8").read()
match = re.search(r"contains\(&value\).*?value\s*==\s*(\d+).*?value\s*==\s*(\d+)", source)
if not match:
    raise SystemExit("could not locate the platform's reserved port list")
print(*match.groups())
PYRESERVED
)"
read -r -a RESERVED_PORT_LIST <<< "$RESERVED_PORTS"
if [ "${#RESERVED_PORT_LIST[@]}" -ne 2 ]; then
    bad "无法从 platform.configure 读取完整保留端口列表"
    CB_READY=0
fi

if [ "$CB_READY" -eq 1 ]; then
    if [ ! -x "${CB_DEST}/app/bin/qwenpaw-supervisor" ] \
        || ! cmp -s -- "$TEST_RELEASE_BIN" "${CB_DEST}/app/bin/qwenpaw-supervisor"; then
        bad "config_callback 临时包第一候选不是当前 release supervisor"
    else
        ok "config_callback 临时包第一候选为当前 release supervisor"
    fi

    rc=0
    cb_run wizard_port=not-a-port wizard_log_level=warning > "${T}/cb-invalid.out" 2>&1 || rc=$?
    if [ "$rc" -ne 0 ] && grep -q 'ERROR: 端口无效' "${T}/cb-invalid.out" \
        && [ "$(cb_hashes)" = "${cb_before}" ] \
        && grep -Fq "\"port\": \"${CB_SERVICE_PORT}\"" "${CB_DEST}/ui/config" \
        && cb_no_service_artifacts; then
        ok "config_callback 在服务启动前拒绝非法端口且配置/服务状态不变"
    else
        bad "config_callback 非法端口失败路径异常: rc=$rc"
    fi

    for reserved in "${RESERVED_PORT_LIST[@]}"; do
        rc=0
        cb_run wizard_port="$reserved" >/dev/null 2>&1 || rc=$?
        if [ "$rc" -ne 0 ] && [ "$(cb_hashes)" = "${cb_before}" ] \
            && grep -Fq "\"port\": \"${CB_SERVICE_PORT}\"" "${CB_DEST}/ui/config" \
            && cb_no_service_artifacts; then
            ok "config_callback 拒绝保留端口 $reserved 且配置/服务状态不变"
        else
            bad "config_callback 未在启动前拒绝保留端口 $reserved: rc=$rc"
        fi
    done
fi

# ── [8] release 原生监督者真实生命周期、锁与多进程竞争回归 ──
MAIN_ENTRY="$ROOT/cmd/main"
L_BIN="${QWENPAW_TEST_SUP_BIN:-$ROOT/supervisor/target/release/qwenpaw-supervisor}"
L_RUNTIME="${QWENPAW_TEST_RUNTIME_BIN:-$ROOT/supervisor/target/release/qwenpaw-test-runtime}"
L_DEST="${T}/life_appdest"
L_VAR="${T}/life_var"
L_HOME="${T}/life_home"
L_ETC="${T}/life_etc"
L_SHARE="${T}/life_share"
L_CONTROL="${T}/life_control"
L_VENV="${L_HOME}/venv"
L_RACE_HOLD="${T}/life_race_hold.sh"

absolute_file() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$PWD" "$1" ;;
    esac
}
L_BIN="$(absolute_file "$L_BIN")"
L_RUNTIME="$(absolute_file "$L_RUNTIME")"

life_process_env_has() {
    local pid="$1" entry="$2"
    tr '\0' '\n' < "/proc/${pid}/environ" 2>/dev/null | grep -Fqx -- "$entry"
}

life_supervisor_identity_matches() {
    local pid="$1" expected_ticks="$2" actual_ticks actual_id
    [ -n "$L_BIN_ID" ] && proc_alive_state "$pid" || return 1
    actual_id="$(stat -Lc '%d:%i' -- "/proc/${pid}/exe" 2>/dev/null)" || return 1
    [ "$actual_id" = "$L_BIN_ID" ] || return 1
    actual_ticks="$(proc_start_ticks "$pid" 2>/dev/null)" || return 1
    [ "$actual_ticks" = "$expected_ticks" ] || return 1
    life_process_env_has "$pid" "TRIM_PKGVAR=$L_VAR" \
        && life_process_env_has "$pid" "TRIM_APPDEST=$L_DEST"
}

life_runtime_identity_matches() {
    local pid="$1" expected_ticks="$2" actual_ticks actual_id
    [ -n "$L_RUNTIME_ID" ] && proc_alive_state "$pid" || return 1
    actual_id="$(stat -Lc '%d:%i' -- "/proc/${pid}/exe" 2>/dev/null)" || return 1
    [ "$actual_id" = "$L_RUNTIME_ID" ] || return 1
    actual_ticks="$(proc_start_ticks "$pid" 2>/dev/null)" || return 1
    [ "$actual_ticks" = "$expected_ticks" ] || return 1
    life_process_env_has "$pid" "TRIM_PKGVAR=$L_VAR" \
        && life_process_env_has "$pid" "TRIM_PKGHOME=$L_HOME" \
        && life_process_env_has "$pid" "TRIM_APPDEST=$L_DEST"
}

life_cli_identity_matches() {
    local pid="$1" expected_ticks="$2" expected_id="$3" actual_ticks actual_id
    proc_alive_state "$pid" || return 1
    actual_id="$(stat -Lc '%d:%i' -- "/proc/${pid}/exe" 2>/dev/null)" || return 1
    [ "$actual_id" = "$expected_id" ] || return 1
    actual_ticks="$(proc_start_ticks "$pid" 2>/dev/null)" || return 1
    [ "$actual_ticks" = "$expected_ticks" ] || return 1
    life_process_env_has "$pid" "TRIM_PKGVAR=$L_VAR" \
        && life_process_env_has "$pid" "TRIM_APPDEST=$L_DEST"
}

life_sentinel_identity_matches() {
    local pid="$1" expected_ticks="$2" expected_id="$3" actual_ticks actual_id
    proc_alive_state "$pid" || return 1
    actual_id="$(stat -Lc '%d:%i' -- "/proc/${pid}/exe" 2>/dev/null)" || return 1
    [ "$actual_id" = "$expected_id" ] || return 1
    actual_ticks="$(proc_start_ticks "$pid" 2>/dev/null)" || return 1
    [ "$actual_ticks" = "$expected_ticks" ]
}

life_run() {
    local seconds="$1"
    shift
    env -i "${L_ENV[@]}" timeout --signal=TERM --kill-after=2 "$seconds" \
        bash "$MAIN_ENTRY" "$@" < /dev/null
}

life_register_cli() {
    local pid="$1" ticks="" exe_id="" attempt
    for attempt in 1 2 3 4 5; do
        ticks="$(proc_start_ticks "$pid" 2>/dev/null)"
        exe_id="$(stat -Lc '%d:%i' -- "/proc/${pid}/exe" 2>/dev/null)"
        if [ -n "$ticks" ] && [ "$exe_id" = "$L_TIMEOUT_ID" ] \
            && life_cli_identity_matches "$pid" "$ticks" "$exe_id"; then
            life_cli_pids+=("$pid")
            life_cli_starttimes+=("$ticks")
            life_cli_exeids+=("$exe_id")
            return 0
        fi
        sleep 0.02
    done
    return 1
}

life_spawn_cli() {
    local seconds="$1" operation="$2" ready_file="$3" go_file="$4"
    env -i "${L_ENV[@]}" BASH_ENV="$L_RACE_HOLD" \
        QWENPAW_RACE_READY_FILE="$ready_file" QWENPAW_RACE_GO_FILE="$go_file" \
        timeout --signal=TERM --kill-after=2 "$seconds" \
        bash "$MAIN_ENTRY" "$operation" < /dev/null &
    LIFE_SPAWN_PID=$!
    LIFE_SPAWN_REGISTERED=0
    if life_register_cli "$LIFE_SPAWN_PID"; then
        LIFE_SPAWN_REGISTERED=1
    fi
}

life_api_snapshot() {
    curl --noproxy '*' --silent --show-error --fail \
        --connect-timeout 0.3 --max-time 1 \
        -H "X-Supervisor-Token: $L_TOKEN" \
        "http://127.0.0.1:${L_SUP_PORT}/status" 2>/dev/null
}

life_status_summary() {
    python3 -c 'import json,sys; d=json.load(sys.stdin); assert d.get("success") is True; print(int(bool(d.get("running"))), d.get("pid", ""), d.get("version", ""))' 2>/dev/null
}

life_runtime_health() {
    python3 - "$L_KERNEL_PORT" <<'PYHEALTH'
import json, socket, sys
sock = socket.socket()
sock.settimeout(10.0)
try:
    sock.connect(("127.0.0.1", int(sys.argv[1])))
    sock.sendall(b"GET /api/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
    chunks = []
    while True:
        chunk = sock.recv(4096)
        if not chunk:
            break
        chunks.append(chunk)
    response = b"".join(chunks).split(b"\r\n\r\n", 1)
    if len(response) != 2 or not response[0].startswith(b"HTTP/1.1 200 "):
        raise SystemExit(1)
    if json.loads(response[1]).get("version") != "1.0.0":
        raise SystemExit(1)
finally:
    sock.close()
PYHEALTH
}

life_read_manager() {
    local attempts="$1" attempt pid ticks
    L_SUP_PID=""
    L_SUP_TICKS=""
    for ((attempt = 0; attempt < attempts; attempt++)); do
        pid="$(head -n 1 "${L_VAR}/supervisor.pid" 2>/dev/null | tr -d '[:space:]')"
        ticks="$(tr -d '[:space:]' < "${L_VAR}/supervisor.pid.starttime" 2>/dev/null)"
        case "$pid:$ticks" in
            *[!0-9:]*|:*) ;;
            *)
                if life_supervisor_identity_matches "$pid" "$ticks"; then
                    L_SUP_PID="$pid"
                    L_SUP_TICKS="$ticks"
                    life_track_manager "$pid" "$ticks"
                    return 0
                fi
                ;;
        esac
        sleep 0.1
    done
    return 1
}

life_read_runtime() {
    local attempts="$1" attempt pid="" ticks=""
    L_KERNEL_PID=""
    L_KERNEL_TICKS=""
    for ((attempt = 0; attempt < attempts; attempt++)); do
        pid=""
        ticks=""
        read -r pid ticks < "${L_VENV}/.qwenpaw-test-runtime.pid" 2>/dev/null || true
        case "$pid:$ticks" in
            *[!0-9:]*|:*) ;;
            *)
                if life_runtime_identity_matches "$pid" "$ticks"; then
                    L_KERNEL_PID="$pid"
                    L_KERNEL_TICKS="$ticks"
                    life_track_runtime "$pid" "$ticks"
                    return 0
                fi
                ;;
        esac
        sleep 0.1
    done
    return 1
}

life_fd_has_path() {
    local pid="$1" wanted="$2" fd target
    for fd in /proc/"$pid"/fd/*; do
        [ -e "$fd" ] || continue
        target="$(readlink "$fd" 2>/dev/null)" || continue
        [ "$target" = "$wanted" ] && return 0
    done
    return 1
}

life_lock_free() {
    [ -f "$1" ] && flock -n "$1" -c ':' >/dev/null 2>&1
}

life_track_manager() {
    local pid="$1" ticks="$2" i
    for i in "${!life_pids[@]}"; do
        [ "${life_pids[$i]}:${life_starttimes[$i]}" = "$pid:$ticks" ] && return 0
    done
    life_pids+=("$pid")
    life_starttimes+=("$ticks")
}

life_track_runtime() {
    local pid="$1" ticks="$2" i
    for i in "${!life_runtime_pids[@]}"; do
        [ "${life_runtime_pids[$i]}:${life_runtime_starttimes[$i]}" = "$pid:$ticks" ] && return 0
    done
    life_runtime_pids+=("$pid")
    life_runtime_starttimes+=("$ticks")
}

life_collect_owned_processes() {
    local proc pid id ticks
    [ -n "$L_BIN_ID" ] || return 0
    for proc in /proc/[0-9]*; do
        [ -d "$proc" ] || continue
        pid="${proc##*/}"
        proc_alive_state "$pid" || continue
        id="$(stat -Lc '%d:%i' -- "$proc/exe" 2>/dev/null)" || continue
        ticks="$(proc_start_ticks "$pid" 2>/dev/null)" || continue
        if [ "$id" = "$L_BIN_ID" ] \
            && life_process_env_has "$pid" "TRIM_PKGVAR=$L_VAR" \
            && life_process_env_has "$pid" "TRIM_APPDEST=$L_DEST"; then
            life_track_manager "$pid" "$ticks"
        fi
        if [ -n "$L_RUNTIME_ID" ] && [ "$id" = "$L_RUNTIME_ID" ] \
            && life_process_env_has "$pid" "TRIM_PKGVAR=$L_VAR" \
            && life_process_env_has "$pid" "TRIM_PKGHOME=$L_HOME"; then
            life_track_runtime "$pid" "$ticks"
        fi
    done
}

life_unique_counts() {
    local proc pid count_sup=0 count_runtime=0 id
    for proc in /proc/[0-9]*; do
        [ -d "$proc" ] || continue
        pid="${proc##*/}"
        proc_alive_state "$pid" || continue
        id="$(stat -Lc '%d:%i' -- "$proc/exe" 2>/dev/null)" || continue
        if [ "$id" = "$L_BIN_ID" ] \
            && life_process_env_has "$pid" "TRIM_PKGVAR=$L_VAR" \
            && life_process_env_has "$pid" "TRIM_APPDEST=$L_DEST"; then
            count_sup=$((count_sup + 1))
        fi
        if [ "$id" = "$L_RUNTIME_ID" ] \
            && life_process_env_has "$pid" "TRIM_PKGVAR=$L_VAR" \
            && life_process_env_has "$pid" "TRIM_PKGHOME=$L_HOME"; then
            count_runtime=$((count_runtime + 1))
        fi
    done
    printf '%s %s\n' "$count_sup" "$count_runtime"
}

life_verify_ready() {
    local body summary running api_pid version counts count_sup count_runtime
    L_READY_FAILURE=""
    body="$(life_api_snapshot)" || { L_READY_FAILURE=supervisor-api-unreachable; return 1; }
    summary="$(printf '%s' "$body" | life_status_summary)" \
        || { L_READY_FAILURE=supervisor-api-invalid; return 1; }
    read -r running api_pid version <<< "$summary"
    [ "$running" = 1 ] || { L_READY_FAILURE=api-reports-not-running; return 1; }
    life_read_manager 1 || { L_READY_FAILURE=supervisor-identity; return 1; }
    life_read_runtime 1 || { L_READY_FAILURE=runtime-identity; return 1; }
    [ "$L_KERNEL_PID" = "$api_pid" ] || { L_READY_FAILURE=api-runtime-pid-mismatch; return 1; }
    [ "$version" = 1.0.0 ] || { L_READY_FAILURE=runtime-version-mismatch; return 1; }
    life_runtime_health || { L_READY_FAILURE=runtime-health-endpoint; return 1; }
    [ "$(proc_start_ticks "$L_SUP_PID" 2>/dev/null)" = "$L_SUP_TICKS" ] \
        || { L_READY_FAILURE=supervisor-starttime-changed; return 1; }
    [ "$(proc_start_ticks "$L_KERNEL_PID" 2>/dev/null)" = "$L_KERNEL_TICKS" ] \
        || { L_READY_FAILURE=runtime-starttime-changed; return 1; }
    counts="$(life_unique_counts)"
    read -r count_sup count_runtime <<< "$counts"
    [ "$count_sup" = 1 ] && [ "$count_runtime" = 1 ] \
        || { L_READY_FAILURE=non-unique-process-counts; return 1; }
    life_fd_has_path "$L_SUP_PID" "${L_VAR}/supervisor.lock" \
        || { L_READY_FAILURE=supervisor-lock-fd-missing; return 1; }
    life_fd_has_path "$L_SUP_PID" "${L_VAR}/config-restart.lock" \
        && { L_READY_FAILURE=supervisor-inherited-config-lock; return 1; }
    life_fd_has_path "$L_KERNEL_PID" "${L_VAR}/config-restart.lock" \
        && { L_READY_FAILURE=runtime-inherited-config-lock; return 1; }
    life_fd_has_path "$L_KERNEL_PID" "${L_VAR}/supervisor.lock" \
        && { L_READY_FAILURE=runtime-inherited-supervisor-lock; return 1; }
    [ -e "${L_VAR}/config-restart.lock" ] \
        || { L_READY_FAILURE=config-lock-file-missing; return 1; }
    life_lock_free "${L_VAR}/config-restart.lock" \
        || { L_READY_FAILURE=config-lock-held; return 1; }
    flock -n "${L_VAR}/supervisor.lock" -c ':' >/dev/null 2>&1 \
        && { L_READY_FAILURE=supervisor-lock-not-held; return 1; }
    return 0
}

life_diagnose_state() {
    local body summary counts manager runtime supervisor_lock_fd config_lock_free supervisor_lock_held
    body="$(life_api_snapshot 2>/dev/null)" || body="unavailable"
    summary="$(printf '%s' "$body" | life_status_summary 2>/dev/null)" || summary="invalid"
    counts="$(life_unique_counts 2>/dev/null)"
    manager="$(cat "${L_VAR}/supervisor.pid" "${L_VAR}/supervisor.pid.starttime" 2>/dev/null | tr '\n' ' ')"
    runtime="$(cat "${L_VENV}/.qwenpaw-test-runtime.pid" 2>/dev/null)"
    supervisor_lock_fd=no
    config_lock_free=no
    supervisor_lock_held=no
    life_fd_has_path "${L_SUP_PID:-0}" "${L_VAR}/supervisor.lock" && supervisor_lock_fd=yes
    life_lock_free "${L_VAR}/config-restart.lock" && config_lock_free=yes
    if ! flock -n "${L_VAR}/supervisor.lock" -c ':' >/dev/null 2>&1; then
        supervisor_lock_held=yes
    fi
    printf '  [DIAG] last-readiness-check=%s API=%s supervisor(pid/starttime)=%s runtime(pid/starttime)=%s process-counts=%s\n' \
        "${L_READY_FAILURE:-unknown}" "$summary" "${manager:-missing}" "${runtime:-missing}" "${counts:-unknown}"
    printf '  [DIAG] lock-fd=%s config-lock-free=%s supervisor-lock-held=%s (no active port probes)\n' \
        "$supervisor_lock_fd" "$config_lock_free" "$supervisor_lock_held"
    for log in "${L_VAR}/supervisor.log" "${L_VAR}/qwenpaw.log"; do
        if [ -f "$log" ]; then
            printf '  [DIAG] last log lines: %s\n' "$log"
            tail -n 8 "$log" 2>/dev/null
        fi
    done
}

life_wait_ready() {
    local attempt
    for ((attempt = 0; attempt < 60; attempt++)); do
        if life_verify_ready; then
            return 0
        fi
        sleep 0.1
    done
    echo "  [DIAG] readiness checks did not settle within 6s"
    life_diagnose_state
    return 1
}

life_port_open() {
    python3 - "$1" "$L_SUP_PORT" "$L_KERNEL_PORT" "$L_TOKEN" <<'PYPORT'
import socket, sys
port, supervisor_port, runtime_port = map(int, sys.argv[1:4])
token = sys.argv[4]
if port == supervisor_port:
    path = "/status"
    headers = f"X-Supervisor-Token: {token}\r\n"
elif port == runtime_port:
    path = "/api/auth/status"
    headers = ""
else:
    path = "/"
    headers = ""
sock = socket.socket()
sock.settimeout(1.0)
try:
    sock.connect(("127.0.0.1", port))
except OSError:
    sock.close()
    raise SystemExit(1)
try:
    request = f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{headers}Connection: close\r\n\r\n"
    sock.sendall(request.encode())
    while sock.recv(4096):
        pass
except (OSError, socket.timeout):
    pass
finally:
    sock.close()
raise SystemExit(0)
PYPORT
}

life_port_closed() {
    local port="$1" attempt
    for attempt in 1 2 3 4 5; do
        if life_port_open "$port"; then
            return 1
        fi
        sleep 0.1
    done
    return 0
}

life_all_tracked_stopped() {
    local i pid
    for i in "${!life_pids[@]}"; do
        pid="${life_pids[$i]}"
        life_supervisor_identity_matches "$pid" "${life_starttimes[$i]}" && return 1
    done
    for i in "${!life_runtime_pids[@]}"; do
        pid="${life_runtime_pids[$i]}"
        life_runtime_identity_matches "$pid" "${life_runtime_starttimes[$i]}" && return 1
    done
    return 0
}

life_verify_stopped() {
    local status_rc=0 counts count_sup count_runtime
    life_run 5 status || status_rc=$?
    [ "$status_rc" -eq 3 ] || return 1
    counts="$(life_unique_counts)"
    read -r count_sup count_runtime <<< "$counts"
    [ "$count_sup" -eq 0 ] && [ "$count_runtime" -eq 0 ] || return 1
    [ ! -e "${L_VAR}/supervisor.pid" ] && [ ! -e "${L_VAR}/supervisor.pid.starttime" ] \
        && [ ! -e "${L_VAR}/kernel.pid" ] || return 1
    life_all_tracked_stopped || return 1
    life_port_closed "$L_SUP_PORT" && life_port_closed "$L_KERNEL_PORT" || return 1
    life_lock_free "${L_VAR}/config-restart.lock" \
        && life_lock_free "${L_VAR}/supervisor.lock"
}

life_ensure_stopped() {
    local status_rc=0
    life_run 5 status || status_rc=$?
    case "$status_rc" in
        0)
            life_run 35 stop || return 1
            ;;
        3) ;;
        *) return 1 ;;
    esac
    if life_verify_stopped; then
        return 0
    fi
    echo "  [DIAG] stopped-state checks did not settle"
    life_diagnose_state
    return 1
}

life_ensure_running() {
    local status_rc=0
    life_run 45 start || return 1
    life_run 5 status || status_rc=$?
    [ "$status_rc" -eq 0 ] || return 1
    life_wait_ready
}

life_race_pair() {
    local label="$1" first="$2" second="$3" sequence="$4"
    local gate="${T}/race-${sequence}.go" ready_a="${T}/race-${sequence}.a" \
        ready_b="${T}/race-${sequence}.b" attempt
    rm -f -- "$gate" "$ready_a" "$ready_b"
    life_spawn_cli 45 "$first" "$ready_a" "$gate"
    RACE_A_PID="$LIFE_SPAWN_PID"
    RACE_A_REGISTERED="$LIFE_SPAWN_REGISTERED"
    life_spawn_cli 45 "$second" "$ready_b" "$gate"
    RACE_B_PID="$LIFE_SPAWN_PID"
    RACE_B_REGISTERED="$LIFE_SPAWN_REGISTERED"
    if [ "$RACE_A_PID" = "$RACE_B_PID" ]; then
        touch "$gate"
        wait "$RACE_A_PID" 2>/dev/null || true
        echo "  [FAIL] ${label}: 两个 CLI PID 相同"
        return 1
    fi
    if [ "$RACE_A_REGISTERED" -ne 1 ] || [ "$RACE_B_REGISTERED" -ne 1 ]; then
        touch "$gate"
        wait "$RACE_A_PID" 2>/dev/null || true
        wait "$RACE_B_PID" 2>/dev/null || true
        echo "  [FAIL] ${label}: 无法登记并核验两个 CLI 的进程身份"
        return 1
    fi
    for ((attempt = 0; attempt < 250; attempt++)); do
        [ -f "$ready_a" ] && [ -f "$ready_b" ] && break
        sleep 0.02
    done
    if [ ! -f "$ready_a" ] || [ ! -f "$ready_b" ]; then
        touch "$gate"
        wait "$RACE_A_PID" 2>/dev/null || true
        wait "$RACE_B_PID" 2>/dev/null || true
        echo "  [FAIL] ${label}: 两个独立 CLI 未在屏障处同时就绪"
        return 1
    fi
    touch "$gate"
    RACE_A_RC=0
    wait "$RACE_A_PID" || RACE_A_RC=$?
    RACE_B_RC=0
    wait "$RACE_B_PID" || RACE_B_RC=$?
    RACE_CLIENTS=$((RACE_CLIENTS + 2))
    if [ "$RACE_A_RC" -ne 0 ] || [ "$RACE_B_RC" -ne 0 ]; then
        echo "  [FAIL] ${label}: rc=${RACE_A_RC}/${RACE_B_RC}"
        return 1
    fi
    return 0
}

life_sha256() {
    sha256sum -- "$1" | awk '{print $1}'
}

life_sentinel_alive() {
    [ -n "$SENTINEL_PID" ] \
        && life_sentinel_identity_matches "$SENTINEL_PID" "$SENTINEL_STARTTIME" "$SENTINEL_EXE_ID"
}

life_verify_running() {
    local status_rc=0
    life_run 5 status || status_rc=$?
    [ "$status_rc" -eq 0 ] && life_wait_ready
}

life_verify_state_coherent() {
    local status_rc=0
    life_run 5 status || status_rc=$?
    case "$status_rc" in
        0) life_wait_ready ;;
        3) life_verify_stopped ;;
        *) return 1 ;;
    esac
}

life_run_race_series() {
    local label="$1" first="$2" second="$3" round sequence
    for ((round = 1; round <= L_RACE_ROUNDS; round++)); do
        case "$label" in
            start/start)
                if ! life_ensure_stopped; then
                    echo "  [FAIL] ${label} 第${round}轮: 无法在起点确认停止态"
                    return 1
                fi
                ;;
            *)
                if ! life_ensure_running; then
                    echo "  [FAIL] ${label} 第${round}轮: 无法在起点确认运行态"
                    return 1
                fi
                ;;
        esac
        RACE_SEQUENCE=$((RACE_SEQUENCE + 1))
        sequence="$RACE_SEQUENCE"
        if ! life_race_pair "${label} 第${round}轮" "$first" "$second" "$sequence"; then
            return 1
        fi
        case "$label" in
            restart/stop)
                if ! life_verify_state_coherent; then
                    echo "  [FAIL] ${label} 第${round}轮: 结束态与 PID/API/端口/锁不一致"
                    return 1
                fi
                ;;
            *)
                if ! life_verify_running; then
                    echo "  [FAIL] ${label} 第${round}轮: 最终运行态未通过 API/runtime/锁检查"
                    return 1
                fi
                ;;
        esac
        if ! life_sentinel_alive; then
            echo "  [FAIL] ${label} 第${round}轮: 无关 sentinel 进程意外退出"
            return 1
        fi
        case "$label" in
            start/start) RACE_DONE_START_START=$((RACE_DONE_START_START + 1)) ;;
            restart/start) RACE_DONE_RESTART_START=$((RACE_DONE_RESTART_START + 1)) ;;
            restart/stop) RACE_DONE_RESTART_STOP=$((RACE_DONE_RESTART_STOP + 1)) ;;
        esac
    done
    return 0
}

L_SETUP_OK=1
L_ENV_READY=0
L_RACE_ROUNDS="${QWENPAW_LIFECYCLE_RACE_ROUNDS:-20}"
L_RACE_VALID=1
if [[ ! "$L_RACE_ROUNDS" =~ ^([1-9]|[1-9][0-9]|100)$ ]]; then
    bad "QWENPAW_LIFECYCLE_RACE_ROUNDS 必须是 1..100 的整数（实际: ${L_RACE_ROUNDS}）"
    L_RACE_VALID=0
fi
for tool in timeout curl python3 sha256sum awk stat flock cp chmod sleep env tail; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        bad "缺少 $tool，无法安全执行 release 生命周期回归"
        L_SETUP_OK=0
    fi
done
if [ ! -f "$L_BIN" ] || [ ! -x "$L_BIN" ]; then
    bad "缺少可执行 release supervisor: $L_BIN（不回退到替身 binary）"
    L_SETUP_OK=0
fi
if [ ! -f "$L_RUNTIME" ] || [ ! -x "$L_RUNTIME" ]; then
    bad "缺少可执行 qwenpaw-test-runtime: $L_RUNTIME（不回退到替身 runtime）"
    L_SETUP_OK=0
fi

if [ "$L_SETUP_OK" -eq 1 ]; then
    L_SUPERVISOR_COPY="${L_DEST}/app/bin/qwenpaw-supervisor"
    L_RUNTIME_COPY="${L_VENV}/bin/python3"
    if ! mkdir -p "${L_DEST}/app/bin" "$L_VAR" "$L_HOME/venv/bin" "$L_ETC" "$L_SHARE/.qwenpaw" "$L_CONTROL"; then
        bad "无法创建隔离生命周期测试目录"
        L_SETUP_OK=0
    fi
    if [ "$L_SETUP_OK" -eq 1 ] \
        && { ! cp -- "$L_BIN" "$L_SUPERVISOR_COPY" \
            || ! cp -- "$L_RUNTIME" "$L_RUNTIME_COPY" \
            || ! chmod 755 "$L_SUPERVISOR_COPY" "$L_RUNTIME_COPY"; }; then
        bad "无法准备 release supervisor/runtime 副本"
        L_SETUP_OK=0
    fi
    if [ "$L_SETUP_OK" -eq 1 ]; then
        L_BIN_SHA="$(life_sha256 "$L_BIN")"
        L_BIN_COPY_SHA="$(life_sha256 "$L_SUPERVISOR_COPY")"
        L_RUNTIME_SHA="$(life_sha256 "$L_RUNTIME")"
        L_RUNTIME_COPY_SHA="$(life_sha256 "$L_RUNTIME_COPY")"
        printf '  release supervisor source: %s\n  SHA256: %s\n' "$L_BIN" "$L_BIN_SHA"
        printf '  test runtime source: %s\n  SHA256: %s\n' "$L_RUNTIME" "$L_RUNTIME_SHA"
        printf '%s  %s\n%s  %s\n' "$L_BIN_SHA" "$L_BIN" "$L_RUNTIME_SHA" "$L_RUNTIME" \
            > "${T}/release-sources.sha256"
        if [ -z "$L_BIN_SHA" ] || [ "$L_BIN_SHA" != "$L_BIN_COPY_SHA" ] \
            || [ -z "$L_RUNTIME_SHA" ] || [ "$L_RUNTIME_SHA" != "$L_RUNTIME_COPY_SHA" ]; then
            bad "隔离副本 SHA256 与 release 来源不一致"
            L_SETUP_OK=0
        else
            ok "release supervisor/runtime 副本 SHA256 与来源一致"
        fi
    fi
    if [ "$L_SETUP_OK" -eq 1 ]; then
        L_BIN_ID="$(stat -Lc '%d:%i' -- "$L_SUPERVISOR_COPY" 2>/dev/null)"
        L_RUNTIME_ID="$(stat -Lc '%d:%i' -- "$L_RUNTIME_COPY" 2>/dev/null)"
        L_TIMEOUT_ID="$(stat -Lc '%d:%i' -- "$(command -v timeout)" 2>/dev/null)"
        L_PORTS="$(python3 - <<'PYPORTS'
import socket
sockets = []
for _ in range(2):
    sock = socket.socket()
    sock.bind(("0.0.0.0", 0))
    sockets.append(sock)
print(sockets[0].getsockname()[1], sockets[1].getsockname()[1])
PYPORTS
)"
        read -r L_SUP_PORT L_KERNEL_PORT <<< "$L_PORTS"
        if [ -z "$L_BIN_ID" ] || [ -z "$L_RUNTIME_ID" ] || [ -z "$L_TIMEOUT_ID" ] \
            || [ -z "$L_SUP_PORT" ] || [ -z "$L_KERNEL_PORT" ] \
            || [ "$L_SUP_PORT" = "$L_KERNEL_PORT" ]; then
            bad "无法登记 binary 身份或选择隔离监听端口"
            L_SETUP_OK=0
        fi
    fi
    if [ "$L_SETUP_OK" -eq 1 ]; then
        L_TOKEN="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
        if [ -z "$L_TOKEN" ] \
            || ! printf '%s\n' "$L_TOKEN" > "${L_VAR}/supervisor-token" \
            || ! chmod 600 "${L_VAR}/supervisor-token" \
            || ! touch "${L_VAR}/config-restart.lock" "${L_VAR}/supervisor.lock"; then
            bad "无法初始化隔离 token/生命周期锁文件"
            L_SETUP_OK=0
        else
            L_ENV=(
                "PATH=$PATH"
                "HOME=$L_HOME"
                "TMPDIR=$T"
                "LANG=C"
                "LC_ALL=C"
                "TRIM_APPDEST=$L_DEST"
                "TRIM_PKGVAR=$L_VAR"
                "TRIM_PKGHOME=$L_HOME"
                "TRIM_PKGETC=$L_ETC"
                "TRIM_DATA_SHARE_PATHS=$L_SHARE"
                "TRIM_SERVICE_PORT=$L_KERNEL_PORT"
                "SUPERVISOR_PORT=$L_SUP_PORT"
                "SUPERVISOR_AUTOSTART=1"
                "QWENPAW_TEST_CONTROL=$L_CONTROL"
            )
            L_ENV_READY=1
            if ! life_port_closed "$L_SUP_PORT" || ! life_port_closed "$L_KERNEL_PORT"; then
                bad "动态选择的 supervisor/runtime 端口已有监听者"
                L_SETUP_OK=0
            fi
        fi
    fi
    if [ "$L_SETUP_OK" -eq 1 ]; then
        cat > "$L_RACE_HOLD" <<'EOF'
if [ -n "${QWENPAW_RACE_READY_FILE:-}" ] && [ -n "${QWENPAW_RACE_GO_FILE:-}" ]; then
    : > "$QWENPAW_RACE_READY_FILE"
    for ((race_wait = 0; race_wait < 250; race_wait++)); do
        [ -e "$QWENPAW_RACE_GO_FILE" ] && break
        sleep 0.02
    done
    [ -e "$QWENPAW_RACE_GO_FILE" ] || exit 91
fi
EOF
        cp -- "$(command -v sleep)" "${T}/life-sentinel" \
            && chmod 755 "${T}/life-sentinel" || L_SETUP_OK=0
        if [ "$L_SETUP_OK" -eq 1 ]; then
            "${T}/life-sentinel" 86400 &
            SENTINEL_PID=$!
            for attempt in 1 2 3 4 5 6 7 8 9 10; do
                SENTINEL_STARTTIME="$(proc_start_ticks "$SENTINEL_PID" 2>/dev/null)"
                SENTINEL_EXE_ID="$(stat -Lc '%d:%i' -- "/proc/${SENTINEL_PID}/exe" 2>/dev/null)"
                life_sentinel_alive && break
                sleep 0.02
            done
            if life_sentinel_alive; then
                ok "无关 sentinel 已登记并保持存活"
            else
                bad "无法登记并核验 sentinel 身份，拒绝启动被测服务"
                L_SETUP_OK=0
            fi
        else
            bad "无法创建 sentinel 测试进程副本"
        fi
    fi
fi

if [ "$L_SETUP_OK" -eq 1 ]; then
    BASE_READY=0
    if life_ensure_stopped && life_ensure_running; then
        BASE_READY=1
        START_SUP_PID="$L_SUP_PID"
        START_SUP_TICKS="$L_SUP_TICKS"
        START_KERNEL_PID="$L_KERNEL_PID"
        START_KERNEL_TICKS="$L_KERNEL_TICKS"
        ok "cmd/main start 拉起 release supervisor 与自有健康 runtime（API/runtime 均就绪）"
        life_sentinel_alive || bad "start 后无关 sentinel 未存活"
    else
        bad "cmd/main start 未达到真实 API/runtime 就绪状态"
    fi

    if [ "$BASE_READY" -eq 1 ]; then
        rc=0
        life_run 8 start || rc=$?
        if [ "$rc" -eq 0 ] && life_wait_ready \
            && [ "$L_SUP_PID" = "$START_SUP_PID" ] \
            && [ "$L_SUP_TICKS" = "$START_SUP_TICKS" ] \
            && [ "$L_KERNEL_PID" = "$START_KERNEL_PID" ] \
            && [ "$L_KERNEL_TICKS" = "$START_KERNEL_TICKS" ]; then
            ok "重复 start 在限时内复用同一 supervisor/runtime 实例"
        else
            bad "重复 start 异常: rc=$rc 或 PID/starttime 被意外替换"
        fi

        OLD_SUP_PID="$L_SUP_PID"
        OLD_SUP_TICKS="$L_SUP_TICKS"
        OLD_KERNEL_PID="$L_KERNEL_PID"
        OLD_KERNEL_TICKS="$L_KERNEL_TICKS"
        rc=0
        life_run 35 restart || rc=$?
        if [ "$rc" -eq 0 ] && life_wait_ready \
            && { [ "$L_SUP_PID" != "$OLD_SUP_PID" ] || [ "$L_SUP_TICKS" != "$OLD_SUP_TICKS" ]; } \
            && { [ "$L_KERNEL_PID" != "$OLD_KERNEL_PID" ] || [ "$L_KERNEL_TICKS" != "$OLD_KERNEL_TICKS" ]; } \
            && ! life_supervisor_identity_matches "$OLD_SUP_PID" "$OLD_SUP_TICKS" \
            && ! life_runtime_identity_matches "$OLD_KERNEL_PID" "$OLD_KERNEL_TICKS"; then
            ok "restart 回收旧 supervisor/runtime 并以新 PID/starttime 接管"
        else
            bad "restart 异常: rc=$rc 或新旧进程身份未正确切换"
        fi
        life_sentinel_alive || bad "restart 后无关 sentinel 未存活"

        if life_ensure_running; then
            STOP_SUP_PID="$L_SUP_PID"
            STOP_SUP_TICKS="$L_SUP_TICKS"
            STOP_KERNEL_PID="$L_KERNEL_PID"
            STOP_KERNEL_TICKS="$L_KERNEL_TICKS"
            rc=0
            life_run 35 stop || rc=$?
            if [ "$rc" -eq 0 ] && life_verify_stopped \
                && ! life_supervisor_identity_matches "$STOP_SUP_PID" "$STOP_SUP_TICKS" \
                && ! life_runtime_identity_matches "$STOP_KERNEL_PID" "$STOP_KERNEL_TICKS"; then
                ok "stop 清除 supervisor/runtime PID、端口与锁（status=3）"
            else
                bad "stop 异常: rc=$rc，进程/端口/锁或 PID 文件仍残留"
            fi
        else
            bad "stop 回归前无法恢复到可运行状态"
        fi
        life_sentinel_alive || bad "stop 后无关 sentinel 未存活"

        if [ "$L_RACE_VALID" -eq 1 ]; then
            RACE_SEQUENCE=0
            RACE_CLIENTS=0
            RACE_DONE_START_START=0
            RACE_DONE_RESTART_START=0
            RACE_DONE_RESTART_STOP=0
            if life_run_race_series "start/start" start start; then
                ok "start/start 竞争完成 ${RACE_DONE_START_START}/${L_RACE_ROUNDS} 轮"
            else
                bad "start/start 竞争仅完成 ${RACE_DONE_START_START}/${L_RACE_ROUNDS} 轮"
            fi
            if life_run_race_series "restart/start" restart start; then
                ok "restart/start 竞争完成 ${RACE_DONE_RESTART_START}/${L_RACE_ROUNDS} 轮"
            else
                bad "restart/start 竞争仅完成 ${RACE_DONE_RESTART_START}/${L_RACE_ROUNDS} 轮"
            fi
            if life_run_race_series "restart/stop" restart stop; then
                ok "restart/stop 竞争完成 ${RACE_DONE_RESTART_STOP}/${L_RACE_ROUNDS} 轮（终态逐轮核验）"
            else
                bad "restart/stop 竞争仅完成 ${RACE_DONE_RESTART_STOP}/${L_RACE_ROUNDS} 轮"
            fi
            printf '  实际完成的并发 CLI 调用数: %s；屏障前成功配对: %s/%s/%s 轮\n' \
                "$RACE_CLIENTS" "$RACE_DONE_START_START" "$RACE_DONE_RESTART_START" "$RACE_DONE_RESTART_STOP"
        fi
        if life_ensure_stopped; then
            ok "真实 lifecycle 回归结束后服务已停止"
        else
            bad "真实 lifecycle 回归结束后未能确认服务停止"
        fi
        life_sentinel_alive || bad "全程 sentinel 未保持原身份存活"
    fi
fi

echo "========================================="
if [ "$fail" -eq 0 ]; then
    echo "test-lifecycle: ALL CHECKS PASSED"
    exit 0
fi
echo "test-lifecycle: FAIL"
exit 1
