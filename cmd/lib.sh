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
