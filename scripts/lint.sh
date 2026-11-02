#!/bin/bash
# lint.sh — 语法与静态检查门禁（六层）
#   [1/5] bash -n 全量
#   [2/5] shellcheck（error 级为门禁，warning 仅报告；本地无二进制时自动下载 pinned 版本）
#   [3/5] 内嵌 spawn 脚本展开验证 —— api.cgi 中以 nohup bash -c "..." / local cmd="..."
#         形式内嵌的启动脚本是"运行时展开"代码，bash -n 与 shellcheck 都查不出其中的
#         变量误展开（历史事故：awk $2 被 nohup 外层展开成空），必须按真实展开语义
#         生成内层脚本后单独验证，并断言每条启动路径都携带内核启动必需的环境变量。
#   [4/5] api.cgi 桥接输出 JSON 有效性 —— 控制台所有动态渲染依赖 res.json() 成功；
#         桥接只要吐出一段非 JSON（历史事故：echo "${resp:-{...}}" 默认词里的 }
#         提前终止参数展开，输出 "{...}}"），前端就全体静默失效停在静态默认值。
#   [6/6] 监督者生命周期回归（scripts/test-lifecycle.sh）：陈旧/复用 PID 不得误杀
#         无关进程（P1-01）、外部停止宽限覆盖内部最坏清理（P1-03）。
# 用法: bash scripts/lint.sh
# 注意: 不用 set -e（工程铁律），失败显式 exit 1

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHELLCHECK_VER="0.10.0"
fail=0

echo "========== lint: 语法与静态检查 =========="

