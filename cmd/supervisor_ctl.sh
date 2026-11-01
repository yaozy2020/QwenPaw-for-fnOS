#!/bin/bash
# cmd/supervisor_ctl.sh — 监督者进程身份校验与安全停止（审计 P1-01/P1-03/P1-04）
# 由 cmd/main、cmd/lib.sh、cmd/uninstall_callback 共同 source：三处停止入口
# 共用同一实现，禁止再各自内联 kill 循环（历史上三处口径漂移的教训）。
#
# 核心纪律：PID 文件是"提示"不是"事实"。kill -0 只证明"该 PID 上有进程"，
# 不证明"该进程是我们的监督者" —— PID 被系统复用后按 PID 发信号会误杀无关
# 进程（已动态复现：把 sleep 的 PID 写入 supervisor.pid，旧实现 status 误报
# 运行、stop 会杀掉它）。因此：
#   1) 发信号前必须核对进程身份（/proc/<pid>/exe 优先，cmdline 兜底）；
#   2) 启动时记录 /proc/<pid>/stat 的 starttime，停止时一并比对，
#      覆盖"复用 PID 恰好也是同名二进制"的窗口；
#   3) 身份不符时只清理陈旧 PID 文件并如实返回，绝不发信号。
#
# 本文件 source 时不得产生任何副作用（不 export、不读环境、不落盘）。

# ── 外部停止宽限（秒）：必须覆盖监督者收到 TERM 后内部最坏清理时长 ——
#    安装取消 ~5s + 内核组 TERM 宽限 10s + 单 KILL 等待 0.5s + 后代清扫 ~1s
#    ≈ 16.5s，再加 /proc 扫描与调度余量。旧值 15s 小于内部最坏值，会在后代
#    清扫完成前强杀监督者，留下 pip/浏览器/MCP 孤儿（P1-03）。三处入口共用
#    本常量，改动只改这里。
SUPERVISOR_STOP_GRACE_SECS="${QWENPAW_STOP_GRACE_SECS:-30}"
# 监督者二进制特征名（exe basename 与 cmdline 均按此识别）
SUPERVISOR_BIN_NAME="qwenpaw-supervisor"

# ── 存活判定：与监督者 Rust 侧 process_alive 同口径 —— 看 stat 状态字符
#    （R/S/D/T 为活，Z 僵尸=已死），不看 /proc 目录存在。
#    返回 0=存活；文件不可读（进程已消失）同样视为不存活。
proc_alive_state() {
    local s rest
    s="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
    rest="${s#*) }"
    case "${rest%% *}" in
        R|S|D|T) return 0 ;;
        *) return 1 ;;
    esac
}

# ── 进程 starttime（/proc/<pid>/stat 去掉 "pid (comm) " 后的第 20 个字段）──
proc_start_ticks() {
    local s
    s="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
    s="${s#*) }"
    # shellcheck disable=SC2086  # stat 字段必须按空白分词
    set -- $s
    [ "$#" -ge 20 ] || return 1
    # 注意:${20} 而非 $20 —— 后者是 ${1}0(状态字符拼个 0),starttime 就废了
    printf '%s\n' "${20}"
}

# ── 身份核对：0 = 该 PID 是"我们的、仍存活的"监督者。
#    用法: supervisor_identity_ok <pid> [expected_start_ticks]
#    exe 可读时以 exe basename 判定（读到但不是监督者 → 确定不是，拒绝）；
#    exe 不可读（权限/已消失）退化用 cmdline 特征，cmdline 也不可读则拒绝
#    （拒绝放行是安全缺省：我们的进程对本回调用户必然可读）。
supervisor_identity_ok() {
    local pid="$1" want_ticks="${2:-}"
    case "$pid" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [ "$pid" -gt 1 ] || return 1
    proc_alive_state "$pid" || return 1
    local exe cmdline
    exe="$(readlink "/proc/${pid}/exe" 2>/dev/null)"
    if [ -n "$exe" ]; then
        case "${exe##*/}" in
            "${SUPERVISOR_BIN_NAME}"*) : ;;
            *) return 1 ;;
        esac
    else
        cmdline="$(tr '\0' ' ' < "/proc/${pid}/cmdline" 2>/dev/null)" || return 1
        case "$cmdline" in
            *"${SUPERVISOR_BIN_NAME}"*) : ;;
            *) return 1 ;;
        esac
    fi
    if [ -n "$want_ticks" ]; then
        local now_ticks
        now_ticks="$(proc_start_ticks "$pid")" || return 1
        [ "$now_ticks" = "$want_ticks" ] || return 1
    fi
    return 0
}

# ── 启动登记：写 PID 文件 + starttime 戳。戳写失败不阻断启动 —— 此时身份
#    核对自动退化为 exe/cmdline（监督者自身启动后还会覆写 PID 文件，同进程
#    同 starttime，戳仍然有效）。
supervisor_register_start() {  # $1=pid $2=pid_file
    local pid="$1" pid_file="$2" ticks
    printf '%s\n' "$pid" > "$pid_file" 2>/dev/null || return 1
    ticks="$(proc_start_ticks "$pid" 2>/dev/null)" || return 0
    printf '%s\n' "$ticks" > "${pid_file}.starttime" 2>/dev/null || true
    return 0
}

# ── 统一状态：0=我们的监督者正在运行；3=未运行（顺带清理陈旧 PID 文件）。
#    官方约定 status 退出码 0/3，调用方据此上报平台。
supervisor_status() {
    local pid_file="$1" pid want_ticks=""
    pid="$(head -n 1 "$pid_file" 2>/dev/null | tr -d '[:space:]')"
    if [ -n "$pid" ]; then
        if [ -r "${pid_file}.starttime" ]; then
            want_ticks="$(tr -d '[:space:]' < "${pid_file}.starttime")"
        fi
        if supervisor_identity_ok "$pid" "$want_ticks"; then
            return 0
        fi
    fi
    rm -f "$pid_file" "${pid_file}.starttime" 2>/dev/null || true
    return 3
}

# ── 统一安全停止：身份核对通过才 TERM → 宽限 → KILL；身份不符只清理
#    陈旧 PID 文件（返回 1），绝不发信号。宽限期内每秒复核身份：进程死亡
#    （含短暂僵尸态）立即返回，不空耗宽限。
#    返回：0=已停止或本就未运行；1=PID 文件存在但身份不符（已清理，未发信号）。
supervisor_safe_stop() {  # $1=pid_file
    local pid_file="$1" pid want_ticks=""
    pid="$(head -n 1 "$pid_file" 2>/dev/null | tr -d '[:space:]')"
    if [ -z "$pid" ]; then
        rm -f "$pid_file" "${pid_file}.starttime" 2>/dev/null || true
        return 0
    fi
    if [ -r "${pid_file}.starttime" ]; then
        want_ticks="$(tr -d '[:space:]' < "${pid_file}.starttime")"
    fi
    if ! supervisor_identity_ok "$pid" "$want_ticks"; then
        rm -f "$pid_file" "${pid_file}.starttime" 2>/dev/null || true
        return 1
    fi
    kill -TERM "$pid" 2>/dev/null || true
    local count=0
    while supervisor_identity_ok "$pid" "$want_ticks" \
        && [ "$count" -lt "$SUPERVISOR_STOP_GRACE_SECS" ]; do
        sleep 1
        count=$((count + 1))
    done
    if supervisor_identity_ok "$pid" "$want_ticks"; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
    rm -f "$pid_file" "${pid_file}.starttime" 2>/dev/null || true
    return 0
}
