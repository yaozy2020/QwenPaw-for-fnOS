#!/bin/bash

# 脚本名称: index.cgi
# 　　版本: 1.0.1
# 　　作者: FNOSP/xieguanru
# 　协作者: FNOSP/MR_XIAOBO
# 创建日期: 2025-11-18
# 最后修改: 2025-11-19
# 　　描述: 这个脚本用于演示 Shell 脚本的各种注释方式
# 使用方式: 文件重命名，从 linux_shell_cgi_index.sh 改成 index.cgi,
# 　　　　　放置应用包 /ui 路径下，记得 chmod +x index.cgi 赋权
# 　许可证: MIT

# 【注意】静态文件根目录。优先用平台注入的 TRIM_APPDEST,
# 否则在非标准安装布局(本地调试/镜像重定位)下会指向不存在的路径。
BASE_PATH="${TRIM_APPDEST:-/var/apps/qwenpaw/target}/www"

# 1. 从 REQUEST_URI 里拿到 index.cgi 后面的路径
#    例如：/cgi/ThirdParty/qwenpaw/index.cgi/index.html?foo=bar   
#    先去掉 ? 后面的 query string
URI_NO_QUERY="${REQUEST_URI%%\?*}"

# 默认值（如果没匹配到 index.cgi）
REL_PATH="/"

# 用 index.cgi 作为切割点，取后面的部分
case "$URI_NO_QUERY" in
    *index.cgi*)
        # 去掉前面所有直到 index.cgi 为止的内容，保留后面的
        # /cgi/ThirdParty/qwenpaw/index.cgi/index.html -> /index.html
        REL_PATH="${URI_NO_QUERY#*index.cgi}"
        ;;
esac

# 如果为空或只有 /，就默认 /index.html
if [ -z "$REL_PATH" ] || [ "$REL_PATH" = "/" ]; then
    REL_PATH="/index.html"
fi

# 拼出真实文件路径：basePath + /ui + index.cgi 后面的路径
TARGET_FILE="${BASE_PATH}${REL_PATH}"

# 简单防御：禁止 .. 越级访问
if echo "$TARGET_FILE" | grep -q '\.\.'; then
    echo "Status: 400 Bad Request"
    echo "Content-Type: text/plain; charset=utf-8"
    echo ""
    echo "Bad Request"
    exit 0
fi

# 2. 判断文件是否存在
if [ ! -f "$TARGET_FILE" ]; then
    echo "Status: 404 Not Found"
    echo "Content-Type: text/plain; charset=utf-8"
    echo "X-Content-Type-Options: nosniff"
    echo ""
    # REL_PATH 完全来自请求,原样回显等于把攻击者可控的任意长字符串
    # (可含换行)写进响应体。截断并去掉控制字符。
    safe_path=$(printf '%s' "$REL_PATH" | tr -d '\000-\037\177' | cut -c1-200)
    echo "404 Not Found: ${safe_path}"
    exit 0
fi

# 3. 根据扩展名简单判断 Content-Type
ext="${TARGET_FILE##*.}"
case "$ext" in
    html|htm)
        mime="text/html; charset=utf-8"
        ;;
    css)
        mime="text/css; charset=utf-8"
        ;;
    js)
        mime="application/javascript; charset=utf-8"
        ;;
    jpg|jpeg)
        mime="image/jpeg"
        ;;
    png)
        mime="image/png"
        ;;
    gif)
        mime="image/gif"
        ;;
    svg)
        mime="image/svg+xml"
        ;;
    txt|log)
        mime="text/plain; charset=utf-8"
        ;;
    *)
        mime="application/octet-stream"
        ;;
esac

# 3.5 Accept-Encoding 协商（RFC 9110）：客户端可解压时优先发预压缩的 .br / .gz
#     规则与 DSH 2026-10 实践一致：显式条目优先于 *（gzip;q=0, * 仍是拒绝），
#     q 值判断（q=0 拒绝），大小写不敏感（上方已统一小写）
AE="$(printf '%s' "${HTTP_ACCEPT_ENCODING:-}" | tr -d ' ' | tr 'A-Z' 'a-z' | tr ',' '\n')"

# q_of <token>：该编码的 q 值；无显式条目输出空（由调用方决定是否回看 *）
q_of() {
    printf '%s\n' "$AE" | awk -F';' -v t="$1" '
        $1 == t {
            q = "1"
            if (NF > 1) { split($2, kv, "="); if (kv[1] == "q" && kv[2] != "") q = kv[2] }
            print q
            exit
        }'
}

# q_positive <q>：q 值是否 > 0
q_positive() {
    [ -n "$1" ] || return 1
    awk -v q="$1" 'BEGIN{exit !((q+0)>0)}'
}

# accepts <token>：显式条目说了算；没有显式条目才回看 *（q=0 拒绝）
accepts() {
    q="$(q_of "$1")"
    if [ -n "$q" ]; then q_positive "$q"; return $?; fi
    q_positive "$(q_of '*')"
}

# CSRF token: 访问 html 时若客户端尚未携带 cookie 则签发 SameSite=Lax cookie。
# 现已改为纯 HTTP 标头传递与前端读取 document.cookie, 不修改 HTML 正文,
# 因此完全兼容 brotli/gzip 预压缩传输(保持首屏从 57KB 压缩到 16KB 性能优势)。
CSRF_NEW=0
if [ "$ext" = "html" ] || [ "$ext" = "htm" ]; then
    CSRF_TOKEN="$(printf '%s\n' "${HTTP_COOKIE:-}" | tr ';' '\n' | sed 's/^ *//' | grep '^qwenpaw_csrf=' | head -1 | cut -d= -f2-)"
    if [ -z "$CSRF_TOKEN" ]; then
        CSRF_TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
        CSRF_NEW=1
    fi
fi

SEND_FILE="$TARGET_FILE"
CONTENT_ENCODING=""
if [ -f "${TARGET_FILE}.br" ] && accepts br; then
    SEND_FILE="${TARGET_FILE}.br"
    CONTENT_ENCODING="br"
elif [ -f "${TARGET_FILE}.gz" ] && accepts gzip; then
    SEND_FILE="${TARGET_FILE}.gz"
    CONTENT_ENCODING="gzip"
fi

# 4. 输出头 + 文件内容
echo "Content-Type: $mime"
echo "Vary: Accept-Encoding"
if [ -n "$CONTENT_ENCODING" ]; then
    echo "Content-Encoding: $CONTENT_ENCODING"
fi
if [ "$ext" = "html" ] || [ "$ext" = "htm" ]; then
    echo "Cache-Control: no-cache, no-store, must-revalidate, max-age=0"
    echo "Pragma: no-cache"
    echo "Expires: 0"
    if [ "$CSRF_NEW" -eq 1 ]; then
        echo "Set-Cookie: qwenpaw_csrf=${CSRF_TOKEN}; Path=/; SameSite=Lax"
    fi
fi
echo ""

# 静态内容直接输出(支持协商压缩 br/gz 零篡改直出)
cat "$SEND_FILE"