package main

// 内核安装状态机 —— 模式照搬 dsh server.go(单槽状态 + Seq 去重 + 终态唯一写入者)。
// 纪律(S8/S4):不编造百分比,只报"已获取 N 项"+ 尾行;取消只发信号,终态唯一写入者是安装协程;
// 失败清理半成品。

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"regexp"
	"sync"
	"syscall"
	"time"
)

type InstallPhase string

const (
	PhaseIdle        InstallPhase = "idle"
	PhaseDownloading InstallPhase = "downloading"
	PhaseInstalling  InstallPhase = "installing"
	PhaseDone        InstallPhase = "done"
	PhaseFailed      InstallPhase = "failed"
)

type InstallState struct {
	mu       sync.Mutex
	Phase    InstallPhase
	Fetched  int
	Message  string
	Error    string
	Seq      int // 第几次安装,前端"结果只提示一次"的去重键
	Running  bool
	StartAt  time.Time
	logPath  string
	notify   func()
	logf     func(string, ...any)
}

func NewInstallState(logPath string, notify func(), logf func(string, ...any)) *InstallState {
	return &InstallState{Phase: PhaseIdle, logPath: logPath, notify: notify, logf: logf}
}

func (is *InstallState) Snapshot() map[string]any {
	is.mu.Lock()
	defer is.mu.Unlock()
	return map[string]any{
		"phase":   string(is.Phase),
		"fetched": is.Fetched,
		"message": is.Message,
		"error":   is.Error,
		"seq":     is.Seq,
		"running": is.Running,
	}
}

func (is *InstallState) update(phase InstallPhase, fetched int, msg string) {
	is.mu.Lock()
	is.Phase = phase
	is.Fetched = fetched
	is.Message = msg
	is.mu.Unlock()
	is.notify()
}

var reCollect = regexp.MustCompile(`^Collecting\s+(\S+)`)
var reDownload = regexp.MustCompile(`^Downloading\s+\S+`)
var reAdded = regexp.MustCompile(`Successfully installed (.+)`)

// RunInstall 以独立进程组运行 pip,逐行解析进度;终态唯一写入者是本协程。
func (is *InstallState) RunInstall(venvDir, spec, mirrorURL string, onDone func(ok bool)) {
	is.mu.Lock()
	if is.Running {
		is.mu.Unlock()
		return
	}
	is.Running = true
	is.Seq++
	is.Phase = PhaseDownloading
	is.Error = ""
	is.Fetched = 0
	is.Message = "启动安装"
	is.StartAt = time.Now()
	seq := is.Seq
	is.mu.Unlock()
	is.notify()

	mirror := ""
	if mirrorURL != "" {
		mirror = "-i " + mirrorURL
	}
	cmd := exec.Command("bash", "-c",
		fmt.Sprintf("source %s/bin/activate && python3 -m pip install --upgrade %s %s",
			venvDir, spec, mirror))
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true} // 独立进程组,失败可整组清扫
	logF, _ := os.OpenFile(is.logPath+".pip", os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if logF != nil {
		defer logF.Close()
		cmd.Stdout = logF
		cmd.Stderr = logF
	}
	pipe, err := cmd.StdoutPipe()
	if err != nil {
		is.mu.Lock()
		is.Running = false
		is.Phase = PhaseFailed
		is.Error = err.Error()
		is.mu.Unlock()
		is.notify()
		return
	}

	go func() {
		defer func() {
			is.mu.Lock()
			is.Running = false
			is.mu.Unlock()
			is.notify()
		}()
		if err := cmd.Start(); err != nil {
			is.mu.Lock()
			is.Phase = PhaseFailed
			is.Error = err.Error()
			is.mu.Unlock()
			is.notify()
			return
		}
		sc := bufio.NewScanner(pipe)
		sc.Buffer(make([]byte, 0, 64*1024), 1024*1024)
		for sc.Scan() {
			line := sc.Text()
			is.mu.Lock()
			prev := is.Message
			is.mu.Unlock()
			if m := reCollect.FindStringSubmatch(line); m != nil {
				is.update(PhaseDownloading, is.fetchedSnapshot()+1, "获取 "+m[1])
			} else if reDownload.MatchString(line) {
				is.update(PhaseDownloading, is.fetchedSnapshot(), "下载中")
			} else if reAdded.MatchString(line) {
				is.update(PhaseInstalling, is.fetchedSnapshot(), "安装依赖包")
			} else if line != prev {
				is.update(is.phaseSnapshot(), is.fetchedSnapshot(), tail(line, 400))
			}
		}
		_ = cmd.Wait()
		is.mu.Lock()
		ok := cmd.ProcessState.Success()
		is.mu.Unlock()
		if ok {
			is.update(PhaseDone, is.fetchedSnapshot(), "安装完成")
			is.logf("kernel install ok (seq=%d)", seq)
		} else {
			is.mu.Lock()
			is.Phase = PhaseFailed
			is.Error = "pip 退出码非零,详见 " + is.logPath + ".pip"
			is.mu.Unlock()
			is.logf("kernel install failed (seq=%d)", seq)
		}
		is.notify()
	}()
}

func (is *InstallState) fetchedSnapshot() int {
	is.mu.Lock()
	defer is.mu.Unlock()
	return is.Fetched
}

func (is *InstallState) phaseSnapshot() InstallPhase {
	is.mu.Lock()
	defer is.mu.Unlock()
	return is.Phase
}

func tail(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[len(s)-n:]
}
