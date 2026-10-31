#!/bin/bash
# 瘦桥:旧 action API → 监督者 HTTP(127.0.0.1:19090)。
# 职责仅剩:token 读取 + 路径映射 + 旧响应形状兼容;全部业务逻辑在监督者。

SUP="${QWENPAW_SUP_URL:-http://127.0.0.1:19090}"  # 可注入假地址供 lint 测兜底路径
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

# version_list:监督者吐 PyPI 原始形状 {releases,latest_stable};
# 控制台吃旧形状 {success,versions:[{version,is_beta}],latest_stable} —— 桥接转换
if [ "$action" = "version_list" ]; then
    echo "Content-Type: application/json"
    echo ""
    printf '%s' "$resp" | python3 -c '
import json, re, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(json.dumps({"success": False, "versions": [], "latest_stable": ""}))
    raise SystemExit
vers = [{"version": v, "is_beta": bool(re.search(r"(?:a|b|rc)[0-9]", v))}
        for v in d.get("releases", [])]
print(json.dumps({"success": True, "versions": vers, "latest_stable": d.get("latest_stable", "")}))
'
    exit 0
fi

# 非 JSON 防御:监督者未响应或返回纯文本(如 invalid token)时,一律输出兜底对象。
# 纪律:绝不透传非 JSON —— 否则控制台 res.json() 抛异常,所有动态渲染静默失效,
# 页面停在 HTML 静态默认值(运行中 + PID "-"),看似活着实则全断(r7 闭环教训)。
case "$resp" in
    \{*) : ;;
    *) resp='{"success":false,"message":"监督者未响应,请稍后重试"}' ;;
esac
echo "Content-Type: application/json"
echo ""
echo "$resp"
