package main

// 监督者入口:env 解析 → 日志 → pid → HTTP(127.0.0.1)→ 信号处理。
// fnOS 停止应用 = TERM 本进程(Supervisor.pid)→ 优雅级联停内核 → 退出(D4)。
// 监督者被 SIGKILL 时内核存活,下次启动由 /proc 收养(D1)。

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"syscall"
	"time"
)

type App struct {
	cfg     Config
	kernel  *KernelManager
	token   string
	events  *Broadcaster
	install *InstallState
}

func main() {
	cfg := LoadConfig()
	os.MkdirAll(cfg.VarDir, 0o755)
	os.MkdirAll(cfg.ShareDir, 0o755)

	logF, err := os.OpenFile(cfg.SupervisorLog, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		fmt.Fprintln(os.Stderr, "open log:", err)
		os.Exit(1)
	}
	logf := func(format string, a ...any) {
		fmt.Fprintf(logF, "[Supervisor] %s %s\n", time.Now().Format("2006/01/02 15:04:05"), fmt.Sprintf(format, a...))
	}

	events := NewBroadcaster()
	inst := NewInstallState(filepath.Join(cfg.VarDir, "qwenpaw-upgrade.log"),
		func() { events.Notify() },
		func(f string, a ...any) { fmt.Fprintf(logF, "[Supervisor] "+f+"\n", a...) })
	app := &App{
		cfg:     cfg,
		kernel:  NewKernelManager(cfg.KernelPidFile, cfg.KernelLog, cfg.KernelPort),
		events:  events,
		install: inst,
	}

	// token:0600,缺失则生成;cgi 与监督者同读
	if b, err := os.ReadFile(cfg.TokenFile); err == nil && len(b) >= 32 {
		app.token = string(b)
	} else {
		buf := make([]byte, 32)
		rand.Read(buf)
		app.token = hex.EncodeToString(buf)
		os.WriteFile(cfg.TokenFile, []byte(app.token), 0o600)
	}
	logf("token file ready (%s)", cfg.TokenFile)

	// supervisor 自身 pid(平台 stop 的目标)
	os.WriteFile(cfg.SupervisorPidFile, []byte(fmt.Sprintf("%d\n", os.Getpid())), 0o644)

	// 启动时收养存活内核(D1:/proc 扫描,不 Pdeathsig——内核须在监督者重启后存活)
	if pid, running := app.kernel.Running(); running {
		logf("adopted running kernel pid=%d", pid)
	} else if app.cfg.AutoStart {
		logf("kernel not running, autostart")
		if err := app.kernel.Spawn(cfg.KernelVenv, cfg.KernelPort); err != nil {
			logf("autostart failed: %v (not_installed?)", err)
		}
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/", app.route)

	addr := net.JoinHostPort("127.0.0.1", strconv.Itoa(cfg.ListenPort))
	srv := &http.Server{Addr: addr, Handler: mux}

	go func() {
		logf("listening %s", addr)
		if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			logf("listen: %v", err)
			os.Exit(1)
		}
	}()

	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT, syscall.SIGQUIT)
	s := <-sig
	logf("received %v, stopping kernel ...", s)
	app.kernel.Stop()
	os.Remove(cfg.SupervisorPidFile)
	logf("supervisor exit")
}
