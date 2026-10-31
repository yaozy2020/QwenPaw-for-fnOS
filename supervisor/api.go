package main

// 路由与处理器。鉴权:X-Supervisor-Token 头(vs VAR_DIR/supervisor-token,0600)。
// 监督者只绑 127.0.0.1;控制台经 cgi 桥接访问(App 切后台不影响本服务)。

import (
	"crypto/subtle"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

func (a *App) route(w http.ResponseWriter, r *http.Request) {
	if !a.checkToken(r) {
		writeErr(w, http.StatusUnauthorized, "invalid token")
		return
	}
	switch {
	case r.URL.Path == "/status" && r.Method == http.MethodGet:
		a.handleStatus(w, r)
	case r.URL.Path == "/start" && r.Method == http.MethodPost:
		a.handleStart(w)
	case r.URL.Path == "/stop" && r.Method == http.MethodPost:
		a.handleStop(w)
	case r.URL.Path == "/restart" && r.Method == http.MethodPost:
		a.handleRestart(w)
	case r.URL.Path == "/events" && r.Method == http.MethodGet:
		a.handleEvents(w, r)
	case r.URL.Path == "/kernel/install" && r.Method == http.MethodPost:
		a.handleKernelInstall(w, r)
	case r.URL.Path == "/kernel/versions" && r.Method == http.MethodGet:
		a.handleKernelVersions(w)
	case r.URL.Path == "/kernel/pin" && r.Method == http.MethodPost:
		a.handleKernelPin(w, r)
	default:
		writeErr(w, http.StatusNotFound, "not found")
	}
}

func (a *App) checkToken(r *http.Request) bool {
	got := r.Header.Get("X-Supervisor-Token")
	if got == "" {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(got), []byte(a.token)) == 1
}

func writeErr(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": msg})
}

func writeOK(w http.ResponseWriter, extra map[string]any) {
	w.Header().Set("Content-Type", "application/json")
	m := map[string]any{"ok": true}
	for k, v := range extra {
		m[k] = v
	}
	json.NewEncoder(w).Encode(m)
}

// kernelVersion 经 venv python 查询内核版本(3s 超时)。
func (a *App) kernelVersion() string {
	out, err := exec.Command(filepath.Join(a.cfg.KernelVenv, "bin", "python3"), "-c",
		"import importlib.metadata as m; print(m.version('qwenpaw'))").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func (a *App) handleStatus(w http.ResponseWriter, r *http.Request) {
	pid, running := a.kernel.Running()
	cpu, mem, uptime, _ := a.kernel.Stats()
	inst := a.install.Snapshot()
	writeOK(w, map[string]any{
		"running":   running,
		"pid":       pid,
		"version":   a.kernelVersion(),
		"installed": a.kernelVersion() != "",
		"cpu":       cpu,
		"memKB":     mem,
		"uptimeSec": int(uptime.Seconds()),
		"port":      a.cfg.KernelPort,
		"install":   inst,
	})
}

func (a *App) handleStart(w http.ResponseWriter) {
	if !a.kernelInstalledFS() {
		writeErr(w, http.StatusConflict, "内核尚未安装,请在「版本更新」中选择版本安装")
		return
	}
	if pid, running := a.kernel.Running(); running {
		writeOK(w, map[string]any{"message": fmt.Sprintf("已在运行 pid=%d", pid), "pid": pid})
		return
	}
	if err := a.kernel.Spawn(a.cfg.KernelVenv, a.cfg.KernelPort); err != nil {
		writeErr(w, http.StatusInternalServerError, err.Error())
		return
	}
	a.events.Notify()
	writeOK(w, map[string]any{"message": "QwenPaw 已启动"})
}

func (a *App) handleStop(w http.ResponseWriter) {
	a.kernel.Stop()
	a.events.Notify()
	writeOK(w, map[string]any{"message": "QwenPaw 已停止"})
}

func (a *App) handleRestart(w http.ResponseWriter) {
	a.kernel.Stop()
	if err := a.kernel.Spawn(a.cfg.KernelVenv, a.cfg.KernelPort); err != nil {
		writeErr(w, http.StatusInternalServerError, err.Error())
		return
	}
	a.events.Notify()
	writeOK(w, map[string]any{"message": "QwenPaw 已重启"})
}

// handleEvents SSE:首帧全量快照 + 1s 快照流(心跳由数据本身承载)。
func (a *App) handleEvents(w http.ResponseWriter, r *http.Request) {
	setSSEHeaders(w)
	ch, unsub := a.events.Subscribe()
	defer unsub()

	snap := a.fullSnapshot()
	sseSend(w, "snapshot", snap)
	ctx := r.Context()
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ch:
			if !sseSend(w, "snapshot", a.fullSnapshot()) {
				return
			}
		case <-ticker.C:
			if !sseSend(w, "snapshot", a.fullSnapshot()) {
				return
			}
		}
	}
}

