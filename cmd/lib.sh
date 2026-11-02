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
# 注:运行态内核由监督者 spawn,日志级别的权威链路是向导落盘
# {VAR_DIR}/qwenpaw-log-level → 监督者白名单校验 → --log-level argv 下发;
# 此 export 仅覆盖安装/升级期脚本直接调起的 python(qwenpaw init 等),
# 监督者 spawn 的内核 app 路径不读它(审查 F1)。
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

# ── 监督者身份校验/安全停止/状态判定（P1-01：与 cmd/main、卸载回调共用
#    同一实现，禁止再内联 kill 循环）──
# shellcheck source=cmd/supervisor_ctl.sh
. "$(dirname "$0")/supervisor_ctl.sh"

# ── 停止监督者（统一 PID 口径 supervisor.pid；旧 qwenpaw.pid 已废弃只清理）──
stop_supervisor() {
    supervisor_safe_stop "${VAR_DIR}/supervisor.pid"
    rm -f "${VAR_DIR}/qwenpaw.pid" 2>/dev/null || true
    return 0
}

# ── 镜像源 URL 映射（统一 install/upgrade/cgi 口径）──
map_mirror_url() {
    case "$1" in
        tsinghua) echo "https://pypi.tuna.tsinghua.edu.cn/simple" ;;
        aliyun)   echo "https://mirrors.aliyun.com/pypi/simple/" ;;
        ustc)     echo "https://pypi.mirrors.ustc.edu.cn/simple/" ;;
        # 向导提供 "official" 选项:显式给出官方源,不要依赖"空串即官方"的隐式行为
        official) echo "https://pypi.org/simple" ;;
        http://*|https://*) echo "$1" ;;
        *)        echo "" ;;
    esac
}

# ── 持久化镜像源到 pip-mirror ──
sync_pip_mirror() {
    local mirror="$1"
    local force="${2:-0}"
    local url
    url="$(map_mirror_url "${mirror}")"
    if [ -n "${url}" ]; then
        if [ "${force}" = "1" ] || [ ! -f "${VAR_DIR}/pip-mirror" ]; then
            mkdir -p "${VAR_DIR}" 2>/dev/null || true
            printf '%s\n' "${url}" > "${VAR_DIR}/pip-mirror" 2>/dev/null || true
        fi
    fi
}

# ── 读取合法版本钉（0=存在且合法；输出版本号）──
read_valid_version_pin() {
    local pin_file="${1:-${VAR_DIR}/qwenpaw-version}"
    [ -f "${pin_file}" ] || return 1
    local raw
    raw="$(tr -d '[:space:]' < "${pin_file}" 2>/dev/null | sed -E 's/^[Vv]ersion:?//' || true)"
    if [ -n "${raw}" ] && valid_version_pin "${raw}"; then
        echo "${raw}"
        return 0
    fi
    return 1
}

# ── 桌面入口端口同步（收敛 install_callback 与 config_callback 重复逻辑）──
sync_ui_config_port() {
    local port="$1"
    [ -n "${port}" ] || return 0
    # 飞牛部署布局是 TRIM_APPDEST=/var/apps/qwenpaw/target,ui/config 在其下;
    # 本地布局则直接落在应用根。两种都要试,不能只认一种。
    local base="${TRIM_APPDEST:-/var/apps/qwenpaw}"
    for ui_cfg in "${base}/ui/config" "${base}/target/ui/config"; do
        if [ -w "${ui_cfg}" ]; then
            sed -i 's/"port": *"[^"]*"/"port": "'"${port}"'"/g' "${ui_cfg}" 2>/dev/null || true
        fi
    done
}
