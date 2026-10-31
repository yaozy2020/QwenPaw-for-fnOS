package main

// 路径总源头(dsh 模式):TRIM_* 注入优先,缺省为本机安装布局。
// 所有自产文件统一挂 VarDir 之下。

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

type Config struct {
	VarDir            string
	EtcDir            string
	HomeDir           string
	ShareDir          string
	KernelVenv        string
	KernelPidFile     string
	KernelLog         string
	KernelPort        int
	TokenFile         string
	SupervisorPidFile string
	SupervisorLog     string
	ListenPort        int
	AutoStart         bool
}

func LoadConfig() Config {
	varDir := envOr("TRIM_PKGVAR", "/var/apps/qwenpaw/var")
	etcDir := envOr("TRIM_PKGETC", "/var/apps/qwenpaw/etc")
	homeDir := envOr("TRIM_PKGHOME", "/var/apps/qwenpaw/home")
	share := os.Getenv("TRIM_DATA_SHARE_PATHS")
	shareDir := "/var/apps/qwenpaw/shares/qwenpaw/.qwenpaw"
	if i := strings.Index(share, ":"); i >= 0 {
		shareDir = strings.TrimSuffix(share[:i], "/") + "/.qwenpaw"
	} else if strings.TrimSpace(share) != "" {
		shareDir = strings.TrimSuffix(share, "/") + "/.qwenpaw"
	}

	kernelPort := 18088
	if p := os.Getenv("TRIM_SERVICE_PORT"); p != "" {
		if v, err := strconv.Atoi(p); err == nil && v >= 1024 && v <= 65535 {
			kernelPort = v
		}
	}
	if b, err := os.ReadFile(filepath.Join(varDir, "service_port")); err == nil {
		if v, err := strconv.Atoi(strings.TrimSpace(string(b))); err == nil && v >= 1024 && v <= 65535 && v != 19091 {
			kernelPort = v
		}
	}

	listen := 19090
	if p := os.Getenv("SUPERVISOR_PORT"); p != "" {
		if v, err := strconv.Atoi(p); err == nil && v >= 1024 && v <= 65535 {
			listen = v
		}
	}

	return Config{
		VarDir:            varDir,
		EtcDir:            etcDir,
		HomeDir:           homeDir,
		ShareDir:          shareDir,
		KernelVenv:        filepath.Join(homeDir, "venv"),
		KernelPidFile:     filepath.Join(varDir, "kernel.pid"),
		KernelLog:         filepath.Join(varDir, "qwenpaw.log"),
		KernelPort:        kernelPort,
		TokenFile:         filepath.Join(varDir, "supervisor-token"),
		SupervisorPidFile: filepath.Join(varDir, "supervisor.pid"),
		SupervisorLog:     filepath.Join(varDir, "supervisor.log"),
		ListenPort:        listen,
		AutoStart:         os.Getenv("SUPERVISOR_AUTOSTART") != "0",
	}
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); strings.TrimSpace(v) != "" {
		return v
	}
	return fallback
}
