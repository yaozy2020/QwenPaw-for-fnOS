#!/bin/bash

# 路径解析：TRIM_* 注入时优先（官方规范），否则回退 /var/apps 约定（官方 cgi 示例即硬编码）
VAR_DIR="${TRIM_PKGVAR:-/var/apps/qwenpaw/var}"
HOME_DIR="${TRIM_PKGHOME:-/var/apps/qwenpaw/home}"
SHARE_DIR="${TRIM_DATA_SHARE_PATHS%%:*}/.qwenpaw"
[ -z "${TRIM_DATA_SHARE_PATHS}" ] && SHARE_DIR="/var/apps/qwenpaw/shares/qwenpaw/.qwenpaw"
VENV_DIR="${HOME_DIR}/venv"

LOG_FILE="${VAR_DIR}/qwenpaw.log"
PID_FILE="${VAR_DIR}/qwenpaw.pid"
UPGRADE_LOG_FILE="${VAR_DIR}/qwenpaw-upgrade.log"
UPGRADE_PID_FILE="${VAR_DIR}/qwenpaw-upgrade.pid"
UPGRADE_LOCK_DIR="${VAR_DIR}/qwenpaw-upgrade.lock"
STATUS_CACHE_FILE="${VAR_DIR}/qwenpaw-status.cache"
PORT_FILE="${VAR_DIR}/service_port"

get_service_port() {
    local p=""
    if [ -f "${PORT_FILE}" ]; then
        p=$(head -n 1 "${PORT_FILE}" | tr -d '[:space:]')
    fi
    echo "${p:-19091}"
}

STATUS_CACHE_TTL=60

AUTH_FILE="${SHARE_DIR}.secret/auth.json"

mkdir -p "$(dirname "${LOG_FILE}")"

check_process() {
    local pid=$1
    if [ -n "${pid}" ] && kill -0 "${pid}" 2>/dev/null; then
        return 0
    else
        return 1
    fi
}

refresh_status_cache() {
    if [ -f "${STATUS_CACHE_FILE}" ]; then
        local cache_age
        cache_age=$(($(date +%s) - $(stat -c %Y "${STATUS_CACHE_FILE}" 2>/dev/null || echo 0)))
        if [ "${cache_age}" -lt "${STATUS_CACHE_TTL}" ]; then
            return 0
        fi
    fi

    if [ -f "${VENV_DIR}/bin/python3" ]; then
        source "${VENV_DIR}/bin/activate" 2>/dev/null
        python3 -c "
import importlib.metadata as m
v=''
try: v=m.version('qwenpaw')
except: pass
print(f'version={v}')
" > "${STATUS_CACHE_FILE}" 2>/dev/null
    else
        echo "version=" > "${STATUS_CACHE_FILE}"
    fi
}

invalidate_status_cache() {
    rm -f "${STATUS_CACHE_FILE}"
}

read_log_file_json() {
    local log_file="$1"
    local lines="$2"

    if [ ! -f "${log_file}" ]; then
        echo ""
        return
    fi

    if [ -f "${VENV_DIR}/bin/python3" ]; then
        source "${VENV_DIR}/bin/activate" 2>/dev/null
        python3 -c "
import json, sys, re
ansi_escape = re.compile(r'\x1b\[[0-9;]*m')
lines = []
try:
    with open('${log_file}') as f:
        all_lines = f.readlines()
        tailed = all_lines[-${lines}:] if len(all_lines) > ${lines} else all_lines
    for line in tailed:
        stripped = ansi_escape.sub('', line.rstrip('\n'))
        if stripped.strip():
            lines.append({'level': 'info', 'time': '', 'message': stripped})
    print(json.dumps({'success': True, 'logs': lines}))
except Exception as e:
    print(json.dumps({'success': True, 'logs': [], 'message': str(e)}))
" 2>/dev/null
    else
        echo '{"success":true,"logs":[]}'
    fi
}

