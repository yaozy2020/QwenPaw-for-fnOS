#!/bin/bash
# 瘦桥:旧 action API → 监督者 HTTP(127.0.0.1:19090)。
# 职责仅剩:token 读取 + 路径映射 + 旧响应形状兼容;全部业务逻辑在监督者。

SUP="http://127.0.0.1:19090"
VAR_DIR="${TRIM_PKGVAR:-/var/apps/qwenpaw/var}"
TOKEN_FILE="${VAR_DIR}/supervisor-token"

TOKEN=$(tr -d '[:space:]' < "${TOKEN_FILE}" 2>/dev/null)

# query 参数提取
qget() {  # $1=key → 输出值
    echo "$QUERY_STRING" | tr '&' '\n' | grep "^${1}=" | head -1 | cut -d= -f2- | tr -d '+'
}

action=$(echo "$QUERY_STRING" | tr '&' '\n' | grep '^action=' | head -1 | cut -d= -f2-)

method="GET"
path="/status"
body=""

case "$action" in
    start|stop|restart)
        method="POST"
        path="/$action"
        ;;
    logs)
        lines=$(qget lines)
        path="/logs?tail=${lines:-80}"
        ;;
    clear_logs)
        method="POST"
        path="/logs/clear"
        ;;
    upgrade)
        method="POST"
        path="/kernel/install"
        ver=$(qget version)
        mir=$(qget mirror)
        body="{\"version\":\"${ver:-latest}\",\"mirror\":\"${mir}\"}"
        ;;
    upgrade_status)
        # 旧形状:{success, upgrading} —— 从监督者快照的 install.running 转换
        method="GET"
        path="/status"
        ;;
    upgrade_logs)
        # pip 日志尾部(监督者落盘文件,同用户直接读)
        PIP_LOG="${VAR_DIR}/qwenpaw-upgrade.log.pip"
        echo "Content-Type: application/json"
        echo ""
        logs=$(tail -n 40 "${PIP_LOG}" 2>/dev/null | tr '\n' '\001' | sed 's/\x01/\\n/g')
        echo "{\"success\":true,\"logs\":[\"${logs}\"]}"
        exit 0
        ;;
    version_list)
        path="/kernel/versions"
        ;;
    reset_auth)
        method="POST"
        path="/kernel/reset-auth"
        ;;
    change_port)
        method="POST"
        path="/port"
        newp=$(qget port)
        body="{\"port\":${newp:-18088}}"
        ;;
    backup_download|check_tunnel)
        echo "Content-Type: application/json"
        echo ""
        echo "{\"success\":false,\"message\":\"功能迁移中,后续版本恢复\"}"
        exit 0
        ;;
esac

# 转发到监督者
resp=$(curl -s --max-time 30 -X "$method" \
    -H "X-Supervisor-Token: ${TOKEN}" \
    -H "Content-Type: application/json" \
    ${body:+-d "$body"} \
    "${SUP}${path}")

# upgrade_status:旧形状转换(控制台轮询升级标志)
if [ "$action" = "upgrade_status" ]; then
    upgrading=$(echo "$resp" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    print('true' if d.get('install', {}).get('running') else 'false')
except Exception:
    print('false')
" 2>/dev/null)
    echo "Content-Type: application/json"
    echo ""
    echo "{\"success\":true,\"upgrading\":${upgrading:-false}}"
    exit 0
fi

echo "Content-Type: application/json"
echo ""
echo "${resp:-{\"success\":false,\"message\":\"监督者未响应,请稍后重试\"}}"
