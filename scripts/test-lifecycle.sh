#!/bin/bash
# test-lifecycle.sh — 监督者身份校验/安全停止回归（审计 P1-01/P1-03，批次 A）
# 可独立运行：bash scripts/test-lifecycle.sh；lint.sh 最后一层自动调用。
# 全部在临时目录进行，只启动 sleep/bash 派生的替身进程，不触碰真实安装。
# 注意：不用 set -e（工程铁律），失败显式计数并 exit 1。

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CTL="$ROOT/cmd/supervisor_ctl.sh"
if [ ! -f "$CTL" ]; then
    echo "FAIL: 找不到 $CTL"
    exit 1
fi

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT INT TERM
fail=0

# 快速宽限：仅影响本测试进程内的行为测试；默认值(30s)另行断言
QWENPAW_STOP_GRACE_SECS=2
# shellcheck source=cmd/supervisor_ctl.sh
. "$CTL"

ok() { echo "  [  OK ] $1"; }
bad() { echo "  [FAIL] $1"; fail=1; }

# ── [0] 默认外部宽限必须覆盖监督者内部最坏清理（P1-03：内部 ~16.5s，
#        旧外部宽限 15s 会在后代清扫完成前强杀监督者）──
def_grace="$(QWENPAW_STOP_GRACE_SECS= bash -c '. '"$(printf '%q' "$CTL")"' >/dev/null 2>&1; echo "$SUPERVISOR_STOP_GRACE_SECS"')"
if [ "${def_grace:-0}" -ge 25 ] 2>/dev/null; then
    ok "默认外部宽限 ${def_grace}s ≥ 25s（覆盖内部最坏 ~16.5s 清理）"
else
    bad "默认外部宽限为 ${def_grace:-未定义}s，低于 25s（P1-03 回归）"
fi

# ── [0b] starttime 戳必须与 /proc 实际字段一致（回归：位置参数误写 $20 会
#        展开成 ${1}0，戳内容变成"状态字符+0"—— 状态字符随时变化，带戳核对
#        必然失败，真实监督者会被误判为陈旧。sleep 替身稳定在 S 态测不出，
#        必须与独立解析交叉验证）──
sleep 1 &
probe=$!
ticks="$(proc_start_ticks "$probe")"
case "$ticks" in
    ''|*[!0-9]*) bad "proc_start_ticks 输出非数字: ${ticks:-空}（位置参数 \${20} 回归）" ;;
    *)
        if command -v python3 >/dev/null 2>&1; then
            expect="$(python3 -c "
s = open('/proc/$probe/stat').read()
print(s[s.rfind(')') + 2:].split()[19])" 2>/dev/null)"
            if [ -n "$expect" ] && [ "$ticks" = "$expect" ]; then
                ok "proc_start_ticks=$ticks 与 /proc 独立解析一致"
            else
                bad "proc_start_ticks($ticks) 与 /proc 实际值(${expect:-解析失败})不一致"
            fi
        else
            ok "proc_start_ticks 输出为纯数字（python3 缺失，跳过交叉验证）"
        fi
        ;;
esac
kill "$probe" 2>/dev/null
wait "$probe" 2>/dev/null

# ── [1] 陈旧 PID 指向无关进程：status 不得报运行，stop 不得发信号（P1-01，
#        动态复现场景的反向断言：把 sleep 的 PID 写进 supervisor.pid）──
sleep 300 &
victim=$!
echo "$victim" > "${T}/supervisor.pid"
if supervisor_status "${T}/supervisor.pid"; then
    bad "status 把无关进程误报为运行中（P1-01）"
else
    ok "status 对陈旧 PID 返回未运行"
fi
[ -f "${T}/supervisor.pid" ] && bad "status 未清理陈旧 PID 文件"
echo "$victim" > "${T}/supervisor.pid"
rc=0
supervisor_safe_stop "${T}/supervisor.pid" || rc=$?
if [ "$rc" -eq 1 ] && proc_alive_state "$victim"; then
    ok "safe_stop 拒绝对身份不符进程发信号（rc=1，进程存活）"
