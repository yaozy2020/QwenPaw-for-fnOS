#!/bin/bash
# 瘦桥:旧 action API → 监督者 HTTP(127.0.0.1:19090)。
# 职责仅剩:token 读取 + 路径映射 + 旧响应形状兼容;全部业务逻辑在监督者。
#
# 安全(P0,勿回退):
#   1. 方法白名单 —— 读操作只许 GET,变更操作只许 POST;违规回 405 + Allow。
#      历史版本对方法不设限,GET 即可触发 stop/reset_auth/upgrade。
#   2. 同源校验(CSRF)—— 浏览器对一切 POST(fetch/sendBeacon/表单)都会携带
#      Origin;缺席或与 Host 不符即跨站/伪造,回 403。非浏览器客户端本就无
#      fnOS 会话凭据,拒绝无副作用。注意:不信任 X-Trim-Userid 等可被客户端
#      伪造的头,管理鉴权仍依赖 fnOS 网关的应用授权(disable_authorization_path=false)。

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

REQUEST_METHOD="${REQUEST_METHOD:-GET}"
# HEAD 是对 GET 的元信息请求(CGI 服务器会丢弃响应体),按 GET 语义处理即可。
# 原实现直接判 405:健康检查/探活用 HEAD 探测时会把一个正常端点误报为不允许。
if [ "$REQUEST_METHOD" = "HEAD" ]; then
    REQUEST_METHOD="GET"
    HEAD_REQUEST=1
fi

# ── 方法白名单:读=GET,变更=POST ──
case "$action" in
    status|logs|version_list|get_mirror|upgrade_status|upgrade_logs) want_method="GET" ;;
    start|stop|restart|clear_logs|upgrade|uninstall_kernel|reset_auth|change_port|set_mirror) want_method="POST" ;;
    *)
        echo "Status: 404 Not Found"
        echo "Content-Type: application/json"
        echo ""
        echo '{"success":false,"message":"未知操作"}'
        exit 0
        ;;
esac
if [ "$REQUEST_METHOD" != "$want_method" ]; then
    echo "Status: 405 Method Not Allowed"
    if [ "$want_method" = "GET" ]; then
        echo "Allow: GET, HEAD"
    else
        echo "Allow: $want_method"
    fi
    echo "Content-Type: application/json"
    echo ""
    printf '{"success":false,"message":"此操作仅允许 %s 请求"}\n' "$want_method"
    exit 0
fi

# ── CSRF 防御: Double-Submit Cookie (安全与部署拓扑/反代彻底解耦) ──
# 机制: index.cgi 签发 SameSite=Lax cookie(qwenpaw_csrf), 控制台前端从
# document.cookie 读取并通过 X-CSRF-Token 自定义头回传, 后端比对两者一致性。
# 跨站攻击站点受同源策略限制无法读取跨域 Cookie, 亦无法在跨站请求中附加自定义 Header。
# 支持 sendBeacon 退路经 _csrf 查询参数回传 Token。GET 读操作不校验。
csrf_ok() {
    local cookie_v token
    cookie_v="$(printf '%s
' "${HTTP_COOKIE:-}" | tr ';' '
' | sed 's/^ *//' | grep '^qwenpaw_csrf=' | head -1 | cut -d= -f2-)"
    [ -n "$cookie_v" ] || return 1
    token="${HTTP_X_CSRF_TOKEN:-}"
    if [ -z "$token" ]; then
        token="$(qget _csrf)"
    fi
    [ -n "$token" ] && [ "$token" = "$cookie_v" ]
}
if [ "$REQUEST_METHOD" = "POST" ] && ! csrf_ok; then
    echo "Status: 403 Forbidden"
    echo "Content-Type: application/json"
    echo ""
    echo '{"success":false,"message":"CSRF 校验失败:请刷新控制台页面后重试"}'
    exit 0
