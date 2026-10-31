#!/bin/bash
# 瘦桥:旧 action API → 监督者 HTTP(127.0.0.1:19090)。
# 职责仅剩:token 读取 + 路径映射 + 旧响应形状兼容;全部业务逻辑在监督者。

SUP="${QWENPAW_SUP_URL:-http://127.0.0.1:19090}"  # 可注入假地址供 lint 测兜底路径
VAR_DIR="${TRIM_PKGVAR:-/var/apps/qwenpaw/var}"
TOKEN_FILE="${VAR_DIR}/supervisor-token"

TOKEN=$(tr -d '[:space:]' < "${TOKEN_FILE}" 2>/dev/null)

# query 参数提取。%XX 必须解码(encodeURIComponent 发来的 %2B 即 '+',PEP440
# 本地版本段);'+' 保留原样不转空格。%b 会解释 \n 等序列 —— 版本号/端口/行数
# 的合法取值不含反斜杠,可接受(审计 minor:原 tr -d '+' 曾把 +cpu 剥成 cpu)。
urldecode() {
    printf '%b' "${1//%/\\x}"
}
qget() {  # $1=key → 输出值
    urldecode "$(echo "$QUERY_STRING" | tr '&' '\n' | grep "^${1}=" | head -1 | cut -d= -f2-)"
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
        # mirror 优先级:显式 mirror= 参数(可为空,控制台总是携带) > 服务端已存
        # 下载源(VAR_DIR/pip-mirror,set_mirror 落盘) > 空(pip 默认官方)
        if echo "$QUERY_STRING" | tr '&' '\n' | grep -q '^mirror='; then
            mir=$(qget mirror)
        elif [ -f "${VAR_DIR}/pip-mirror" ]; then
            mir=$(head -n 1 "${VAR_DIR}/pip-mirror" | tr -d '[:space:]')
        else
            mir=""
        fi
        # JSON 体经 python3 构造,杜绝 shell 拼串注入(审计 nit);无 python3 时退化转义
        if command -v python3 >/dev/null 2>&1; then
            body=$(python3 -c 'import json,sys; print(json.dumps({"version": sys.argv[1] or "latest", "mirror": sys.argv[2]}))' "${ver:-latest}" "${mir}")
        else
            body="{\"version\":\"${ver//\"/\\\"}\",\"mirror\":\"${mir//\"/\\\"}\"}"
        fi
        ;;
    upgrade_status)
        # 监督者轻端点直出旧形状,桥零转换(曾每轮询起一个 python3,卡顿源之一)
        method="GET"
        path="/kernel/install-status"
        ;;
    upgrade_logs)
        # 监督者轻端点直出(Rust 侧 JSON 转义),桥零转换零 python3
        method="GET"
        path="/kernel/install-log?tail=40"
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
        case "$newp" in
            ''|*[!0-9]*)
                # 端口进 JSON 数字位,非纯数字一律拒绝(审计 nit)
                echo "Content-Type: application/json"
                echo ""
                echo '{"success":false,"message":"端口参数非法,需为纯数字"}'
                exit 0
                ;;
        esac
        body="{\"port\":${newp}}"
        ;;
    set_mirror)
        # 下载源落盘:mirror 会进入监督者的 bash -c pip 命令串,必须白名单字符集
        # (仅 http(s) URL 安全字符),杜绝 shell 注入(独立审计"shell 拼 JSON"同源风险)
        m=$(qget mirror)
        ok=1
        case "$m" in
            "") : ;;
            https://*|http://*) case "$m" in *[!A-Za-z0-9:/._~-]*) ok=0 ;; esac ;;
            *) ok=0 ;;
        esac
        if [ "$ok" -ne 1 ]; then
            echo "Content-Type: application/json"
            echo ""
            echo '{"success":false,"message":"下载源需为 http(s):// 开头且仅含 URL 安全字符的地址"}'
            exit 0
        fi
        printf '%s\n' "$m" > "${VAR_DIR}/pip-mirror" 2>/dev/null \
            && out='{"success":true,"message":"下载源已保存"}' \
            || out='{"success":false,"message":"下载源写入失败(权限?)"}'
        echo "Content-Type: application/json"
        echo ""
        echo "$out"
        exit 0
        ;;
    get_mirror)
        m=""
        [ -f "${VAR_DIR}/pip-mirror" ] && m=$(head -n 1 "${VAR_DIR}/pip-mirror" | tr -d '[:space:]')
        echo "Content-Type: application/json"
        echo ""
        printf '{"success":true,"mirror":"%s"}\n' "$m"
        exit 0
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

# version_list:监督者交回 PyPI 全量原始键名;语义排序(2.10.0>2.9.9,稳定>
# 预发布)、is_beta 判定、最新稳定版推导都在这里做 —— 控制台取 versions[0]
# 前的第一个 is_beta 作"最新测试",降序排列才能取到真正最新的 beta。
# 上游失败(err_json/形状异常)必须 success:false 透传,绝不伪装"检查完成"。
if [ "$action" = "version_list" ]; then
    echo "Content-Type: application/json"
    echo ""
    if command -v python3 >/dev/null 2>&1; then
        printf '%s' "$resp" | python3 -c '
import json, re, sys
out = {"success": False, "versions": [], "latest_stable": "", "message": ""}
try:
    d = json.load(sys.stdin)
    if not isinstance(d, dict) or d.get("ok") is False or not isinstance(d.get("releases"), list):
        out["message"] = (d.get("message") or d.get("error") or "查询 PyPI 失败") if isinstance(d, dict) else "监督者响应形状异常"
    else:
        def vkey(v):
            m = re.match(r"^(\d+)\.(\d+)(?:\.(\d+))?(.*)", v)
            if not m:
                return (0, 0, 0, -1, v)
            rest = m.group(4) or ""
            rank = 1 if (rest == "" or rest.startswith(".post")) else 0
            return (int(m.group(1)), int(m.group(2)), int(m.group(3) or 0), rank, rest)
        names = sorted(map(str, d["releases"]), key=vkey, reverse=True)[:14]
        out["success"] = True
        out["versions"] = [{"version": v, "is_beta": bool(re.search(r"(?:a|b|rc)[0-9]", v))} for v in names]
        out["latest_stable"] = next(
            (v for v in names if re.match(r"^\d+(\.\d+)*(\.post\d+)?$", v)),
            str(d.get("latest_stable", "")),
        )
except Exception as e:
    out["message"] = "解析失败: %s" % e
print(json.dumps(out))
'
    else
        echo '{"success":false,"versions":[],"latest_stable":"","message":"环境缺少 python3,无法解析版本列表"}'
    fi
    exit 0
fi

# 非 JSON 防御:监督者未响应/返回纯文本/JSON 被截断,一律替换为兜底对象,
# 绝不透传 —— 否则控制台 res.json() 抛异常,所有动态渲染静默失效,页面停在
# HTML 静态默认值(r7 闭环事故)。有 python3 时做完整 json.loads 校验(审计
# minor:仅查首字符拦不住截断 JSON);无 python3 退化为首尾花括号启发式。
if command -v python3 >/dev/null 2>&1; then
    if ! printf '%s' "$resp" | python3 -c 'import json,sys; json.load(sys.stdin)' >/dev/null 2>&1; then
        resp='{"success":false,"message":"监督者未响应,请稍后重试"}'
    fi
else
    case "$resp" in
        \{*) : ;;
        *) resp='{"success":false,"message":"监督者未响应,请稍后重试"}' ;;
    esac
fi
echo "Content-Type: application/json"
echo ""
echo "$resp"