else
    bad "safe_stop 行为异常：rc=$rc，victim 存活=$(proc_alive_state "$victim" && echo yes || echo no)"
fi
# 非数字 PID 同样按"身份不符"处理：只清理不发信号（rc=1）
printf 'not-a-pid\n' > "${T}/supervisor.pid"
rc=0
supervisor_safe_stop "${T}/supervisor.pid" || rc=$?
if [ "$rc" -eq 1 ] && [ ! -f "${T}/supervisor.pid" ]; then
    ok "非数字 PID 被安全忽略（rc=1，文件已清理）"
else
    bad "非数字 PID 处理异常：rc=$rc"
fi
if proc_alive_state "$victim"; then
    ok "全部陈旧 PID 场景下无关进程未被误杀"
else
    bad "无关进程被终止（P1-01 回归失败）"
fi
kill "$victim" 2>/dev/null
wait "$victim" 2>/dev/null

# ── [2] 真实替身（exe basename 命中监督者特征）：status 可识别，stop 可停止，
#        starttime 戳一并登记 ──
cp /bin/sleep "${T}/qwenpaw-supervisor"
"${T}/qwenpaw-supervisor" 300 &
fake=$!
supervisor_register_start "$fake" "${T}/supervisor.pid" >/dev/null || bad "register_start 失败"
if supervisor_status "${T}/supervisor.pid"; then
    ok "status 识别出替身监督者"
else
    bad "status 未能识别替身监督者"
fi
[ -f "${T}/supervisor.pid.starttime" ] || bad "starttime 戳未登记"
supervisor_safe_stop "${T}/supervisor.pid"
wait "$fake" 2>/dev/null
if proc_alive_state "$fake"; then
    bad "safe_stop 未能停止替身监督者"
else
    ok "safe_stop 正常停止替身监督者并清理 PID 文件"
fi
[ -f "${T}/supervisor.pid" ] || [ -f "${T}/supervisor.pid.starttime" ] \
    && bad "停止后 PID/戳文件未清理" || ok "停止后 PID/戳文件已清理"

# ── [3] starttime 戳不匹配：即使 exe 同名也拒绝（防"复用 PID 恰好同名"）──
"${T}/qwenpaw-supervisor" 300 &
fake2=$!
echo "$fake2" > "${T}/supervisor.pid"
echo "123456789" > "${T}/supervisor.pid.starttime"
if supervisor_status "${T}/supervisor.pid"; then
    bad "戳不匹配时 status 误报运行"
else
    ok "戳不匹配时 status 判定未运行（已清理）"
fi
echo "$fake2" > "${T}/supervisor.pid"
echo "123456789" > "${T}/supervisor.pid.starttime"
supervisor_safe_stop "${T}/supervisor.pid" || true
if proc_alive_state "$fake2"; then
    ok "戳不匹配时 safe_stop 未发信号"
else
    bad "戳不匹配时 safe_stop 仍终止了进程"
fi
kill "$fake2" 2>/dev/null
wait "$fake2" 2>/dev/null

# ── [4] 忽略 TERM 的替身：宽限耗尽后 KILL 兜底，宽限上限生效 ──
cp /bin/bash "${T}/qwenpaw-supervisor"
# 末尾用复合命令:防止 bash 对 -c 末尾简单命令做 exec 优化(那样 exe 会变成
# sleep,身份校验按设计拒绝,测不到 KILL 兜底);sleep 1 保证无 >1s 的孤儿
"${T}/qwenpaw-supervisor" -c 'trap "" TERM; while :; do sleep 1; done' &
stubborn=$!
echo "$stubborn" > "${T}/supervisor.pid"
start_ts=$(date +%s)
supervisor_safe_stop "${T}/supervisor.pid"
elapsed=$(( $(date +%s) - start_ts ))
if proc_alive_state "$stubborn"; then
    bad "忽略 TERM 的进程未被 KILL 兜底"