func (a *App) fullSnapshot() map[string]any {
	pid, running := a.kernel.Running()
	cpu, mem, uptime, _ := a.kernel.Stats()
	inst := a.install.Snapshot()
	v := a.kernelVersion()
	return map[string]any{
		"running":   running,
		"pid":       pid,
		"version":   v,
		"installed": v != "",
		"cpu":       cpu,
		"memKB":     mem,
		"uptimeSec": int(uptime.Seconds()),
		"port":      a.cfg.KernelPort,
		"install":   inst,
	}
}

func (a *App) handleKernelInstall(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Version string `json:"version"`
		Mirror  string `json:"mirror"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || strings.TrimSpace(req.Version) == "" {
		writeErr(w, http.StatusBadRequest, "缺少 version")
		return
	}
	spec := "qwenpaw"
	switch req.Version {
	case "latest":
	case "beta", "pre":
		spec = "--pre qwenpaw"
	default:
		spec = "qwenpaw==" + req.Version
	}
	mirror := ""
	switch req.Mirror {
	case "tsinghua":
		mirror = "https://pypi.tuna.tsinghua.edu.cn/simple"
	case "aliyun":
		mirror = "https://mirrors.aliyun.com/pypi/simple/"
	case "ustc":
		mirror = "https://pypi.mirrors.ustc.edu.cn/simple/"
	}
	// 安装期间先停内核(pip 换包需停服);装完由前端触发重启
	a.kernel.Stop()
	go func() {
		a.install.RunInstall(a.cfg.KernelVenv, spec, mirror, nil)
	}()
	writeOK(w, map[string]any{"message": "安装已开始"})
}

func (a *App) handleKernelVersions(w http.ResponseWriter) {
	out, err := exec.Command("python3", "-c", `
import urllib.request, json
req = urllib.request.Request('https://pypi.org/pypi/qwenpaw/json', headers={'User-Agent': 'QwenPaw'})
data = json.loads(urllib.request.urlopen(req, timeout=5).read())
releases = sorted(data.get('releases', {}).keys())
print(json.dumps({'releases': releases[-14:], 'latest_stable': data['info']['version']}))
`).Output()
	if err != nil {
		writeErr(w, http.StatusBadGateway, "查询 PyPI 失败")
		return
	}
	var payload map[string]any
	json.Unmarshal(out, &payload)
	writeOK(w, payload)
}

func (a *App) handleKernelPin(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Version string `json:"version"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.Version == "" {
		writeErr(w, http.StatusBadRequest, "缺少 version")
		return
	}
	if err := os.WriteFile(a.cfg.VarDir+"/qwenpaw-version", []byte(req.Version+"\n"), 0o644); err != nil {
		writeErr(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeOK(w, map[string]any{"message": "版本钉已更新: " + req.Version})
}

// kernelInstalledFS 以文件系统为准(venv python 可导入 qwenpaw)。
func (a *App) kernelInstalledFS() bool {
	return a.kernelVersion() != ""
}
