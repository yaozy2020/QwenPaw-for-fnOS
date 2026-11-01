#!/bin/bash
# cmd/lib.sh — 生命周期回调公共库（由各回调在顶部 source，不作为可执行入口）
#   . "$(dirname "$0")/lib.sh"
#
# 历史教训：环境变量曾散落在 main/install/upgrade 三处，QWENPAW_DEPENDENCY_STATE_DIR
# 漏注入导致内核 b2+ 启动崩溃——运行环境类变量只在 lib 里维护，新增变量只改这里。
# lint 的内嵌脚本展开验证会断言各启动路径携带同一份环境。
#
# 注意：HOME 的语义各回调不同（main 运行态用 ETC_DIR，install/upgrade 用 WORKING_DIR），
# 由各回调自行导出，lib 不统一，避免静默改变已有行为。

# ── 路径（TRIM_* 由飞牛平台注入，缺省为本机安装布局）──
VAR_DIR="${TRIM_PKGVAR:-/var/apps/qwenpaw/var}"
ETC_DIR="${TRIM_PKGETC:-/var/apps/qwenpaw/etc}"
WORKING_DIR="${TRIM_PKGHOME:-/var/apps/qwenpaw/home}"
VENV_DIR="${WORKING_DIR}/venv"
SHARE_DIR="${TRIM_DATA_SHARE_PATHS%%:*}/.qwenpaw"
[ -z "${TRIM_DATA_SHARE_PATHS}" ] && SHARE_DIR="/var/apps/qwenpaw/shares/qwenpaw/.qwenpaw"

# ── 内核运行环境（所有启动路径共用；新增变量只加这里）──
export QWENPAW_WORKING_DIR="${SHARE_DIR}"
export QWENPAW_LOG_LEVEL="${QWENPAW_LOG_LEVEL:-${wizard_log_level:-info}}"
export QWENPAW_AUTH_ENABLED=true
export PATH="/var/apps/nodejs_v24/target/bin:$PATH"
export QWENPAW_DEPENDENCY_STATE_DIR="${SHARE_DIR}/dependency-environments"

# ── 日志：写入 LIB_LOG_FILE（各回调在 source 前指定），缺省写运行日志 ──
log_msg() {
    mkdir -p "${VAR_DIR}" 2>/dev/null || true
    local target="${LIB_LOG_FILE:-${VAR_DIR}/qwenpaw.log}"
    mkdir -p "$(dirname "${target}")" 2>/dev/null || true
    echo "$(date '+%Y-%m-%d %H:%M:%S') - $*" | tee -a "${target}"
}

# ── 内核检测：0=已安装 ──
kernel_installed() {
    [ -x "${VENV_DIR}/bin/python3" ] && \
    "${VENV_DIR}/bin/python3" -c "import importlib.metadata as m; m.version('qwenpaw')" >/dev/null 2>&1
}

# ── 用户可见错误通道（官方规范：生命周期脚本的失败信息写入 TRIM_TEMP_LOGFILE，
#    会展示给用户；未注入时静默跳过）──
notify_user() {
    [ -n "${TRIM_TEMP_LOGFILE:-}" ] || return 0
    echo "$*" >> "${TRIM_TEMP_LOGFILE}" 2>/dev/null || true
}

# ── 版本钉白名单：与监督者安装入口同一规则（PEP 440 字符集）。
#    版本钉会被拼成 `qwenpaw=={pin}` 传给 pip，白名单挡住 requirement 语法/
#    附加 pip 选项/路径类篡改值。0=合法 ──
valid_version_pin() {
    case "$1" in
        ""|.[!.]*|-*) return 1 ;;
        *[!A-Za-z0-9._+-]*) return 1 ;;
    esac
    [ "${#1}" -le 64 ]
}

# ── 原子写版本钉：临时文件 → chmod 600 → rename（半写不会出现在目标路径）。
#    钉文件记录"最后成功运行版本"，venv 重建时按它恢复 ──
write_version_pin() {  # $1=版本内容
    local pin="${VAR_DIR}/qwenpaw-version" tmp="${VAR_DIR}/.qwenpaw-version.tmp"
    mkdir -p "${VAR_DIR}" 2>/dev/null || true
    printf '%s\n' "$1" > "${tmp}" 2>/dev/null || return 1
    chmod 600 "${tmp}" 2>/dev/null || true
    mv -f "${tmp}" "${pin}" 2>/dev/null || return 1
    return 0
}

# ── VAR_DIR 权限收敛：目录 750 / 普通文件 640 / Token 600。
#    历史版本 `chmod -R 755 VAR_DIR` 曾把 supervisor-token 提权到 0755
#    （同机其他用户可读 = 可调监督者管理接口），禁止再用递归 755 ──
secure_var_perms() {
    local dir="${1:-${VAR_DIR}}"
    [ -d "${dir}" ] || return 0
    chmod 750 "${dir}" 2>/dev/null || true
    find "${dir}" -type d -exec chmod 750 {} + 2>/dev/null || true
    find "${dir}" -type f -exec chmod 640 {} + 2>/dev/null || true
    [ -f "${dir}/supervisor-token" ] && chmod 600 "${dir}/supervisor-token" 2>/dev/null || true
    return 0
}

# ── 停止监督者（统一 PID 口径 supervisor.pid；旧 qwenpaw.pid 已废弃只清理）──
stop_supervisor() {
    local pid_file="${VAR_DIR}/supervisor.pid"
    if [ -r "${pid_file}" ]; then
        local pid
        pid=$(head -n 1 "${pid_file}" | tr -d '[:space:]')
        if [ -n "${pid}" ] && kill -0 "${pid}" 2>/dev/null; then
            kill -TERM "${pid}" 2>/dev/null || true
            local count=0
            while kill -0 "${pid}" 2>/dev/null && [ ${count} -lt 15 ]; do
                sleep 1
                count=$((count + 1))
            done
            kill -KILL "${pid}" 2>/dev/null || true
        fi
        rm -f "${pid_file}"
    fi
    rm -f "${VAR_DIR}/qwenpaw.pid" 2>/dev/null || true
    return 0
}
