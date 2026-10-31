package main

// 内核进程管理器 —— 模式裁剪自 dsh backend/dsh.go(MIT),按 QwenPaw 语境改写。
// 血泪清单(每条都有 dsh 线上事故背书,勿删):
//   - 只按 PID/进程组杀,绝不按进程名杀(误杀持锁进程会留陈旧锁)
//   - 僵尸判定看 /proc/<pid>/stat 最后一个 ')' 之后的状态字符,不看目录存在
//   - 被管 pid 失效时扫 /proc 重新发现(内核自换 PID 的对账兜底)
//   - 重发现结果缓存 2s,防高频轮询拖垮接口
//   - 停止三段式:组 TERM(宽限)→ 单 KILL → 组 KILL 清扫

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

// NewKernelManager 构造(mu 零值不可用,必须经此构造)。
func NewKernelManager(pidFile, logFile string, port int) *KernelManager {
	return &KernelManager{mu: sync.Mutex{}, pidFile: pidFile, logFile: logFile, port: port}
}

// KernelManager 持有内核子进程的全部受管状态。
type KernelManager struct {
	mu        sync.Mutex
	cmd       *exec.Cmd
	tracked   int    // 受管 pid(启动时记录)
	livePid   int    // /proc 重发现缓存
	pidAt     time.Time
	pidFile   string // kernel.pid(被管进程,与 supervisor.pid 分离)
	logFile   string
	port      int
	startedAt time.Time
}

// Spawn 以独立进程组启动内核(start_new_session 等价于 bash setsid)。
// pid 文件记录的就是真实内核进程(v1.0.9 教训:不能记 bash 包装层)。
func (m *KernelManager) Spawn(venvDir string, port int) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.tracked != 0 && processAlive(m.tracked) {
		return fmt.Errorf("kernel already running pid=%d", m.tracked)
	}
	m.reapCmd()
	bin := filepath.Join(venvDir, "bin", "python3")
	if _, err := os.Stat(bin); err != nil {
		return fmt.Errorf("kernel not installed: %w", err)
	}
	cmd := exec.Command(bin, "-m", "qwenpaw", "app",
		"--host", "0.0.0.0", "--port", strconv.Itoa(port))
	cmd.Env = append(os.Environ(),
		fmt.Sprintf("QWENPAW_DEPENDENCY_STATE_DIR=%s/dependency-environments", m.shareDir()))
	cmd.Dir = m.shareDir()
	// 独立进程组:停止时组信号可连带浏览器/MCP 等子进程
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	logF, err := os.OpenFile(m.logFile, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return err
	}
	defer logF.Close()
	cmd.Stdout = logF
	cmd.Stderr = logF
	if err := cmd.Start(); err != nil {
		return err
	}
	m.cmd = cmd
	m.tracked = cmd.Process.Pid
	m.livePid = m.tracked
	m.pidAt = time.Now()
	m.startedAt = time.Now()
	m.port = port
	m.writePidFile()
	return nil
}

// Stop 三段式:组 TERM(宽限 10s)→ 单 KILL → 组 KILL 清扫。
// 组信号仅在内核为自身进程组长(setsid 启动)时启用,防误伤同组平台进程。
func (m *KernelManager) Stop() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.stopLocked()
}

func (m *KernelManager) stopLocked() {
	target := m.tracked
	if target == 0 || !processAlive(target) {
		target = m.rediscoverLocked(0)
	}
	if target <= 0 {
		m.reapCmd()
		m.removePidFile()
		return
	}
	pgid := groupOf(target)
	groupOK := pgid == target
	// 1) 组 TERM + 单 TERM(宽限 10s)
	if groupOK {
		_ = syscall.Kill(-target, syscall.SIGTERM)
	}
	_ = syscall.Kill(target, syscall.SIGTERM)
	for i := 0; i < 10 && processAlive(target); i++ {
		time.Sleep(time.Second)
	}
	// 2) 仍活:单 KILL
	if processAlive(target) {
		_ = syscall.Kill(target, syscall.SIGKILL)
		time.Sleep(time.Second)
	}
	// 3) 组 KILL 清扫:无视 TERM 的子进程(chromium/MCP)在此兜底;
	//    已死成员 ESRCH 为空操作。组不可用时跳过(防误伤同组平台进程)。
	if groupOK {
		_ = syscall.Kill(-target, syscall.SIGKILL)
	}
	m.reapCmd()
	m.removePidFile()
	m.tracked = 0
	m.livePid = 0
	m.cmd = nil
}

// Running 返回内核是否存活(受管 pid 失效时 /proc 重发现,结果缓存 2s)。
func (m *KernelManager) Running() (int, bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.tracked != 0 && processAlive(m.tracked) {
		return m.tracked, true
	}
	pid := m.rediscoverLocked(m.tracked)
	if pid > 0 {
		m.livePid = pid
		m.startedAt = processStartWall(pid) // 收养路径:反推真实启动时刻,防 uptime 溢出
		m.writePidFile()
		m.reapCmd()
		return pid, true
	}
	m.tracked = 0
	return 0, false
}

// rediscoverLocked 扫 /proc:cmdline 匹配 python3/-m/qwenpaw/app/--port N,取最大 pid。
func (m *KernelManager) rediscoverLocked(exclude int) int {
	best := 0
	entries, _ := os.ReadDir("/proc")
	for _, e := range entries {
		pid, err := strconv.Atoi(e.Name())
		if err != nil || pid == exclude || pid == os.Getpid() {
			continue
		}
		raw, err := os.ReadFile("/proc/" + e.Name() + "/cmdline")
		if err != nil || len(raw) == 0 {
			continue // 空 cmdline = 僵尸,天然不命中
		}
		argv := strings.Split(strings.TrimRight(string(raw), "\x00"), "\x00")
		if !isKernelArgv(argv, m.port) {
			continue
		}
		if pid > best {
			best = pid
		}
	}
	return best
}