# ── [1/3] bash -n 全量 ──
echo "[1/5] bash -n ..."
for f in "$ROOT"/cmd/* "$ROOT"/app/ui/*.cgi "$ROOT"/build.sh "$ROOT"/scripts/*.sh; do
    [ -f "$f" ] || continue
    if bash -n "$f" 2>/dev/null; then
        echo "  [  OK ] ${f#"$ROOT"/}"
    else
        echo "  [FAIL] ${f#"$ROOT"/}"
        bash -n "$f"
        fail=1
    fi
done

# 全局删除门禁：生命周期回调只允许删除 TRIM_* 约束内的路径。
# 历史事故：测试以临时 TRIM_DATA_SHARE_PATHS 运行 uninstall_callback，
# 回调内硬编码的全局路径删除绕过了隔离，误删真实应用数据。
echo "  [gate] uninstall_callback 全局路径删除检查 ..."
if grep -nE '[^#]*rm[[:space:]]+-rf[[:space:]]+"/?vol' "$ROOT/cmd/uninstall_callback" >/dev/null; then
    echo "  [FAIL] cmd/uninstall_callback 含硬编码 /vol* 删除（会绕过 TRIM_* 隔离）"
    fail=1
else
    echo "  [  OK ] 无硬编码 /vol* 删除"
fi

# 产物签名门禁：.sha256 内必须是纯文件名。
# 历史事故：build.sh 用 sha256sum <绝对路径> 生成签名，哈希值对但文件名部分
# 带上了构建机的绝对路径，用户下载后 `sha256sum -c` 因该路径不存在而失败。
echo "  [gate] build.sh 的 .sha256 必须携带纯文件名 ..."
if grep -qE 'sha256sum[[:space:]]+"\$OUTPUT"' "$ROOT/build.sh"; then
    echo "  [FAIL] build.sh 直接对 \$OUTPUT(绝对路径) 调用 sha256sum，签名里会带构建机路径"
    fail=1
else
    echo "  [  OK ] .sha256 以纯文件名生成"
fi

# ── [2/3] shellcheck（error 级门禁；warning 仅报告不拦截） ──
echo "[2/5] shellcheck v${SHELLCHECK_VER} ..."
SC_BIN="$(command -v shellcheck 2>/dev/null || true)"
if [ -z "$SC_BIN" ]; then
    CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/qwenpaw-build"
    mkdir -p "$CACHE_DIR"
    SC_BIN="$CACHE_DIR/shellcheck"
    if [ ! -x "$SC_BIN" ]; then
        TARBALL="$CACHE_DIR/shellcheck-${SHELLCHECK_VER}.tar.xz"
        if curl -sL --max-time 120 -o "$TARBALL" \
            "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VER}/shellcheck-v${SHELLCHECK_VER}.linux.x86_64.tar.xz" \
            && tar -xJf "$TARBALL" -C "$CACHE_DIR" "shellcheck-v${SHELLCHECK_VER}/shellcheck" 2>/dev/null; then
            mv "$CACHE_DIR/shellcheck-v${SHELLCHECK_VER}/shellcheck" "$SC_BIN"
            chmod +x "$SC_BIN"
            rm -f "$TARBALL"
        fi
    fi
fi
if [ -n "$SC_BIN" ] && [ -x "$SC_BIN" ]; then
    # lib.sh 是公共函数库,此前只被 bash -n 语法检查过,没进 shellcheck;
    # 四个 *_init 与两个门禁脚本同样遗漏。
    TARGETS=(cmd/main cmd/lib.sh cmd/supervisor_ctl.sh cmd/install_callback cmd/upgrade_callback cmd/config_callback cmd/uninstall_callback cmd/install_init cmd/upgrade_init cmd/config_init cmd/uninstall_init app/ui/api.cgi app/ui/index.cgi build.sh scripts/preflight.sh scripts/audit-fpk.sh scripts/test-lifecycle.sh scripts/test-venv-recovery.sh)
    if "$SC_BIN" -S error "${TARGETS[@]/#/"$ROOT/"}" 2>&1; then
        echo "  [  OK ] error 级问题为 0（warning 级见上方输出，不拦截）"
    else
        echo "  [FAIL] shellcheck error 级问题需修复"
        fail=1
    fi
else
    if [ "${STRICT_CI:-0}" = "1" ]; then
        echo "  [FAIL] STRICT_CI 模式下不允许跳过 shellcheck 门禁"
        fail=1
    else
        echo "  [SKIP] shellcheck 不可用（网络/二进制缺失），跳过本层"
    fi
fi

# ── [3/4] 内嵌 spawn 脚本展开验证 ──
# v2 起 spawn 逻辑迁入监督者(kernel.rs process_group),瘦桥无内嵌启动脚本,
# 本层常态提取 0 段 —— 保留作哨兵:若未来重新出现内嵌启动脚本即自动激活。
echo "[3/5] 内嵌 spawn 脚本展开验证 ..."
API_CGI="$ROOT/app/ui/api.cgi"
WORK="$(mktemp -d)"

python3 - "$API_CGI" "$WORK/spawn_harness.sh" "$WORK/names.txt" <<'PY'
import re, sys

src = open(sys.argv[1]).read().splitlines()
harness_path, names_path = sys.argv[2], sys.argv[3]
out, names = [], []
i, n = 0, 0
while i < len(src):
    line = src[i]
    if re.match(r'\s*nohup bash -c "$', line):
        n += 1
        name = f'inner_{n}'
        names.append(name)
        out.append(f'{name}="')
        i += 1
        while i < len(src):
            s = src[i]
            stripped = s.strip()
            # 收尾行形如 `" &` 或 `" >/dev/null 2>&1 &`（change_port 块）
            if stripped.startswith('"') and stripped.endswith('&'):
                out.append('"')
                i += 1
                break
            out.append(s)
            i += 1
        continue
    m = re.match(r'\s*local cmd="(.*)"\s*$', line)
    if m:
        n += 1
        name = f'cmd_{n}'
        names.append(name)
        out.append(f'{name}="{m.group(1)}"')
        i += 1
        continue
    i += 1

vars_lines = [l for l in src if re.match(r'^[A-Z_]+="', l)]
stub = [
    'get_service_port(){ echo 18088; }',
    'check_process(){ return 0; }',
    'upgrade_cmd="true"',
    'new_port="18099"',
]
open(harness_path, 'w').write('\n'.join(vars_lines + stub + out) + '\n')
open(names_path, 'w').write('\n'.join(names) + '\n')
print(f'  提取内嵌 spawn 脚本 {n} 段')
PY

if [ ! -f "$WORK/spawn_harness.sh" ]; then
    echo "  [FAIL] 内嵌脚本提取失败"
    exit 1
fi

# 在同一 shell 内 source（子进程变量传不回来），逐段展开验证
source "$WORK/spawn_harness.sh"
while read -r vname; do
    [ -z "$vname" ] && continue
    script="$(eval "printf '%s' \"\${$vname}\"")"
    label="$vname"
    if [ -z "$script" ]; then
        echo "  [FAIL] $label 展开为空"
        fail=1
        continue
    fi
    if ! bash -n <<< "$script" 2>/dev/null; then
        echo "  [FAIL] $label 展开后语法错误:"
        bash -n <<< "$script"
        fail=1
    fi
    case "$script" in
        *QWENPAW_DEPENDENCY_STATE_DIR*) : ;;
        *)
            echo "  [FAIL] $label 缺少 QWENPAW_DEPENDENCY_STATE_DIR（内核 b2+ 启动必需）"
            fail=1
            ;;
    esac
    if [ $fail -eq 0 ]; then
        echo "  [  OK ] $label（展开后 $(printf '%s' "$script" | wc -c) 字符）"
    fi
done < "$WORK/names.txt"

# ── [4/4] api.cgi 桥接输出 JSON 有效性 ──
echo "[4/5] api.cgi 桥接 JSON 有效性 ..."
BR="$ROOT/app/ui/api.cgi"
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }
FAKE_PORT="$(free_port)"
FAIL_PORT="$(free_port)"
FAKE_TOKEN_DIR="$(mktemp -d)"
echo "lint-fake-token-lint-fake-token-lint-fake-token" > "$FAKE_TOKEN_DIR/supervisor-token"

# 一次性假监督者 ×2,请求落盘(reqlog)防"服务没起来→测了个空"的空转通过:
#   FAKE_PORT  /kernel/versions 回乱序 PyPI 形状(故意回旧 latest_stable);其余回纯文本
#   FAIL_PORT  一律回 err_json 形状(模拟监督者的上游 PyPI 失败)
python3 - "$FAKE_PORT" "$WORK/reqlog" <<'PY' &
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        open(sys.argv[2], "a").write(self.path + "\n")
        if self.path.startswith("/kernel/install-log"):
            # 含引号/反斜杠的 canned 响应:钉死"桥透传不破坏 JSON"完整性
            body = json.dumps({"success": True,
                               "logs": ['Downloading "qwenpaw" wheel', "path C:\\Users\\x"]}).encode()
        elif self.path.startswith("/kernel/versions"):
            body = json.dumps({"releases": ["2.2.2.post1", "2.3.0b2", "2.9.9", "2.10.0b1", "2.10.0"],
                               "latest_stable": "2.2.2.post1"}).encode()
        else:
            body = b"invalid token"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        open(sys.argv[2], "a").write(self.path + " " + self.rfile.read(n).decode(errors="replace") + "\n")
        if self.path.startswith("/restart"):
            body = b'{"ok": false, "success": false, "message": "busy"}'
            self.send_response(409)
        else:
            body = b'{"ok": true, "success": true, "message": "install accepted"}'
            self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
FAKE_PID=$!
python3 - "$FAIL_PORT" "$WORK/reqlog2" <<'PY' &
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        open(sys.argv[2], "a").write(self.path + "\n")
        body = '{"ok": false, "success": false, "error": "upstream", "message": "查询 PyPI 失败"}'.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
FAIL_PID=$!
# 单一 trap 收口(此前两个 EXIT trap 互相覆盖,$WORK 泄漏);INT/TERM 一并清理
trap 'rm -rf "$WORK" "$FAKE_TOKEN_DIR"; kill "$FAKE_PID" "$FAIL_PID" 2>/dev/null' EXIT INT TERM

# 就绪探测:轮询 /healthz 直至假监督者真正应答(固定 sleep 在慢机偶发假 FAIL)
ready=0
for _ in 1 2 3 4 5 6 7 8 9 10; do
    if curl -s -o /dev/null "http://127.0.0.1:${FAKE_PORT}/healthz" \
        && grep -q '^/healthz' "$WORK/reqlog" 2>/dev/null; then
        ready=1
        break
    fi
    sleep 0.2
done
if [ "$ready" -ne 1 ]; then
    echo "  [FAIL] 假监督者未就绪,本层结果不可信"
    fail=1
fi

json_field() {  # $1=payload $2=python表达式 → 输出求值结果
    printf '%s' "$1" | python3 -c "import json,sys; d=json.load(sys.stdin); print($2)" 2>/dev/null
}

bridge_run() {  # $1=METHOD $2=QUERY $3=SUP_URL [$4=Origin] [$5=Cookie]
    local out
    if [ -n "${4:-}" ]; then
        if [ -n "${5:-}" ]; then
            out="$(QWENPAW_SUP_URL="$3" TRIM_PKGVAR="$FAKE_TOKEN_DIR"                 QUERY_STRING="$2" REQUEST_METHOD="$1"                 HTTP_HOST="127.0.0.1:9999" HTTP_ORIGIN="$4" HTTP_COOKIE="$5" "$BR")"
        else
            out="$(QWENPAW_SUP_URL="$3" TRIM_PKGVAR="$FAKE_TOKEN_DIR"                 QUERY_STRING="$2" REQUEST_METHOD="$1"                 HTTP_HOST="127.0.0.1:9999" HTTP_ORIGIN="$4" "$BR")"
        fi
    else
        out="$(env -u HTTP_ORIGIN -u HTTP_REFERER -u HTTP_HOST             QWENPAW_SUP_URL="$3" TRIM_PKGVAR="$FAKE_TOKEN_DIR"             QUERY_STRING="$2" REQUEST_METHOD="$1" "$BR")"
    fi
    printf '%s
' "$out" | tail -n 1
}

bridge_status() {  # $1=METHOD $2=QUERY_STRING → 输出桥声明的 HTTP 状态行(CGI Status: 头)
    env -u HTTP_ORIGIN -u HTTP_REFERER -u HTTP_HOST \
        QWENPAW_SUP_URL="http://127.0.0.1:1" TRIM_PKGVAR="$FAKE_TOKEN_DIR" \
        QUERY_STRING="$2" REQUEST_METHOD="$1" "$BR" | head -n 1
}

# T1 非 JSON 防御:上游回纯文本,桥必须替换为兜底对象而非透传
t1="$(bridge_run GET "action=status" "http://127.0.0.1:${FAKE_PORT}")"
if [ "$(json_field "$t1" "d.get('success')")" = "False" ] \
    && [ "$(json_field "$t1" "'监督者' in d.get('message','')")" = "True" ]; then
    echo "  [  OK ] 非 JSON 上游 → 兜底对象(不透传)"
else
    echo "  [FAIL] 非 JSON 上游被透传或兜底形状错误: ${t1:0:120}"
    fail=1
fi

# T2 version_list:形状转换 + 语义排序(2.10.0>2.9.9,稳定>预发布)+ latest_stable
# 推导(假监督者故意回旧值 2.2.2.post1,桥必须推出 2.10.0,不信 PyPI info.version)
t2="$(bridge_run GET "action=version_list" "http://127.0.0.1:${FAKE_PORT}")"
if [ "$(json_field "$t2" "d.get('success')")" = "True" ] \
    && [ "$(json_field "$t2" "d['versions'][0]['version']")" = "2.10.0" ] \
    && [ "$(json_field "$t2" "d['versions'][0]['is_beta']")" = "False" ] \
    && [ "$(json_field "$t2" "d['versions'][1]['version']")" = "2.10.0b1" ] \
    && [ "$(json_field "$t2" "d['versions'][1]['is_beta']")" = "True" ] \
    && [ "$(json_field "$t2" "d['versions'][2]['version']")" = "2.9.9" ] \
    && [ "$(json_field "$t2" "d.get('latest_stable')")" = "2.10.0" ]; then
    echo "  [  OK ] version_list 形状转换 + 语义排序 + latest_stable 推导"
else
    echo "  [FAIL] version_list 转换错误: ${t2:0:160}"
    fail=1
fi

# T3 监督者不可达:兜底对象必须是合法 JSON
t3="$(bridge_run GET "action=status" "http://127.0.0.1:1")"
if [ "$(json_field "$t3" "d.get('success')")" = "False" ]; then
    echo "  [  OK ] 监督者不可达 → 合法兜底 JSON"
else
    echo "  [FAIL] 不可达兜底非法: ${t3:0:120}"
    fail=1
fi

# T4 upgrade_logs 透传完整性:转义职责已下沉监督者(Rust),桥必须原样转发
# 含引号/反斜杠的 JSON 而不破坏结构(该路径曾裸拼产出非法 JSON,审计 major)
t4="$(bridge_run GET "action=upgrade_logs" "http://127.0.0.1:${FAKE_PORT}")"
if [ "$(json_field "$t4" "d.get('success')")" = "True" ] \
    && [ "$(json_field "$t4" "len(d.get('logs', []))")" = "2" ] \
    && [ "$(json_field "$t4" "chr(34) + 'qwenpaw' + chr(34) in d['logs'][0]")" = "True" ]; then
    echo "  [  OK ] upgrade_logs 引号/反斜杠安全转义"
else
    echo "  [FAIL] upgrade_logs 转义失败: ${t4:0:160}"
    fail=1
fi

# T5 version_list 上游失败:必须 success:false + 错误透传,绝不伪装"检查完成"
t5="$(bridge_run GET "action=version_list" "http://127.0.0.1:${FAIL_PORT}")"
if [ "$(json_field "$t5" "d.get('success')")" = "False" ] \
    && [ "$(json_field "$t5" "'查询 PyPI 失败' in d.get('message','')")" = "True" ]; then
    echo "  [  OK ] version_list 上游失败如实降级"
else
    echo "  [FAIL] 上游失败被伪装成成功: ${t5:0:160}"
    fail=1
fi

# T6 set_mirror:合法 URL 落盘;注入载荷(%3B → ';')被字符白名单拒绝 ——
# mirror 会进监督者的 bash -c pip 串,这里必须挡死
t6="$(bridge_run POST "action=set_mirror&mirror=https://pypi.tuna.tsinghua.edu.cn/simple&_csrf=tok" "http://127.0.0.1:1" "http://127.0.0.1:9999" "qwenpaw_csrf=tok")"
t6bad="$(bridge_run POST "action=set_mirror&mirror=https%3A%2F%2Fx.test%2F%3Brm&_csrf=tok" "http://127.0.0.1:1" "http://127.0.0.1:9999" "qwenpaw_csrf=tok")"
if [ "$(json_field "$t6" "d.get('success')")" = "True" ] \
    && [ "$(cat "$FAKE_TOKEN_DIR/pip-mirror")" = "https://pypi.tuna.tsinghua.edu.cn/simple" ] \
    && [ "$(json_field "$t6bad" "d.get('success')")" = "False" ]; then
    echo "  [  OK ] set_mirror 落盘 + 注入载荷拒绝"
else
    echo "  [FAIL] set_mirror 异常: ok=${t6:0:100} bad=${t6bad:0:100}"
    fail=1
fi

# T7 upgrade 未带 mirror → 桥必须把已存下载源注入请求体
t7="$(bridge_run POST "action=upgrade&version=2.3.0b2&_csrf=tok" "http://127.0.0.1:${FAKE_PORT}" "http://127.0.0.1:9999" "qwenpaw_csrf=tok")"
if [ "$(json_field "$t7" "d.get('success')")" = "True" ] \
    && grep -qF '/kernel/install {"version": "2.3.0b2", "mirror": "https://pypi.tuna.tsinghua.edu.cn/simple"}' "$WORK/reqlog"; then
    echo "  [  OK ] upgrade 请求体携带已存下载源"
else
    echo "  [FAIL] upgrade 未携带下载源: ${t7:0:120}"
    fail=1
fi

# T8 upgrade 显式 mirror=(空,控制台选官方)→ 覆盖已存文件值
t8="$(bridge_run POST "action=upgrade&version=2.2.2.post1&mirror=&_csrf=tok" "http://127.0.0.1:${FAKE_PORT}" "http://127.0.0.1:9999" "qwenpaw_csrf=tok")"
if [ "$(json_field "$t8" "d.get('success')")" = "True" ] \
    && grep -qF '/kernel/install {"version": "2.2.2.post1", "mirror": ""}' "$WORK/reqlog"; then
    echo "  [  OK ] upgrade 显式 mirror= 覆盖文件值"
else
    echo "  [FAIL] 显式 mirror= 未生效: ${t8:0:120}"
    fail=1
fi

# ── [4b/5] 桥安全门禁:方法白名单 + 同源校验(P0 回归,勿删) ──
echo "[4b/5] 桥安全门禁(405/403/404)..."

# T9 读操作之外的 action 用 GET 触发 → 必须 405 + Allow: POST(历史版本 GET 即可停服)
s9="$(bridge_status GET "action=stop")"
t9="$(bridge_run GET "action=stop" "http://127.0.0.1:${FAKE_PORT}")"
if printf '%s' "$s9" | grep -q "405" \
    && [ "$(json_field "$t9" "d.get('success')")" = "False" ]; then
    echo "  [  OK ] GET 触发变更操作 → 405 拒绝"
else
    echo "  [FAIL] GET 变更操作未被 405 拒绝: status=${s9} body=${t9:0:120}"
    fail=1
fi

# ── CSRF:double-submit cookie 机制(2.0.0-14 起)。背景:fnOS 网关代理 CGI
# 时 Host 被重写为 "localhost"(插桩实测),基于 Host/Origin 的同源校验在该
# 形态下把控制台全部 POST 误杀 403 —— token 机制与部署拓扑完全无关。 ──
envrun() {  # 直接以环境变量驱动桥:$1=QUERY_STRING,其余为 VAR=VAL 环境对
    local qs="$1"; shift
    env QWENPAW_SUP_URL="http://127.0.0.1:${FAKE_PORT}" TRIM_PKGVAR="$FAKE_TOKEN_DIR"         QUERY_STRING="$qs" REQUEST_METHOD="POST" "$@" "$BR" | tail -n 1
}

# T10 无 token(cookie 缺失 = 页面过旧/直连伪造)→ 403 且不透传监督者
t10a="$(envrun "action=stop")"
# T10b token 头与 cookie 不匹配(伪造)→ 403
t10b="$(envrun "action=stop" HTTP_COOKIE="qwenpaw_csrf=good" HTTP_X_CSRF_TOKEN="evil")"
if [ "$(json_field "$t10a" "d.get('success')")" = "False" ]     && [ "$(json_field "$t10b" "d.get('success')")" = "False" ]     && ! grep -q '^/stop' "$WORK/reqlog"; then
    echo "  [  OK ] 无 token/伪造 token POST → 403 且未透传监督者"
else
    echo "  [FAIL] CSRF token 校验失效: missing=${t10a:0:100} evil=${t10b:0:100}"
    fail=1
fi

# T11 合法 token → 放行。★实捕回放★:环境头逐字取自 2.0.0-13 插桩捕获的
# 网关真实请求(Host=localhost / Origin=NAS:15666 / SCRIPT_NAME=安装路径),
# 在此形态下加配对 cookie+头必须放行 —— 这正是当初被误杀的合法控制台请求。
t11="$(envrun "action=start"     HTTP_HOST="localhost"     HTTP_ORIGIN="http://192.0.2.10:15666"     HTTP_REFERER="http://192.0.2.10:15666/cgi/ThirdParty/qwenpaw/index.cgi/"     SCRIPT_NAME="/var/apps/qwenpaw"     HTTP_COOKIE="qwenpaw_csrf=tok" HTTP_X_CSRF_TOKEN="tok")"
if [ "$(json_field "$t11" "d.get('success')")" = "True" ]     && grep -q '^/start' "$WORK/reqlog"; then
    echo "  [  OK ] 实捕网关形态 + 合法 token → 放行(Host=localhost 不再误杀)"
else
    echo "  [FAIL] 实捕网关形态被误拒: ${t11:0:160}"
    fail=1
fi

# T11b sendBeacon 退路:带不了自定义头,token 走 _csrf 查询参数 → 等效放行
t11b="$(envrun "action=start&_csrf=tok"     HTTP_HOST="localhost" HTTP_COOKIE="qwenpaw_csrf=tok")"
if [ "$(json_field "$t11b" "d.get('success')")" = "True" ]     && grep -q '^/start' "$WORK/reqlog"; then
    echo "  [  OK ] sendBeacon _csrf 查询参数退路 → 放行"
else
    echo "  [FAIL] sendBeacon 退路被误拒: ${t11b:0:140}"
    fail=1
fi

# T11c 上游 409 必须连同 JSON 一起透传，不能由 CGI 改成 200
s11c="$(env QWENPAW_SUP_URL="http://127.0.0.1:${FAKE_PORT}" TRIM_PKGVAR="$FAKE_TOKEN_DIR" \
    QUERY_STRING="action=restart" REQUEST_METHOD="POST" HTTP_COOKIE="qwenpaw_csrf=tok" \
    HTTP_X_CSRF_TOKEN="tok" "$BR" | head -n 1)"
t11c="$(envrun "action=restart" HTTP_COOKIE="qwenpaw_csrf=tok" HTTP_X_CSRF_TOKEN="tok")"
if printf '%s' "$s11c" | grep -q '^Status: 409' \
    && [ "$(json_field "$t11c" "d.get('success')")" = "False" ]; then
    echo "  [  OK ] 上游 409 状态码与错误 JSON 同步透传"
else
    echo "  [FAIL] 上游 409 未正确透传: status=${s11c} body=${t11c:0:120}"
    fail=1
fi

# T11d 仅卸载内核必须走独立端点,不得误映射为卸载整个应用
if envrun "action=uninstall_kernel" HTTP_COOKIE="qwenpaw_csrf=tok" HTTP_X_CSRF_TOKEN="tok" >/dev/null \
    && grep -q '^/kernel/uninstall' "$WORK/reqlog"; then
    echo "  [  OK ] 仅卸载内核桥接至独立监督者端点"
else
    echo "  [FAIL] 仅卸载内核桥接路由缺失"
    fail=1
fi

# T12 未知 action → 404(白名单外不落到 /status 兜底)
s12="$(bridge_status GET "action=unknown_action")"
if printf '%s' "$s12" | grep -q "404"; then
    echo "  [  OK ] 未知 action → 404"
else
    echo "  [FAIL] 未知 action 未返回 404: ${s12}"
    fail=1
fi

# ── [5/5] 控制台前端编译(esbuild 即语法门禁;内联 JS 语法错误曾令控制台
# 全动态渲染静默失效停在静态默认 —— 编译不过是同一事故类别的门禁) ──
echo "[5/5] 控制台前端编译 ..."
WEB_DIR="$ROOT/web"
if [ -f "$WEB_DIR/src/main.ts" ] && [ -f "$WEB_DIR/src/index.html" ]; then
    NODE_BIN="$(command -v node 2>/dev/null || true)"
    [ -z "$NODE_BIN" ] && [ -x /var/apps/nodejs_v24/target/bin/node ] && NODE_BIN=/var/apps/nodejs_v24/target/bin/node
    if [ -n "$NODE_BIN" ] && [ -x "$WEB_DIR/node_modules/.bin/esbuild" ]; then
        if (cd "$WEB_DIR" && BUILD_ID=lint "$NODE_BIN" build.mjs >/dev/null 2>&1); then
            echo "  [  OK ] esbuild 编译通过(单文件产物 dist/index.html)"
        else
            echo "  [FAIL] esbuild 编译失败:"
            (cd "$WEB_DIR" && BUILD_ID=lint "$NODE_BIN" build.mjs)
            fail=1
        fi
    else
        if [ "${STRICT_CI:-0}" = "1" ]; then
            echo "  [FAIL] STRICT_CI 模式下不允许跳过 esbuild 前端编译门禁"
            fail=1
        else
            echo "  [SKIP] esbuild 不可用(缺 node 或 web/node_modules,先 npm install)"
        fi
    fi
else
    if [ "${STRICT_CI:-0}" = "1" ]; then
        echo "  [FAIL] STRICT_CI 模式下 web/src 缺失"
        fail=1
    else
        echo "  [SKIP] web/src 缺失"
    fi
fi

# ── [6/6] 监督者生命周期身份校验回归（P1-01/P1-03，勿删）──
echo "[6/6] 监督者生命周期回归 ..."
if bash "$ROOT/scripts/test-lifecycle.sh"; then
    echo "  [  OK ] 身份校验/安全停止/停止宽限回归通过"
else
    echo "  [FAIL] 监督者生命周期回归失败（陈旧 PID 误杀或宽限不足）"
    fail=1
fi

echo "========================================="
if [ $fail -eq 0 ]; then
    echo "lint: ALL CHECKS PASSED"
    exit 0
fi
echo "lint: FAIL"
exit 1