get_status() {
    local running=false
    local pid=""
    local start_at="null"

    if [ -f "${PID_FILE}" ]; then
        pid=$(head -n 1 "${PID_FILE}" | tr -d '[:space:]')
        if check_process "${pid}"; then
            running=true
            if command -v stat &>/dev/null; then
                start_at=$(stat -c %Y "${PID_FILE}" 2>/dev/null || echo "null")
            fi
        else
            rm -f "${PID_FILE}"
            pid=""
        fi
    fi

    local version=""

    refresh_status_cache
    if [ -f "${STATUS_CACHE_FILE}" ]; then
        source "${STATUS_CACHE_FILE}" 2>/dev/null
        version="${version:-}"
    fi

    echo "Content-Type: application/json"
    echo ""

    local auth_enabled=false
    if [ -f "${AUTH_FILE}" ]; then
        auth_enabled=true
    fi

    local installed=false
    if [ -n "${version}" ]; then
        installed=true
    fi

    local current_port=$(get_service_port)
    echo "{\"success\":true,\"running\":${running},\"pid\":\"${pid}\",\"startAt\":${start_at},\"version\":\"${version}\",\"installed\":${installed},\"authEnabled\":${auth_enabled},\"port\":\"${current_port}\"}"
}

status() {
    get_status
}

