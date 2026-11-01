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

echo "========================================="
if [ "$fail" -eq 0 ]; then
    echo "test-lifecycle: ALL CHECKS PASSED"
    exit 0
fi
echo "test-lifecycle: FAIL"
exit 1
