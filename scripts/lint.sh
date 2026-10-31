#!/bin/bash
# lint.sh — 语法与静态检查门禁（四层）
#   [1/4] bash -n 全量
#   [2/4] shellcheck（error 级为门禁，warning 仅报告；本地无二进制时自动下载 pinned 版本）
#   [3/4] 内嵌 spawn 脚本展开验证 —— api.cgi 中以 nohup bash -c "..." / local cmd="..."
#         形式内嵌的启动脚本是"运行时展开"代码，bash -n 与 shellcheck 都查不出其中的
#         变量误展开（历史事故：awk $2 被 nohup 外层展开成空），必须按真实展开语义
#         生成内层脚本后单独验证，并断言每条启动路径都携带内核启动必需的环境变量。
#   [4/4] api.cgi 桥接输出 JSON 有效性 —— 控制台所有动态渲染依赖 res.json() 成功；
#         桥接只要吐出一段非 JSON（历史事故：echo "${resp:-{...}}" 默认词里的 }
#         提前终止参数展开，输出 "{...}}"），前端就全体静默失效停在静态默认值。
# 用法: bash scripts/lint.sh
# 注意: 不用 set -e（工程铁律），失败显式 exit 1

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHELLCHECK_VER="0.10.0"
fail=0

echo "========== lint: 语法与静态检查 =========="

# ── [1/3] bash -n 全量 ──
echo "[1/4] bash -n ..."
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

# ── [2/3] shellcheck（error 级门禁；warning 仅报告不拦截） ──
echo "[2/4] shellcheck v${SHELLCHECK_VER} ..."
SC_BIN="$(command -v shellcheck 2>/dev/null || true)"
[ -z "$SC_BIN" ] && [ -x /tmp/shellcheck ] && SC_BIN=/tmp/shellcheck
if [ -z "$SC_BIN" ]; then
    TARBALL="/tmp/shellcheck-${SHELLCHECK_VER}.tar.xz"
    if curl -sL --max-time 120 -o "$TARBALL" \
        "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VER}/shellcheck-v${SHELLCHECK_VER}.linux.x86_64.tar.xz" \
        && tar -xJf "$TARBALL" -C /tmp "shellcheck-v${SHELLCHECK_VER}/shellcheck" 2>/dev/null; then
        mv "/tmp/shellcheck-v${SHELLCHECK_VER}/shellcheck" /tmp/shellcheck
        SC_BIN=/tmp/shellcheck
    fi
fi
if [ -n "$SC_BIN" ] && [ -x "$SC_BIN" ]; then
    # 活跃脚本；scripts/update-qwenpaw.sh 为遗留脚本不参与门禁
    TARGETS=(cmd/main cmd/install_callback cmd/upgrade_callback cmd/config_callback cmd/uninstall_callback app/ui/api.cgi app/ui/index.cgi build.sh)
    if "$SC_BIN" -S error "${TARGETS[@]/#/"$ROOT/"}" 2>&1; then
        echo "  [  OK ] error 级问题为 0（warning 级见上方输出，不拦截）"
    else
        echo "  [FAIL] shellcheck error 级问题需修复"
        fail=1
    fi
else
    echo "  [SKIP] shellcheck 不可用（网络/二进制缺失），跳过本层"
fi

# ── [3/4] 内嵌 spawn 脚本展开验证 ──
# v2 起 spawn 逻辑迁入监督者(kernel.rs process_group),瘦桥无内嵌启动脚本,
# 本层常态提取 0 段 —— 保留作哨兵:若未来重新出现内嵌启动脚本即自动激活。
echo "[3/4] 内嵌 spawn 脚本展开验证 ..."
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
echo "[4/4] api.cgi 桥接 JSON 有效性 ..."
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
        if self.path.startswith("/kernel/versions"):
            body = json.dumps({"releases": ["2.2.2.post1", "2.3.0b2", "2.9.9", "2.10.0b1", "2.10.0"],
                               "latest_stable": "2.2.2.post1"}).encode()
        else:
            body = b"invalid token"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
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

bridge_run() {  # $1=QUERY_STRING $2=SUP_URL → 输出尾行(JSON 体)
    local out
    out="$(QWENPAW_SUP_URL="$2" TRIM_PKGVAR="$FAKE_TOKEN_DIR" \
        QUERY_STRING="$1" REQUEST_METHOD=GET "$BR")"
    printf '%s\n' "$out" | tail -n 1
}

# T1 非 JSON 防御:上游回纯文本,桥必须替换为兜底对象而非透传
t1="$(bridge_run "action=status" "http://127.0.0.1:${FAKE_PORT}")"
if [ "$(json_field "$t1" "d.get('success')")" = "False" ] \
    && [ "$(json_field "$t1" "'监督者' in d.get('message','')")" = "True" ]; then
    echo "  [  OK ] 非 JSON 上游 → 兜底对象(不透传)"
else
    echo "  [FAIL] 非 JSON 上游被透传或兜底形状错误: ${t1:0:120}"
    fail=1
fi

# T2 version_list:形状转换 + 语义排序(2.10.0>2.9.9,稳定>预发布)+ latest_stable
# 推导(假监督者故意回旧值 2.2.2.post1,桥必须推出 2.10.0,不信 PyPI info.version)
t2="$(bridge_run "action=version_list" "http://127.0.0.1:${FAKE_PORT}")"
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
t3="$(bridge_run "action=status" "http://127.0.0.1:1")"
if [ "$(json_field "$t3" "d.get('success')")" = "False" ]; then
    echo "  [  OK ] 监督者不可达 → 合法兜底 JSON"
else
    echo "  [FAIL] 不可达兜底非法: ${t3:0:120}"
    fail=1
fi

# T4 upgrade_logs:pip 日志含引号/反斜杠时必须经 json.dumps 转义(审计 major ——
# 旧实现裸拼,升级日志一出引号就产出非法 JSON,且该路径此前零覆盖)
printf 'Downloading "qwenpaw" wheel\npath C:\\Users\\x\n' > "$FAKE_TOKEN_DIR/qwenpaw-upgrade.log.pip"
t4="$(bridge_run "action=upgrade_logs" "http://127.0.0.1:1")"
if [ "$(json_field "$t4" "d.get('success')")" = "True" ] \
    && [ "$(json_field "$t4" "len(d.get('logs', []))")" = "2" ] \
    && [ "$(json_field "$t4" "chr(34) + 'qwenpaw' + chr(34) in d['logs'][0]")" = "True" ]; then
    echo "  [  OK ] upgrade_logs 引号/反斜杠安全转义"
else
    echo "  [FAIL] upgrade_logs 转义失败: ${t4:0:160}"
    fail=1
fi

# T5 version_list 上游失败:必须 success:false + 错误透传,绝不伪装"检查完成"
t5="$(bridge_run "action=version_list" "http://127.0.0.1:${FAIL_PORT}")"
if [ "$(json_field "$t5" "d.get('success')")" = "False" ] \
    && [ "$(json_field "$t5" "'查询 PyPI 失败' in d.get('message','')")" = "True" ]; then
    echo "  [  OK ] version_list 上游失败如实降级"
else
    echo "  [FAIL] 上游失败被伪装成成功: ${t5:0:160}"
    fail=1
fi

echo "========================================="
if [ $fail -eq 0 ]; then
    echo "lint: ALL CHECKS PASSED"
    exit 0
fi
echo "lint: FAIL"
exit 1