start_service() {
    if [ "$REQUEST_METHOD" != "POST" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"仅支持 POST 请求\"}"
        exit 0
    fi

    # 内核未安装时禁止启动（首次安装模式：内核由控制台「版本更新」安装）
    if ! "${VENV_DIR}/bin/python3" -c "import importlib.metadata as m; m.version('qwenpaw')" >/dev/null 2>&1; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"QwenPaw 内核尚未安装，请在「版本更新」中选择版本安装\",\"installed\":false}"
        exit 0
    fi

    if [ -f "${PID_FILE}" ]; then
        local pid=$(head -n 1 "${PID_FILE}" | tr -d '[:space:]')
        if check_process "${pid}"; then
            echo "Content-Type: application/json"
            echo ""
            echo "{\"success\":true,\"message\":\"QwenPaw 已在运行\"}"
            exit 0
        else
            rm -f "${PID_FILE}"
        fi
    fi

    local cmd="export HOME=${HOME_DIR} && export QWENPAW_WORKING_DIR=${SHARE_DIR} && export QWENPAW_DEPENDENCY_STATE_DIR=${SHARE_DIR}/dependency-environments && export QWENPAW_AUTH_ENABLED=true && export PATH=/var/apps/nodejs_v24/target/bin:\$PATH && source ${VENV_DIR}/bin/activate && exec python3 -m qwenpaw app --host 0.0.0.0 --port $(get_service_port)"
    setsid bash -c "${cmd}" </dev/null >> "${LOG_FILE}" 2>&1 &
    echo $! > "${PID_FILE}"

    sleep 2
    if check_process "$(cat "${PID_FILE}" 2>/dev/null)"; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":true,\"message\":\"QwenPaw 启动成功\"}"
    else
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"QwenPaw 启动失败，请查看日志\"}"
    fi
}

stop_service() {
    if [ "$REQUEST_METHOD" != "POST" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"仅支持 POST 请求\"}"
        exit 0
    fi

    if [ -f "${PID_FILE}" ]; then
        local pid=$(head -n 1 "${PID_FILE}" | tr -d '[:space:]')
        if check_process "${pid}"; then
            # 保底机制:setsid 启动的内核为自身进程组长,组信号连带浏览器/MCP
            # 等子进程;pgid != pid 时禁用整组杀,防止误伤同组平台进程
            local pgid=$(ps -o pgid= -p "${pid}" 2>/dev/null | tr -d '[:space:]')
            if [ -n "${pgid}" ] && [ "${pgid}" = "${pid}" ]; then
                kill -TERM -- -"${pid}" 2>/dev/null || true
            fi
            kill -TERM "${pid}"

            local count=0
            while check_process "${pid}" && [ $count -lt 10 ]; do
                sleep 1
                count=$((count + 1))
            done

            if check_process "${pid}"; then
                kill -KILL "${pid}"
                sleep 1
            fi
            # 兜底清扫:无视 TERM 的子进程由组 KILL 清除
            if [ -n "${pgid}" ] && [ "${pgid}" = "${pid}" ]; then
                kill -KILL -- -"${pid}" 2>/dev/null || true
            fi

            rm -f "${PID_FILE}"
            echo "Content-Type: application/json"
            echo ""
            echo "{\"success\":true,\"message\":\"QwenPaw 已停止\"}"
        else
            rm -f "${PID_FILE}"
            echo "Content-Type: application/json"
            echo ""
            echo "{\"success\":true,\"message\":\"QwenPaw 未在运行\"}"
        fi
    else
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":true,\"message\":\"QwenPaw 未在运行\"}"
    fi
}

restart_service() {
    if [ "$REQUEST_METHOD" != "POST" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"仅支持 POST 请求\"}"
        exit 0
    fi

    # 内核未安装时禁止启动（首次安装模式：内核由控制台「版本更新」安装）
    if ! "${VENV_DIR}/bin/python3" -c "import importlib.metadata as m; m.version('qwenpaw')" >/dev/null 2>&1; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"QwenPaw 内核尚未安装，请在「版本更新」中选择版本安装\",\"installed\":false}"
        exit 0
    fi

    if [ -f "${PID_FILE}" ]; then
        local pid=$(head -n 1 "${PID_FILE}" | tr -d '[:space:]')
        if check_process "${pid}"; then
            local rpgid=$(ps -o pgid= -p "${pid}" 2>/dev/null | tr -d '[:space:]')
            if [ -n "${rpgid}" ] && [ "${rpgid}" = "${pid}" ]; then
                kill -TERM -- -"${pid}" 2>/dev/null || true
            fi
            kill -TERM "${pid}"
            local count=0
            while check_process "${pid}" && [ $count -lt 10 ]; do
                sleep 1
                count=$((count + 1))
            done
            if check_process "${pid}"; then
                kill -KILL "${pid}"
                sleep 1
            fi
        fi
        rm -f "${PID_FILE}"
    fi

    local cmd="export HOME=${HOME_DIR} && export QWENPAW_WORKING_DIR=${SHARE_DIR} && export QWENPAW_DEPENDENCY_STATE_DIR=${SHARE_DIR}/dependency-environments && export QWENPAW_AUTH_ENABLED=true && export PATH=/var/apps/nodejs_v24/target/bin:\$PATH && source ${VENV_DIR}/bin/activate && exec python3 -m qwenpaw app --host 0.0.0.0 --port $(get_service_port)"
    setsid bash -c "${cmd}" </dev/null >> "${LOG_FILE}" 2>&1 &
    echo $! > "${PID_FILE}"

    sleep 2
    if check_process "$(cat "${PID_FILE}" 2>/dev/null)"; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":true,\"message\":\"QwenPaw 重启成功\"}"
    else
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"QwenPaw 重启失败，请查看日志\"}"
    fi
}

change_port() {
    if [ "$REQUEST_METHOD" != "POST" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"仅支持 POST 请求\"}"
        exit 0
    fi

    # 内核未安装时不允许改端口重启（首次安装模式）
    if ! "${VENV_DIR}/bin/python3" -c "import importlib.metadata as m; m.version('qwenpaw')" >/dev/null 2>&1; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"QwenPaw 内核尚未安装，请在「版本更新」中选择版本安装\",\"installed\":false}"
        exit 0
    fi

    local new_port=""
    if echo "$QUERY_STRING$REQUEST_URI" | grep -q "port="; then
        new_port=$(echo "$QUERY_STRING$REQUEST_URI" | sed 's/.*port=\([^&]*\).*/\1/')
    fi

    # 简单端口校验: 必须纯数字且在 1024-65535 范围
    if ! echo "$new_port" | grep -qE '^[0-9]+$' || [ "$new_port" -lt 1024 ] || [ "$new_port" -gt 65535 ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"无效的端口号 (需在 1024~65535 之间)\"}"
        exit 0
    fi

    # 1. 持久化到 service_port 文件
    echo "$new_port" > "${PORT_FILE}"

    # 2. 同步修改飞牛系统元数据与桌面快捷方式配置 (保证桌面图标点击也对齐新端口)
    local ui_cfg="/var/apps/qwenpaw/app/ui/config"
    if [ -w "$ui_cfg" ]; then
        sed -i 's/"port": *"[0-9]*"/"port": "'"${new_port}"'"/g' "$ui_cfg" 2>/dev/null || true
    fi
    local mf_cfg="/var/apps/qwenpaw/manifest"
    if [ -w "$mf_cfg" ]; then
        sed -i 's/^service_port *= *[0-9]*/service_port               = '"${new_port}"'/g' "$mf_cfg" 2>/dev/null || true
    fi

    # 3. 彻底杀死旧 PID，并直接拉起绑定新端口的进程
    setsid bash -c "
        sleep 1
        if [ -f '${PID_FILE}' ]; then
            old_pid=\$(head -n 1 '${PID_FILE}' | tr -d '[:space:]')
            if [ -n \"\${old_pid}\" ] && kill -0 \"\${old_pid}\" 2>/dev/null; then
                kill -TERM \"\${old_pid}\"
                count=0
                while kill -0 \"\${old_pid}\" 2>/dev/null && [ \$count -lt 10 ]; do
                    sleep 1
                    count=\$((count + 1))
                done
                kill -9 \"\${old_pid}\" 2>/dev/null || true
            fi
            rm -f '${PID_FILE}'
        fi
        export HOME=${HOME_DIR}
        export QWENPAW_WORKING_DIR=${SHARE_DIR}
        export QWENPAW_DEPENDENCY_STATE_DIR=${SHARE_DIR}/dependency-environments
        export QWENPAW_AUTH_ENABLED=true
        export PATH=/var/apps/nodejs_v24/target/bin:\$PATH
        source ${VENV_DIR}/bin/activate
        setsid python3 -m qwenpaw app --host 0.0.0.0 --port ${new_port} >> ${LOG_FILE} 2>&1 &
        echo \$! > ${PID_FILE}
    " </dev/null >/dev/null 2>&1 &

    echo "Content-Type: application/json"
    echo ""
    echo "{\"success\":true,\"message\":\"端口已更改为 ${new_port}，旧服务已终止，正在绑定新端口拉起...\",\"port\":\"${new_port}\"}"
}

get_version_list() {
    echo "Content-Type: application/json"
    echo ""
    python3 -c "
import urllib.request, json
try:
    req = urllib.request.Request('https://pypi.org/pypi/qwenpaw/json', headers={'User-Agent': 'QwenPaw'})
    with urllib.request.urlopen(req, timeout=5) as res:
        data = json.loads(res.read().decode())
        releases = list(data.get('releases', {}).keys())
        latest_stable = data.get('info', {}).get('version')
        recent = releases[-14:]
        items = []
        for v in reversed(recent):
            is_beta = any(x in v.lower() for x in ['a', 'b', 'rc', 'dev', 'pre'])
            items.append({'version': v, 'is_beta': is_beta, 'is_stable': v == latest_stable})
        print(json.dumps({'success': True, 'latest_stable': latest_stable, 'versions': items}))
except Exception as e:
    fallback = [
        {'version': '2.3.0b1', 'is_beta': True, 'is_stable': False},
        {'version': '2.2.2.post1', 'is_beta': False, 'is_stable': True},
        {'version': '2.2.2b6', 'is_beta': True, 'is_stable': False},
        {'version': '2.2.2', 'is_beta': False, 'is_stable': False}
    ]
    print(json.dumps({'success': True, 'latest_stable': '2.2.2.post1', 'versions': fallback, 'fallback': True}))
" 2>/dev/null
}

get_upgrade_status() {
    local upgrading=false
    local upgrade_pid=""

    if [ -f "${UPGRADE_PID_FILE}" ]; then
        upgrade_pid=$(head -n 1 "${UPGRADE_PID_FILE}" | tr -d '[:space:]')
        if check_process "${upgrade_pid}"; then
            upgrading=true
        else
            rm -f "${UPGRADE_PID_FILE}"
        fi
    fi

    echo "Content-Type: application/json"
    echo ""
    echo "{\"success\":true,\"upgrading\":${upgrading}}"
}

upgrade_service() {
    if [ "$REQUEST_METHOD" != "POST" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"仅支持 POST 请求\"}"
        exit 0
    fi

    if ! mkdir "${UPGRADE_LOCK_DIR}" 2>/dev/null; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"升级正在进行中，请稍候\"}"
        exit 0
    fi

    local mirror=""
    local mirror_url=""

    if echo "$QUERY_STRING$REQUEST_URI" | grep -q "mirror="; then
        mirror=$(echo "$QUERY_STRING$REQUEST_URI" | sed 's/.*mirror=\([^&]*\).*/\1/')
    fi

    case "$mirror" in
        tsinghua)
            mirror_url="-i https://pypi.tuna.tsinghua.edu.cn/simple"
            ;;
        aliyun)
            mirror_url="-i https://mirrors.aliyun.com/pypi/simple"
            ;;
        ustc)
            mirror_url="-i https://pypi.mirrors.ustc.edu.cn/simple"
            ;;
    esac

    > "${UPGRADE_LOG_FILE}"

        local target_ver=""
    if echo "$QUERY_STRING$REQUEST_URI" | grep -q "version="; then
        target_ver=$(echo "$QUERY_STRING$REQUEST_URI" | sed 's/.*version=\([^&]*\).*/\1/')
    fi

    local pkg_spec="qwenpaw"
    if [ "$target_ver" = "beta" ] || [ "$target_ver" = "pre" ]; then
        pkg_spec="--pre qwenpaw"
    elif [ -n "$target_ver" ] && [ "$target_ver" != "latest" ]; then
        pkg_spec="qwenpaw==${target_ver}"
    fi

    local upgrade_cmd="source ${VENV_DIR}/bin/activate && PYTHONUNBUFFERED=1 pip install --upgrade ${pkg_spec} ${mirror_url} 2>&1"

    setsid bash -c "
        echo '=== QwenPaw 升级开始 ===' >> ${UPGRADE_LOG_FILE}
        echo '时间: $(date)' >> ${UPGRADE_LOG_FILE}
        echo '' >> ${UPGRADE_LOG_FILE}
        ${upgrade_cmd} >> ${UPGRADE_LOG_FILE} 2>&1
        rc=\$?
        echo '' >> ${UPGRADE_LOG_FILE}
        if [ \$rc -eq 0 ]; then
            echo '=== 升级成功 ===' >> ${UPGRADE_LOG_FILE}
            rm -f ${STATUS_CACHE_FILE}
            ${VENV_DIR}/bin/pip show qwenpaw 2>/dev/null | grep -a '^Version:' | cut -d' ' -f2 > ${VAR_DIR}/qwenpaw-version
            echo '正在重启 QwenPaw...' >> ${UPGRADE_LOG_FILE}
            if [ -f ${PID_FILE} ]; then
                old_pid=\$(head -n 1 ${PID_FILE} | tr -d '[:space:]')
                if kill -0 \${old_pid} 2>/dev/null; then
                    kill -TERM \${old_pid}
                    count=0
                    while kill -0 \${old_pid} 2>/dev/null && [ \$count -lt 10 ]; do
                        sleep 1
                        count=\$((count + 1))
                    done
                    if kill -0 \${old_pid} 2>/dev/null; then
                        kill -KILL \${old_pid}
                    fi
                fi
                rm -f ${PID_FILE}
            fi
            export HOME=${HOME_DIR}
            export QWENPAW_WORKING_DIR=${SHARE_DIR}
            export QWENPAW_DEPENDENCY_STATE_DIR=${SHARE_DIR}/dependency-environments
            export QWENPAW_AUTH_ENABLED=true
            export PATH=/var/apps/nodejs_v24/target/bin:\$PATH
            source ${VENV_DIR}/bin/activate
            setsid python3 -m qwenpaw app --host 0.0.0.0 --port $(get_service_port) >> ${LOG_FILE} 2>&1 &
            echo \$! > ${PID_FILE}
            echo 'QwenPaw 已重启' >> ${UPGRADE_LOG_FILE}
        else
            echo '=== 升级失败 (exit code: '\$rc') ===' >> ${UPGRADE_LOG_FILE}
        fi
        rm -f ${UPGRADE_PID_FILE}
        rm -rf ${UPGRADE_LOCK_DIR}
    " </dev/null &
    echo $! > "${UPGRADE_PID_FILE}"

    sleep 1

    echo "Content-Type: application/json"
    echo ""
    echo "{\"success\":true,\"message\":\"升级已开始\"}"
}

get_upgrade_logs() {
    local json_output
    json_output=$(read_log_file_json "${UPGRADE_LOG_FILE}" 500)

    echo "Content-Type: application/json"
    echo ""
    if [ -n "${json_output}" ]; then
        echo "${json_output}"
    else
        echo "{\"success\":true,\"logs\":[]}"
    fi
}

get_logs() {
    local json_output
    json_output=$(read_log_file_json "${LOG_FILE}" 500)

    echo "Content-Type: application/json"
    echo ""
    if [ -n "${json_output}" ]; then
        echo "${json_output}"
    else
        echo "{\"success\":true,\"logs\":[]}"
    fi
}

clear_logs() {
    if [ "$REQUEST_METHOD" != "POST" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"仅支持 POST 请求\"}"
        exit 0
    fi

    > "${LOG_FILE}"
    > "${UPGRADE_LOG_FILE}"

    echo "Content-Type: application/json"
    echo ""
    echo "{\"success\":true,\"message\":\"日志已清空\"}"
}

reset_auth() {
    if [ "$REQUEST_METHOD" != "POST" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"仅支持 POST 请求\"}"
        exit 0
    fi

    if [ -f "${AUTH_FILE}" ]; then
        rm -f "${AUTH_FILE}"
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":true,\"message\":\"密码已重置，下次进入 QwenPaw 时将重新设置账号密码\"}"
    else
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":true,\"message\":\"未找到认证文件，无需重置\"}"
    fi
}

check_tunnel_installed() {
    local tunnel_api="/var/apps/com.dustinky.tunnel/target/ui/api.cgi"
    local installed=false

    if [ -f "${tunnel_api}" ]; then
        installed=true
    fi

    echo "Content-Type: application/json"
    echo ""
    echo "{\"success\":true,\"installed\":${installed}}"
}

backup_download() {
    local working_base="${SHARE_DIR%/.qwenpaw}"
    local working_dir="${working_base}/.qwenpaw"
    local secret_dir="${working_base}/.qwenpaw.secret"
    local backup_name="qwenpaw-backup-$(date +%Y%m%d-%H%M%S).tar.gz"
    local tmp_file="/tmp/${backup_name}"

    if [ ! -d "${working_dir}" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"工作目录不存在，无法备份\"}"
        exit 0
    fi

    if [ ! -d "${secret_dir}" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"密钥目录不存在，无法备份\"}"
        exit 0
    fi

    if ! tar -czf "${tmp_file}" -C "${working_base}" ".qwenpaw" ".qwenpaw.secret" 2>/dev/null; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"打包失败，请检查磁盘空间和权限\"}"
        rm -f "${tmp_file}"
        exit 0
    fi

    if [ ! -f "${tmp_file}" ] || [ ! -s "${tmp_file}" ]; then
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"打包失败，文件为空\"}"
        rm -f "${tmp_file}"
        exit 0
    fi

    echo "Content-Type: application/gzip"
    echo "Content-Disposition: attachment; filename=\"${backup_name}\""
    echo ""

    cat "${tmp_file}"
    rm -f "${tmp_file}"
}

action=""

if [ -n "$QUERY_STRING" ]; then
    if echo "$QUERY_STRING" | grep -q "action=status"; then
        action="status"
    elif echo "$QUERY_STRING" | grep -q "action=start"; then
        action="start"
    elif echo "$QUERY_STRING" | grep -q "action=stop"; then
        action="stop"
    elif echo "$QUERY_STRING" | grep -q "action=restart"; then
        action="restart"
    elif echo "$QUERY_STRING" | grep -q "action=upgrade_status"; then
        action="upgrade_status"
    elif echo "$QUERY_STRING" | grep -q "action=upgrade_logs"; then
        action="upgrade_logs"
    elif echo "$QUERY_STRING" | grep -q "action=upgrade"; then
        action="upgrade"
    elif echo "$QUERY_STRING" | grep -q "action=version_list"; then
        action="version_list"
    elif echo "$QUERY_STRING" | grep -q "action=change_port"; then
        action="change_port"
    elif echo "$QUERY_STRING" | grep -q "action=logs"; then
        action="logs"
    elif echo "$QUERY_STRING" | grep -q "action=clear_logs"; then
        action="clear_logs"
    elif echo "$QUERY_STRING" | grep -q "action=reset_auth"; then
        action="reset_auth"
    elif echo "$QUERY_STRING" | grep -q "action=backup_download"; then
        action="backup_download"
    elif echo "$QUERY_STRING" | grep -q "action=check_tunnel"; then
        action="check_tunnel"
    fi
fi

if [ -z "$action" ] && [ -n "$REQUEST_URI" ]; then
    if echo "$REQUEST_URI" | grep -q "action=status"; then
        action="status"
    elif echo "$REQUEST_URI" | grep -q "action=start"; then
        action="start"
    elif echo "$REQUEST_URI" | grep -q "action=stop"; then
        action="stop"
    elif echo "$REQUEST_URI" | grep -q "action=restart"; then
        action="restart"
    elif echo "$REQUEST_URI" | grep -q "action=upgrade_status"; then
        action="upgrade_status"
    elif echo "$REQUEST_URI" | grep -q "action=upgrade_logs"; then
        action="upgrade_logs"
    elif echo "$REQUEST_URI" | grep -q "action=upgrade"; then
        action="upgrade"
    elif echo "$REQUEST_URI" | grep -q "action=version_list"; then
        action="version_list"
    elif echo "$REQUEST_URI" | grep -q "action=change_port"; then
        action="change_port"
    elif echo "$REQUEST_URI" | grep -q "action=logs"; then
        action="logs"
    elif echo "$REQUEST_URI" | grep -q "action=clear_logs"; then
        action="clear_logs"
    elif echo "$REQUEST_URI" | grep -q "action=reset_auth"; then
        action="reset_auth"
    elif echo "$REQUEST_URI" | grep -q "action=backup_download"; then
        action="backup_download"
    elif echo "$REQUEST_URI" | grep -q "action=check_tunnel"; then
        action="check_tunnel"
    fi
fi

case "$action" in
    status)
        status
        ;;
    start)
        start_service
        ;;
    stop)
        stop_service
        ;;
    restart)
        restart_service
        ;;
    upgrade)
        upgrade_service
        ;;
    version_list)
        get_version_list
        ;;
    change_port)
        change_port
        ;;
    upgrade_status)
        get_upgrade_status
        ;;
    upgrade_logs)
        get_upgrade_logs
        ;;
    logs)
        get_logs
        ;;
    clear_logs)
        clear_logs
        ;;
    reset_auth)
        reset_auth
        ;;
    backup_download)
        backup_download
        ;;
    check_tunnel)
        check_tunnel_installed
        ;;
    *)
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"无效的操作\"}"
        ;;
esac