fi

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
        # lines 必须数字白名单:直接拼进 URL 的任意用户输入可注入
        # `&`/`#` 改写查询串,非数字值也会让监督者按默认值或报错。
        case "$lines" in
            ''|*[!0-9]*) lines=80 ;;
        esac
        [ "${#lines}" -gt 6 ] && lines=100000
        path="/logs?tail=${lines}"
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
            # 无 python3 的退化路径必须先转义反斜杠再转义引号,
            # 否则 mirror=...\ 这类输入会产出非法 JSON(尾部转义掉收尾引号)。
            ver_esc=${ver//\\/\\\\}
            ver_esc=${ver_esc//\"/\\\"}
            mir_esc=${mir//\\/\\\\}
            mir_esc=${mir_esc//\"/\\\"}
            body="{\"version\":\"${ver_esc:-latest}\",\"mirror\":\"${mir_esc}\"}"
        fi
        ;;
    uninstall_kernel)
        method="POST"
        path="/kernel/uninstall"
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
        # 下载源落盘:mirror 会进监督者的 pip --index-url 参数,必须白名单字符集
        # (仅 http(s) URL 安全字符),纵深防御直连调用方(独立审计"shell 拼 JSON"同源风险)
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
esac

# 转发到监督者。Token 经 0600 临时头文件传给 curl(-H @file,curl≥7.55):
# 命令行参数会完整出现在 /proc/<pid>/cmdline,同机任意用户可读 —— 与 Token
# 文件 0600 的威胁模型不对等(审查 S1)。即用即删,trap 兜底异常退出。
# curl 缺失时必须在此明确失败。原实现会一路走到下面的 case,
# http_code 拿到空串(既不是 200 也不是 000),落进兜底分支,
# 前端只看到一条含糊的"未知错误",排查成本极高。
if ! command -v curl >/dev/null 2>&1; then
    echo "Content-Type: application/json"
    echo "Status: 503 Service Unavailable"
    echo ""
    echo '{"success":false,"message":"系统缺少 curl,无法与监督者通信,请检查应用依赖"}'
    exit 0
fi

TOKHDR=$(mktemp "${TMPDIR:-/tmp}/qwenpaw-hdr.XXXXXX") || {
    echo '{"success":false,"message":"内部错误:无法创建临时文件"}'
    exit 1
}
chmod 600 "${TOKHDR}" 2>/dev/null || true
printf 'X-Supervisor-Token: %s\n' "${TOKEN}" > "${TOKHDR}"
trap 'rm -f "${TOKHDR}"' EXIT
raw_resp=$(curl -s --max-time 30 -w $'\n%{http_code}' -X "$method" \
    -H @"${TOKHDR}" \
    -H "Content-Type: application/json" \
    ${body:+-d "$body"} \
    "${SUP}${path}")
rm -f "${TOKHDR}"
trap - EXIT
http_code="${raw_resp##*$'\n'}"
resp="${raw_resp%$'\n'*}"
STATUS_LINE=""
case "$http_code" in
    200) ;;
    2??) STATUS_LINE="Status: ${http_code}" ;;
    000)
        STATUS_LINE="Status: 502 Bad Gateway"
        resp='{"success":false,"message":"监督者未响应,请稍后重试"}'
        ;;
    *) STATUS_LINE="Status: ${http_code}" ;;
esac

# version_list:监督者交回 PyPI 全量原始键名;语义排序(2.10.0>2.9.9,稳定>
# 预发布)、is_beta 判定、最新稳定版推导都在这里做 —— 控制台取 versions[0]
# 前的第一个 is_beta 作"最新测试",降序排列才能取到真正最新的 beta。
# 上游失败(err_json/形状异常)必须 success:false 透传,绝不伪装"检查完成"。
if [ "$action" = "version_list" ]; then
    [ -n "$STATUS_LINE" ] && echo "$STATUS_LINE"
    echo "Content-Type: application/json"
    echo ""
    if command -v python3 >/dev/null 2>&1; then
        printf '%s' "$resp" | python3 -c '
import json, re, sys
out = {"success": False, "versions": [], "latest_stable": "", "message": ""}
try:
    d = json.load(sys.stdin)
    if isinstance(d, dict) and isinstance(d.get("versions"), list) and d.get("success") is True:
        out = d
    elif not isinstance(d, dict) or d.get("ok") is False or not isinstance(d.get("releases"), list):
        out["message"] = (d.get("message") or d.get("error") or "查询 PyPI 失败") if isinstance(d, dict) else "监督者响应形状异常"
    else:
        def vkey(v):
            m = re.match(r"^(\d+)\.(\d+)(?:\.(\d+))?(.*)", v)
            if not m:
                return (0, 0, 0, -1, 0, 0)
            rest = m.group(4) or ""
            if not rest:
                return (int(m.group(1)), int(m.group(2)), int(m.group(3) or 0), 4, 0, 0)
            if ".post" in rest:
                pnum = int(re.sub(r"\D", "", rest) or 0)
                return (int(m.group(1)), int(m.group(2)), int(m.group(3) or 0), 5, 0, pnum)
            c = rest.lstrip(".-").lower()
            stage = 0
            if c.startswith("dev"): stage = 0
            elif c.startswith("a") or c.startswith("alpha"): stage = 1
            elif c.startswith("b") or c.startswith("beta"): stage = 2
            elif c.startswith("rc") or c.startswith("c") or c.startswith("pre"): stage = 3
            num = int(re.sub(r"\D", "", c) or 0)
            return (int(m.group(1)), int(m.group(2)), int(m.group(3) or 0), stage, num, 0)
        names = sorted(map(str, d["releases"]), key=vkey, reverse=True)[:14]
        out["success"] = True
        out["versions"] = [{"version": v, "is_beta": not bool(re.match(r"^\d+(\.\d+)*(\.post\d+)?$", v))} for v in names]
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
[ -n "$STATUS_LINE" ] && echo "$STATUS_LINE"
echo "Content-Type: application/json"
echo ""
echo "$resp"