// isKernelArgv 特征匹配:basename 含 python3 且 argv 含 -m/qwenpaw/app/--port N。
func isKernelArgv(argv []string, port int) bool {
	var hasPort bool
	okBase, okMod, okApp := false, false, false
	for i, a := range argv {
		base := filepath.Base(a)
		if strings.HasPrefix(base, "python3") {
			okBase = true
		}
		switch a {
		case "-m":
			okMod = i+1 < len(argv) && argv[i+1] == "qwenpaw"
		case "app":
			okApp = true
		case "--port":
			if i+1 < len(argv) {
				p, err := strconv.Atoi(argv[i+1])
				if err == nil && p == port {
					hasPort = true
				}
			}
		}
	}
	return okBase && okMod && okApp && hasPort
}

// Stats 采样 CPU%(间隔 300ms 两次采 /proc stat)与 RSS。
func (m *KernelManager) Stats() (cpuPercent float64, rssKB int64, uptime time.Duration, ok bool) {
	pid, running := m.Running()
	if !running {
		return 0, 0, 0, false
	}
	u1, s1 := cpuTicks(pid)
	t1 := time.Now()
	time.Sleep(300 * time.Millisecond)
	u2, s2 := cpuTicks(pid)
	t2 := time.Now()
	cpuPercent = (float64(u2-u1+s2-s1) / 100.0) / t2.Sub(t1).Seconds() * 100 / float64(numCPU())
	if cpuPercent < 0 {
		cpuPercent = 0
	}
	rssKB = rssOf(pid)
	_ = t1
	return cpuPercent, rssKB, time.Since(m.startedAt), true
}

// ── /proc 基元(dsh 模式照搬) ──

func processAlive(pid int) bool {
	if pid <= 0 {
		return false
	}
	raw, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return false
	}
	// comm 可含空格/括号:从最后一个 ')' 之后取状态字符
	s := string(raw)
	if i := strings.LastIndex(s, ")"); i >= 0 && i+2 < len(s) {
		switch s[i+2] {
		case 'Z', 'X':
			return false // 僵尸/已死
		}
	}
	return true
}

func cpuTicks(pid int) (utime, stime int64) {
	raw, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return 0, 0
	}
	s := string(raw)
	if i := strings.LastIndex(s, ")"); i >= 0 && i+2 < len(s) {
		fields := strings.Fields(s[i+2:])
		// fields[0]=state, utime=fields[11], stime=fields[12](相对整体第 14/15 列)
		if len(fields) > 12 {
			utime, _ = strconv.ParseInt(fields[11], 10, 64)
			stime, _ = strconv.ParseInt(fields[12], 10, 64)
		}
	}
	return
}

func rssOf(pid int) int64 {
	raw, err := os.ReadFile(fmt.Sprintf("/proc/%d/statm", pid))
	if err != nil {
		return 0
	}
	f := strings.Fields(string(raw))
	if len(f) < 2 {
		return 0
	}
	pages, _ := strconv.ParseInt(f[1], 10, 64)
	return pages * int64(os.Getpagesize()) / 1024
}

func groupOf(pid int) int {
	raw, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return 0
	}
	s := string(raw)
	if i := strings.LastIndex(s, ")"); i >= 0 && i+2 < len(s) {
		fields := strings.Fields(s[i+2:])
		if len(fields) > 2 {
			p, _ := strconv.ParseInt(fields[2], 10, 64) // pgrp
			return int(p)
		}
	}
	return 0
}

func numCPU() int { return runtimeNumCPU() }

// ── pid 文件与收尾 ──

func (m *KernelManager) writePidFile() {
	_ = os.WriteFile(m.pidFile, []byte(fmt.Sprintf("%d\n", m.tracked)), 0o644)
}

func (m *KernelManager) removePidFile() { _ = os.Remove(m.pidFile) }

// reapCmd 回收已结束的受管子进程(Wait 幂等守卫),防僵尸。
func (m *KernelManager) reapCmd() {
	if m.cmd != nil && m.cmd.Process != nil {
		done := make(chan struct{})
		go func() { _ = m.cmd.Wait(); close(done) }()
		select {
		case <-done:
		case <-time.After(3 * time.Second):
		}
	}
	m.cmd = nil
}

func (m *KernelManager) shareDir() string {
	share := os.Getenv("QWENPAW_WORKING_DIR")
	if share == "" {
		share = "/var/apps/qwenpaw/shares/qwenpaw/.qwenpaw"
	}
	return share
}

// processStartWall 从 /proc 反推进程的真实启动时刻(墙钟)。
func processStartWall(pid int) time.Time {
	statRaw, err1 := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	upRaw, err2 := os.ReadFile("/proc/uptime")
	if err1 != nil || err2 != nil {
		return time.Now()
	}
	s := string(statRaw)
	idx := strings.LastIndex(s, ")")
	if idx < 0 || idx+2 >= len(s) {
		return time.Now()
	}
	fields := strings.Fields(s[idx+2:])
	if len(fields) < 20 {
		return time.Now()
	}
	startTicks, _ := strconv.ParseInt(fields[19], 10, 64)
	upFields := strings.Fields(string(upRaw))
	uptimeSec, _ := strconv.ParseFloat(upFields[0], 64)
	bootWall := time.Now().Add(-time.Duration(uptimeSec * float64(time.Second)))
	return bootWall.Add(time.Duration(float64(startTicks) / 100.0 * float64(time.Second)))
}