elif [ "$elapsed" -ge 6 ]; then
    bad "宽限上限未生效：耗时 ${elapsed}s（预期 ~2-3s）"
else
    ok "宽限 ${elapsed}s 后 KILL 兜底生效"
fi

# ── [5] 幂等：PID 文件缺失/为空时安全返回 0 ──
supervisor_safe_stop "${T}/nonexistent.pid" && ok "缺失 PID 文件安全返回" || bad "缺失 PID 文件返回异常"
: > "${T}/empty.pid"
supervisor_safe_stop "${T}/empty.pid" && ok "空 PID 文件安全返回" || bad "空 PID 文件返回异常"

# ── [6] 向导初始化回显 (config_init / upgrade_init) ──
# 6.1 config_init 默认回显 (无落盘文件时)
out6a="$(TRIM_PKGVAR="${T}/cfg_var1" bash "$ROOT/cmd/config_init")"
if echo "$out6a" | grep -q "wizard_port=18088" &&    echo "$out6a" | grep -q "service_port=18088" &&    echo "$out6a" | grep -q "wizard_log_level=info"; then
    ok "config_init 默认回显正常 (wizard_port/service_port/wizard_log_level)"
else
    bad "config_init 默认回显异常: $out6a"
fi

# 6.2 config_init 自定义落盘值回显 (端口与 debug 日志级别)
mkdir -p "${T}/cfg_var2"
echo "19088" > "${T}/cfg_var2/service_port"
echo "debug" > "${T}/cfg_var2/qwenpaw-log-level"
out6b="$(TRIM_PKGVAR="${T}/cfg_var2" bash "$ROOT/cmd/config_init")"
if echo "$out6b" | grep -q "wizard_port=19088" &&    echo "$out6b" | grep -q "service_port=19088" &&    echo "$out6b" | grep -q "wizard_log_level=debug"; then
    ok "config_init 自定义端口与日志级别回显正常 (避免设置弹窗覆盖重置)"
else
    bad "config_init 自定义回显异常: $out6b"
fi

# 6.3 upgrade_init 镜像源回显
mkdir -p "${T}/up_var"
echo "https://pypi.tuna.tsinghua.edu.cn/simple" > "${T}/up_var/pip-mirror"
out6c="$(TRIM_PKGVAR="${T}/up_var" bash "$ROOT/cmd/upgrade_init")"
if [ "$out6c" = "wizard_pypi_mirror=tsinghua" ]; then
    ok "upgrade_init 已存清华源正常回显"
else
    bad "upgrade_init 清华源回显异常: $out6c"
fi
echo "https://mirrors.aliyun.com/pypi/simple/" > "${T}/up_var/pip-mirror"
out6d="$(TRIM_PKGVAR="${T}/up_var" bash "$ROOT/cmd/upgrade_init")"
if [ "$out6d" = "wizard_pypi_mirror=aliyun" ]; then
    ok "upgrade_init 已存阿里源正常回显"
else
    bad "upgrade_init 阿里源回显异常: $out6d"
fi

# ── [7] 配置修改回调 (config_callback) ──
mkdir -p "${T}/cb_var" "${T}/cb_dest/cmd" "${T}/cb_dest/ui"
cat > "${T}/cb_dest/ui/config" <<'EOF'
{"port": "18088"}
EOF
cat > "${T}/cb_dest/cmd/main" <<'EOF'
#!/bin/bash
exit 0
EOF
chmod +x "${T}/cb_dest/cmd/main"
TRIM_PKGVAR="${T}/cb_var" TRIM_APPDEST="${T}/cb_dest" wizard_port=18092 wizard_log_level=warning bash "$ROOT/cmd/config_callback" >/dev/null 2>&1
if [ "$(cat "${T}/cb_var/service_port" 2>/dev/null)" = "18092" ] &&    [ "$(cat "${T}/cb_var/qwenpaw-log-level" 2>/dev/null)" = "warning" ] &&    grep -q '"port": "18092"' "${T}/cb_dest/ui/config"; then
    ok "config_callback 正常落盘新端口、日志级别并同步 ui/config"
else
    bad "config_callback 落盘异常"
fi

# ── [8] 卸载回调生命周期与数据治理 (uninstall_callback) ──
# 回归门禁：删除必须受 TRIM_* 约束，不得出现硬编码全局路径（历史事故：
# 测试以临时 TRIM_DATA_SHARE_PATHS 运行回调，硬编码删除绕过了隔离，
# 直接删除了真实应用数据）。
if grep -nE '[^#]*rm[[:space:]]+-rf[[:space:]]+"/?vol' "$ROOT/cmd/uninstall_callback" >/dev/null; then
    bad "uninstall_callback 含硬编码 /vol* 删除（会绕过 TRIM_* 隔离）"
else
    ok "uninstall_callback 无硬编码 /vol* 删除"
fi

# 8.1 彻底删除模式 (wizard_keep_data=remove)：清应用目录，保留 fnOS 共享根
M_VAR="${T}/un_var1"
M_HOME="${T}/un_home1"
M_ETC="${T}/un_etc1"
M_SHARE="${T}/un_share1"
mkdir -p "$M_VAR" "$M_HOME" "$M_ETC" "$M_SHARE/.qwenpaw" "$M_SHARE/.qwenpaw.secret"
touch "$M_VAR/dummy" "$M_HOME/dummy" "$M_ETC/dummy" "$M_SHARE/dummy" "$M_SHARE/.qwenpaw/memory.db" "$M_SHARE/.qwenpaw.secret/auth.json"
wizard_keep_data=remove TRIM_PKGVAR="$M_VAR" TRIM_PKGHOME="$M_HOME" TRIM_PKGETC="$M_ETC" TRIM_DATA_SHARE_PATHS="$M_SHARE" bash "$ROOT/cmd/uninstall_callback" >/dev/null 2>&1
if [ ! -d "$M_VAR" ] && [ ! -d "$M_HOME" ] && [ ! -d "$M_ETC" ] \
    && [ -d "$M_SHARE" ] && [ ! -d "$M_SHARE/.qwenpaw" ] && [ ! -d "$M_SHARE/.qwenpaw.secret" ] && [ -f "$M_SHARE/dummy" ]; then
    ok "uninstall_callback 彻底删除模式仅清 .qwenpaw，保留共享根"
else
    bad "uninstall_callback 彻底删除模式越界或残留: var=$(test -d "$M_VAR" && echo 存在) home=$(test -d "$M_HOME" && echo 存在) share=$(test -d "$M_SHARE" && echo 存在)"
fi

# 8.2 保留数据模式 (wizard_keep_data=keep)：仅清理 venv，保留数据与配置
M_VAR2="${T}/un_var2"
M_HOME2="${T}/un_home2"
M_SHARE2="${T}/un_share2"
mkdir -p "$M_VAR2" "$M_HOME2/venv" "$M_SHARE2/.qwenpaw"
touch "$M_VAR2/config" "$M_HOME2/venv/bin_python" "$M_SHARE2/.qwenpaw/memory.db"
wizard_keep_data=keep TRIM_PKGVAR="$M_VAR2" TRIM_PKGHOME="$M_HOME2" TRIM_DATA_SHARE_PATHS="$M_SHARE2" bash "$ROOT/cmd/uninstall_callback" >/dev/null 2>&1
if [ -f "$M_VAR2/config" ] && [ -f "$M_SHARE2/.qwenpaw/memory.db" ] && [ ! -d "$M_HOME2/venv" ]; then
    ok "uninstall_callback 保留数据模式成功保留配置与记忆，并仅清除 venv"
else
    bad "uninstall_callback 保留数据模式行为异常"
fi

echo "========================================="
if [ "$fail" -eq 0 ]; then
    echo "test-lifecycle: ALL CHECKS PASSED"
    exit 0
fi
echo "test-lifecycle: FAIL"
exit 1
