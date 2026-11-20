//! 内核进程管理器 —— 模式裁剪自 dsh backend/dsh.go(MIT)。
//! 血泪清单(每条都有 dsh 线上事故背书,勿删):
//!   - 只按 PID/进程组杀,绝不按进程名杀(误杀持锁进程会留陈旧锁)
//!   - 僵尸判定看 stat 状态字符,不看目录存在(procfs.rs)
//!   - 被管 pid 失效时扫 /proc 重新发现(对账兜底),结果缓存 2s
//!   - 停止三段式:组 TERM(宽限)→ 单 KILL → 组 KILL 清扫
//!   - 不用 Pdeathsig:内核必须在监督者重启(外壳升级)后存活,由 /proc 收养

use std::{
    collections::BTreeMap,
    os::unix::process::CommandExt,
    path::Path,
    sync::{
        atomic::{AtomicU16, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime},
};

use crate::procfs;

#[derive(Clone, Debug)]
pub struct KernelConfig {
    pub venv_dir: String,
    pub pid_file: String,
    pub log_file: String,
    pub share_dir: String,
    pub etc_dir: String,
    /// 内核日志级别(向导落盘 → config.rs 白名单校验),spawn 时经 --log-level 下发
    pub log_level: String,
}

fn signal_verified(identity: procfs::ProcessIdentity, signal: i32) -> Result<(), String> {
    use std::os::fd::FromRawFd;

    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) as i32 };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        if matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS) | Some(libc::EINVAL)
        ) {
            match procfs::identity_status(identity) {
                procfs::IdentityStatus::Gone => return Ok(()),
                procfs::IdentityStatus::Reused => {
                    return Err(format!(
                        "PID {} identity changed before signal",
                        identity.pid
                    ));
                }
                procfs::IdentityStatus::Unknown(error) => return Err(error),
                procfs::IdentityStatus::Alive => {}
            }
            if unsafe { libc::kill(identity.pid, signal) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(format!("signal PID {}: {error}", identity.pid))
            };
        }
        return Err(format!("open pidfd for {}: {error}", identity.pid));
    }
    let pidfd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    match procfs::identity_status(identity) {
        procfs::IdentityStatus::Gone => return Ok(()),
        procfs::IdentityStatus::Reused => {
            return Err(format!(
                "PID {} identity changed before signal",
                identity.pid
            ));
        }
        procfs::IdentityStatus::Unknown(error) => return Err(error),
        procfs::IdentityStatus::Alive => {}
    }
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            std::os::fd::AsRawFd::as_raw_fd(&pidfd),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(format!("signal PID {}: {error}", identity.pid))
        }
    }
}

fn signal_group_verified(identity: procfs::ProcessIdentity, signal: i32) -> Result<(), String> {
    if identity.pgrp <= 1 || identity.pgrp != identity.pid {
        return Err(format!(
            "refusing to signal unverified process group {}",
            identity.pgrp
        ));
    }
    match procfs::identity_status(identity) {
        procfs::IdentityStatus::Alive => {}
        procfs::IdentityStatus::Gone => {
            if procfs::processes_in_group(identity.pgrp, identity.session)?.is_empty() {
                return Ok(());
            }
        }
        procfs::IdentityStatus::Reused => {
            return Err(format!(
                "process group leader PID {} identity changed",
                identity.pid
            ));
        }
        procfs::IdentityStatus::Unknown(error) => return Err(error),
    }
    if unsafe { libc::kill(-identity.pgrp, signal) } == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(format!("signal process group {}: {error}", identity.pgrp))
        }
    }
}

fn merge_identity(
    processes: &mut BTreeMap<i32, procfs::ProcessIdentity>,
    identity: procfs::ProcessIdentity,
) {
    processes.entry(identity.pid).or_insert(identity);
}

pub struct KernelManager {
    cfg: KernelConfig,
    port: AtomicU16,
    log_level: Mutex<String>,
    state: Mutex<State>,
    version_probe: Mutex<()>,
    /// 进程生命周期操作的序列化锁。
    ///
    /// API 侧的运维互斥锁(tokio Mutex)只能串行化 HTTP 处理器之间的操作;
    /// 安装线程直接调用 spawn/stop 且全程不持有那把锁,因此存在这样的交错:
    /// 用户 restart 已 stop 完、正要 spawn,安装线程先一步 spawn 成功,
    /// 随后 restart 再 spawn 撞上"已在运行"并把它报成重启失败 —— 实际留下了
    /// 一个新内核,状态却显示失败。用 std 锁覆盖两条路径,保证 spawn/stop 不交错。
    lifecycle: Mutex<()>,
}

#[derive(Default)]
struct State {
    shutdown: bool,
    tracked: i32,
    tracked_identity: Option<procfs::ProcessIdentity>,
    native_proxy_identity: Option<procfs::ProcessIdentity>,
    pending_stop: Vec<procfs::ProcessIdentity>,
    last_stop_clean: bool,
    stop_diagnostics: Vec<String>,
    started_wall: Option<SystemTime>,
    cache_pid: i32,
    cache_at: Option<Instant>,
    last_ticks: Option<(i32, u64, Instant, u64)>,
    cache_cpu: Option<f64>,
    cache_rss_kb: i64,
    display_probe_pending: bool,
    version_generation: u64,
    version_cache: String,
    version_cache_at: Option<Instant>,
}

impl State {
    fn sample(&mut self, now: Instant) -> bool {
        let Some(id) = self.tracked_identity.filter(|id| id.pid == self.tracked) else {
            return false;
        };
        let Some(ticks) = procfs::cpu_ticks(id) else {
            return false;
        };
        let previous = self
            .last_ticks
            .filter(|(pid, start, _, _)| (*pid, *start) == (id.pid, id.starttime));
        if previous.is_some_and(|(_, _, at, _)| now.duration_since(at) < Duration::from_millis(250))
        {
            return true;
        }
        self.cache_cpu = previous.and_then(|(_, _, at, old)| {
            let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            let cores = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            let delta = ticks.checked_sub(old)? as f64;
            (hz > 0).then(|| {
                (100.0 * delta / hz as f64 / now.duration_since(at).as_secs_f64() / cores as f64)
                    .clamp(0.0, 100.0)
            })
        });
        self.cache_rss_kb = procfs::rss_kb(id.pid);
        self.last_ticks = Some((id.pid, id.starttime, now, ticks));
        true
    }
}

#[derive(Clone, Debug)]
pub struct KernelStatus {
    pub pid: i32,
    pub running: bool,
    pub adopted: bool,
    pub started_at: Option<SystemTime>,
    pub cpu_percent: Option<f64>,
    pub rss_kb: i64,
    pub port: u16,
}

impl KernelManager {
    pub fn new(cfg: KernelConfig, port: u16) -> Self {
        Self {
            log_level: Mutex::new(cfg.log_level.clone()),
            cfg,
            port: AtomicU16::new(port),
            state: Mutex::new(State::default()),
            lifecycle: Mutex::new(()),
            version_probe: Mutex::new(()),
        }
    }

    pub fn entry_identity(&self) -> Option<procfs::ProcessIdentity> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tracked_identity
    }

    pub fn supports_native_proxy(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.native_proxy_identity.is_some() && s.native_proxy_identity == s.tracked_identity
    }

    pub fn entry_record(&self) -> std::path::PathBuf {
        crate::entry_login::path(Path::new(&self.cfg.pid_file))
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    pub fn log_level(&self) -> String {
        self.log_level
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_log_level(&self, level: &str) -> Result<(), String> {
        if !matches!(
            level,
            "critical" | "error" | "warning" | "info" | "debug" | "trace"
        ) {
            return Err("日志级别无效".into());
        }
        *self.log_level.lock().unwrap_or_else(|e| e.into_inner()) = level.into();
        Ok(())
    }

    pub fn kernel_version(&self) -> String {
        self.kernel_version_with_timeout(Duration::from_secs(15))
    }

    fn kernel_version_with_timeout(&self, timeout: Duration) -> String {
        let _probe = self.version_probe.lock().unwrap_or_else(|e| e.into_inner());
        let log_level = self.log_level();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .version_cache_at
            .map(|at| at.elapsed() < Duration::from_secs(60))
            .unwrap_or(false)
        {
            return state.version_cache.clone();
        }
        let generation = state.version_generation;
        drop(state);
        let bin = format!("{}/bin/python3", self.cfg.venv_dir);
        let runtime = crate::runtime::RuntimeAdapter::new(
            &self.cfg.venv_dir,
            &self.cfg.share_dir,
            &self.cfg.etc_dir,
            log_level,
        );
        let cmd = runtime.metadata_command_for(Path::new(&bin));
        let version =
            match crate::install::run_with_timeout(cmd, timeout, "读取内核版本(缓存失效路径)")
            {
                Ok(o) if o.status.success() => {
                    String::from_utf8_lossy(&o.stdout).trim().to_string()
                }
                _ => String::new(),
            };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if generation != state.version_generation {
            return String::new();
        }
        state.version_cache = version.clone();
        state.version_cache_at = Some(Instant::now());
        version
    }

    pub fn display_version(
        self: &std::sync::Arc<Self>,
        updated: impl FnOnce() + Send + 'static,
    ) -> String {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let current = state.version_cache.clone();
        if !state.display_probe_pending
            && !state
                .version_cache_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            state.display_probe_pending = true;
            let kernel = self.clone();
            let previous = current.clone();
            let generation = state.version_generation;
            if std::thread::Builder::new()
                .name("status-version".into())
                .spawn(move || {
                    let version = kernel.kernel_version();
                    let mut state = kernel.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.display_probe_pending = false;
                    let invalidated = generation != state.version_generation;
                    drop(state);
                    if invalidated || previous != version {
                        updated();
                    }
                })
                .is_err()
            {
                state.display_probe_pending = false;
            }
        }
        current
    }

    pub fn installed(&self) -> bool {
        Path::new(&self.cfg.venv_dir).join("bin/python3").exists()
    }

    pub fn invalidate_version(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.version_generation = state.version_generation.wrapping_add(1);
        state.version_cache.clear();
        state.version_cache_at = None;
    }

    pub fn prevent_start(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shutdown = true;
    }

    pub fn last_stop_clean(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_stop_clean
    }

    pub fn stop_verified_sync(&self) -> Result<(), String> {
        self.stop_sync();
        self.stop_verification_error().map_or(Ok(()), Err)
    }

    pub fn stop_verification_error(&self) -> Option<String> {
        let clean = self.last_stop_clean();
        let status = self.status_now();
        if clean && !status.running {
            None
        } else {
            Some(format!(
                "内核停止核验失败: last_stop_clean={clean}, running={}, pid={}",
                status.running, status.pid
            ))
        }
    }

    fn record_stop_failure(
        &self,
        target: i32,
        target_identity: Option<procfs::ProcessIdentity>,
        pending: Vec<procfs::ProcessIdentity>,
        diagnostics: Vec<String>,
    ) {
        if let Ok(mut log) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.cfg.log_file)
        {
            use std::io::Write as _;
            for diagnostic in &diagnostics {
                let _ = writeln!(log, "[kernel] stop verification failed: {diagnostic}");
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.last_stop_clean = false;
        state.stop_diagnostics = diagnostics;
        state.pending_stop = pending;
        if target > 0 {
            state.tracked = target;
            state.tracked_identity = target_identity;
            state.cache_pid = target;
        }
    }

    /// 同步启动内核(无 await;由 axum 处理器与安装完成回调直接调用)。
    /// 以独立进程组启动(process_group(0) ≈ setsid)。
    pub fn spawn(&self) -> Result<i32, String> {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.shutdown {
                return Err("supervisor is shutting down".into());
            }
            if st.tracked != 0 && procfs::process_alive(st.tracked) {
                return Err(format!("kernel already running pid={}", st.tracked));
            }
        }
        let bin = format!("{}/bin/python3", self.cfg.venv_dir);
        if !Path::new(&bin).exists() {
            return Err("kernel not installed".into());
        }
        let port = self.port();
        let playwright_host = std::env::var("QWENPAW_PLAYWRIGHT_DOWNLOAD_HOST")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "https://npmmirror.com/mirrors/playwright".into());
        // std::process::Command:spawn 可从任意线程调用(安装线程的失败自愈/
        // on_success 都在裸线程上,tokio Command 在无 runtime 上下文会 panic ——
        // 审计 B2 实测:panic + 日志锁毒化,一次 pip 失败砖死安装子系统)
        let mut cmd = std::process::Command::new(&bin);
        let log_level = self.log_level();
        cmd.args([
            "-m",
            "qwenpaw",
            "app",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--log-level",
            &log_level,
        ])
        .current_dir(&self.cfg.share_dir)
        .env("HOME", &self.cfg.etc_dir)
        .env("QWENPAW_WORKING_DIR", &self.cfg.share_dir)
        .env("QWENPAW_AUTH_ENABLED", "true")
        .env("QWENPAW_LOG_LEVEL", &log_level)
        .env(
            "QWENPAW_DEPENDENCY_STATE_DIR",
            format!("{}/dependency-environments", self.cfg.share_dir),
        )
        .env("PLAYWRIGHT_DOWNLOAD_HOST", playwright_host)
        .env(
            "PATH",
            "/var/apps/nodejs_v24/target/bin:/usr/local/bin:/usr/bin:/bin",
        )
        .process_group(0); // 独立进程组:停止时组信号可连带浏览器/MCP 子进程
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.cfg.log_file)
            .map_err(|e| format!("open kernel log: {e}"))?;
        if crate::install::base_path_patch::active(Path::new(&self.cfg.venv_dir)) {
            cmd.env("QWENPAW_BASE_PATH", "/app/qwenpaw/web");
        }
        cmd.env("QWENPAW_FNOS_ENTRY_RECORD", self.entry_record());
        cmd.env("QWENPAW_FNOS_ENTRY_PARENT", std::process::id().to_string());
        let compatible = crate::entry_login::compatible(Path::new(&self.cfg.venv_dir));
        if crate::entry_login::selected(&self.entry_record()) && !compatible {
            return Err("当前内核不支持已迁移的入口鉴权，拒绝回退启动".into());
        }
        cmd.stdout(std::process::Stdio::from(
            log.try_clone()
                .map_err(|e| format!("clone kernel log: {e}"))?,
        ));
        cmd.stderr(std::process::Stdio::from(log));
        let mut child = cmd.spawn().map_err(|e| format!("spawn kernel: {e}"))?;
        let pid = child.id() as i32;
        // 原子写:裸 write 在断电/并发下可能留下半截 pid,导致后续识别错乱
        let _ = crate::install::atomic_write_0600(&self.cfg.pid_file, &format!("{pid}\n"));
        // 收尾线程:wait() 回收僵尸并落退出日志;std Child drop 不杀进程,
        // 内核生命周期仍由自身管理(与原 kill_on_drop(false) 语义一致)
        let log_path = self.cfg.log_file.clone();
        std::thread::spawn(move || {
            use std::io::Write as _;
            if let Ok(st) = child.wait() {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                {
                    let _ = writeln!(f, "[kernel] pid exited: {st}");
                }
            }
        });
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.tracked = pid;
            st.tracked_identity = procfs::process_identity(pid).ok().flatten();
            st.native_proxy_identity = st.tracked_identity.filter(|_| compatible);
            st.started_wall = procfs::started_wall(pid);
            st.cache_pid = pid;
            st.cache_at = Some(Instant::now());
        }
        Ok(pid)
    }

    pub(crate) fn base_path(&self) -> &'static str {
        if crate::install::base_path_patch::active(Path::new(&self.cfg.venv_dir)) {
            "/app/qwenpaw/web"
        } else {
            ""
        }
    }

    /// 启动后健康检查:进程存活且 /api/version 在总期限内返回当前版本。
    /// "进程已创建"不等于"服务成功" —— 改端口/重置认证的成功判定以此为准,
    /// 失败路径据此回滚(超时视为失败,由调用方处理)。
    /// 阻塞式:调用方(axum 处理器)须经 spawn_blocking 包裹。
    pub fn wait_healthy(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let pid = self.state.lock().unwrap_or_else(|e| e.into_inner()).tracked;
        let expected_version =
            self.kernel_version_with_timeout(deadline.saturating_duration_since(Instant::now()));
        let remaining = deadline.saturating_duration_since(Instant::now());
        if pid <= 0
            || !procfs::process_alive(pid)
            || expected_version.is_empty()
            || remaining.is_zero()
        {
            return false;
        }
        let base_path = self.base_path();
        crate::install::wait_for_version_deadline_at(
            self.port(),
            &expected_version,
            deadline,
            base_path,
        )
    }

    /// 三段式停止:组 TERM(宽限 10s)→ 单 KILL → 组 KILL 清扫。
    /// 异步非阻塞接口:将阻塞的睡眠轮询放进 spawn_blocking,不占用 Tokio 工作线程
    pub async fn stop(self: &std::sync::Arc<Self>) {
        let this = std::sync::Arc::clone(self);
        let _ = tokio::task::spawn_blocking(move || this.stop_sync()).await;
    }

    pub fn stop_sync(&self) {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        let (mut target, mut target_identity, pending) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.last_stop_clean = false;
            state.stop_diagnostics.clear();
            (
                state.tracked,
                state.tracked_identity,
                state.pending_stop.clone(),
            )
        };
        let mut diagnostics = Vec::new();
        let mut processes = BTreeMap::new();
        for identity in pending {
            merge_identity(&mut processes, identity);
        }

        if target > 0 && target_identity.is_none() {
            match procfs::process_identity(target) {
                Ok(Some(identity)) => target_identity = Some(identity),
                Ok(None) => {}
                Err(error) => diagnostics.push(error),
            }
        }
        if let Some(identity) = target_identity {
            match procfs::identity_status(identity) {
                procfs::IdentityStatus::Alive => merge_identity(&mut processes, identity),
                procfs::IdentityStatus::Gone => {}
                procfs::IdentityStatus::Reused => diagnostics.push(format!(
                    "tracked PID {} identity changed: recorded starttime {} no longer matches the current process",
                    identity.pid, identity.starttime
                )),
                procfs::IdentityStatus::Unknown(error) => diagnostics.push(error),
            }
        }
        match procfs::rediscover_all_in_venv(self.port(), target, &self.cfg.venv_dir) {
            Ok(candidates) => {
                for candidate in candidates {
                    match procfs::process_identity(candidate) {
                        Ok(Some(identity)) => merge_identity(&mut processes, identity),
                        Ok(None) => diagnostics.push(format!(
                            "rediscovered kernel PID {candidate} exited before identity check"
                        )),
                        Err(error) => diagnostics.push(error),
                    }
                }
            }
            Err(error) => diagnostics.push(error),
        }
        target = processes.keys().copied().max().unwrap_or(0);
        target_identity = processes.get(&target).copied();
        if !diagnostics.is_empty() {
            self.record_stop_failure(
                target,
                target_identity,
                processes.values().copied().collect(),
                diagnostics,
            );
            return;
        }
        if target <= 0 {
            processes.retain(|_, identity| match procfs::identity_status(*identity) {
                procfs::IdentityStatus::Gone => false,
                procfs::IdentityStatus::Alive => true,
                procfs::IdentityStatus::Reused => {
                    diagnostics.push(format!("pending PID {} identity changed", identity.pid));
                    true
                }
                procfs::IdentityStatus::Unknown(error) => {
                    diagnostics.push(error);
                    true
                }
            });
            if !diagnostics.is_empty() {
                self.record_stop_failure(
                    0,
                    None,
                    processes.values().copied().collect(),
                    diagnostics,
                );
                return;
            }
            for identity in processes.values().copied() {
                if let Err(error) = signal_verified(identity, libc::SIGTERM) {
                    diagnostics.push(error);
                }
            }
            if !processes.is_empty() {
                std::thread::sleep(Duration::from_secs(1));
            }
            for identity in processes.values().copied() {
                if procfs::identity_status(identity) == procfs::IdentityStatus::Alive {
                    if let Err(error) = signal_verified(identity, libc::SIGKILL) {
                        diagnostics.push(error);
                    }
                }
            }
            for identity in processes.values().copied() {
                match procfs::identity_status(identity) {
                    procfs::IdentityStatus::Gone => {}
                    procfs::IdentityStatus::Alive => {
                        diagnostics.push(format!("pending PID {} still alive", identity.pid))
                    }
                    procfs::IdentityStatus::Reused => diagnostics.push(format!(
                        "pending PID {} identity changed during verification",
                        identity.pid
                    )),
                    procfs::IdentityStatus::Unknown(error) => diagnostics.push(error),
                }
            }
            if !diagnostics.is_empty() {
                self.record_stop_failure(
                    0,
                    None,
                    processes.values().copied().collect(),
                    diagnostics,
                );
                return;
            }
            if let Err(error) = std::fs::remove_file(&self.cfg.pid_file) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    self.record_stop_failure(
                        0,
                        None,
                        Vec::new(),
                        vec![format!("remove stale kernel PID file: {error}")],
                    );
                    return;
                }
            }
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.tracked = 0;
            state.tracked_identity = None;
            state.pending_stop.clear();
            state.cache_pid = 0;
            state.started_wall = None;
            state.stop_diagnostics.clear();
            state.last_stop_clean = true;
            return;
        }

        let target_identity = target_identity.expect("verified kernel target identity");
        let group_ok = target_identity.pgrp == target;
        let group = if group_ok {
            match procfs::processes_in_group(target, target_identity.session) {
                Ok(members) => {
                    for identity in members {
                        merge_identity(&mut processes, identity);
                    }
                    true
                }
                Err(error) => {
                    diagnostics.push(error);
                    false
                }
            }
        } else {
            false
        };
        match procfs::descendants_of_checked(target_identity) {
            Ok(descendants) => {
                for identity in descendants {
                    merge_identity(&mut processes, identity);
                }
            }
            Err(error) => diagnostics.push(error),
        }
        if !diagnostics.is_empty() {
            self.record_stop_failure(
                target,
                Some(target_identity),
                processes.values().copied().collect(),
                diagnostics,
            );
            return;
        }

        // 第一阶段:进程组 TERM + 身份核验后的 leader/后代 TERM,宽限期最多 10 秒。
        if group {
            if let Err(error) = signal_group_verified(target_identity, libc::SIGTERM) {
                diagnostics.push(error);
            }
        }
        let initial: Vec<_> = processes.values().copied().collect();
        for identity in &initial {
            if let Err(error) = signal_verified(*identity, libc::SIGTERM) {
                diagnostics.push(error);
            }
        }
        for _ in 0..10 {
            if procfs::identity_status(target_identity) != procfs::IdentityStatus::Alive {
                break;
            }
            match procfs::descendants_of_checked(target_identity) {
                Ok(descendants) => {
                    for identity in descendants {
                        if !processes.contains_key(&identity.pid) {
                            merge_identity(&mut processes, identity);
                            if let Err(error) = signal_verified(identity, libc::SIGTERM) {
                                diagnostics.push(error);
                            }
                        }
                    }
                }
                Err(error)
                    if procfs::identity_status(target_identity) != procfs::IdentityStatus::Gone =>
                {
                    diagnostics.push(error)
                }
                Err(_) => {}
            }
            if group {
                match procfs::processes_in_group(target, target_identity.session) {
                    Ok(members) => {
                        for identity in members {
                            if !processes.contains_key(&identity.pid) {
                                merge_identity(&mut processes, identity);
                            }
                        }
                    }
                    Err(error) => diagnostics.push(error),
                }
            }
            let all_gone = processes
                .values()
                .all(|identity| procfs::identity_status(*identity) == procfs::IdentityStatus::Gone);
            let group_empty = if group {
                procfs::processes_in_group(target, target_identity.session)
                    .map(|members| members.is_empty())
                    .unwrap_or(false)
            } else {
                true
            };
            if all_gone && group_empty {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }

        // 第二阶段:leader KILL;第三阶段:进程组 KILL,保留原有分层边界。
        if procfs::identity_status(target_identity) == procfs::IdentityStatus::Alive {
            if let Err(error) = signal_verified(target_identity, libc::SIGKILL) {
                diagnostics.push(error);
            }
        }
        if group {
            if let Err(error) = signal_group_verified(target_identity, libc::SIGKILL) {
                diagnostics.push(error);
            }
        }

        // 组外(含双 setsid 后代)继续 TERM → 1 秒 → KILL,逐个用 pidfd 锁定身份。
        let mut detached = Vec::new();
        for identity in processes.values().copied() {
            if group && identity.pgrp == target {
                continue;
            }
            if procfs::identity_status(identity) == procfs::IdentityStatus::Alive {
                detached.push(identity);
                if let Err(error) = signal_verified(identity, libc::SIGTERM) {
                    diagnostics.push(error);
                }
            }
        }
        if !detached.is_empty() {
            std::thread::sleep(Duration::from_secs(1));
        }
        for identity in detached {
            if procfs::identity_status(identity) == procfs::IdentityStatus::Alive {
                if let Err(error) = signal_verified(identity, libc::SIGKILL) {
                    diagnostics.push(error);
                }
            }
        }

        // 必须实际确认收集到的每个进程身份及整个目标组均消失;无法读/proc 也按失败处理。
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut unresolved = Vec::new();
            for identity in processes.values().copied() {
                match procfs::identity_status(identity) {
                    procfs::IdentityStatus::Gone => {}
                    procfs::IdentityStatus::Alive => {
                        unresolved.push(format!("PID {} still alive", identity.pid))
                    }
                    procfs::IdentityStatus::Reused => unresolved.push(format!(
                        "PID {} identity changed during stop verification",
                        identity.pid
                    )),
                    procfs::IdentityStatus::Unknown(error) => unresolved.push(error),
                }
            }
            if group {
                match procfs::processes_in_group(target, target_identity.session) {
                    Ok(members) if members.is_empty() => {}
                    Ok(members) => unresolved.extend(members.into_iter().map(|member| {
                        format!("process group {target} still contains PID {}", member.pid)
                    })),
                    Err(error) => unresolved.push(error),
                }
            }
            if unresolved.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                diagnostics.extend(unresolved);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !diagnostics.is_empty() {
            self.record_stop_failure(
                target,
                Some(target_identity),
                processes.values().copied().collect(),
                diagnostics,
            );
            return;
        }

        if let Err(error) = std::fs::remove_file(&self.cfg.pid_file) {
            if error.kind() != std::io::ErrorKind::NotFound {
                self.record_stop_failure(
                    target,
                    Some(target_identity),
                    processes.values().copied().collect(),
                    vec![format!("remove kernel PID file: {error}")],
                );
                return;
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.tracked = 0;
        state.tracked_identity = None;
        state.pending_stop.clear();
        state.cache_pid = 0;
        state.started_wall = None;
        state.stop_diagnostics.clear();
        state.last_stop_clean = true;
    }

    pub fn stop_for_uninstall(&self) -> Result<(), String> {
        let candidates = procfs::rediscover_all_in_venv(self.port(), 0, &self.cfg.venv_dir)?;
        for pid in candidates {
            let Some(identity) = procfs::process_identity(pid)? else {
                continue;
            };
            {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                state.tracked = pid;
                state.tracked_identity = Some(identity);
                state.pending_stop.clear();
                state.cache_pid = 0;
                state.cache_at = None;
            }
            self.stop_sync();
            if !self.last_stop_clean() {
                let diagnostics = self
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .stop_diagnostics
                    .join("; ");
                return Err(format!("停止孤儿内核 PID {pid} 失败: {diagnostics}"));
            }
        }
        let remaining = procfs::rediscover_all_in_venv(self.port(), 0, &self.cfg.venv_dir)?;
        if !remaining.is_empty() {
            return Err(format!("卸载前仍有内核进程存活: {remaining:?}"));
        }
        match std::fs::remove_file(&self.cfg.pid_file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("清理内核 PID 文件失败: {error}")),
        }
    }

    pub fn metrics_now(&self) -> (i32, Option<SystemTime>, Option<f64>, i64) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.sample(Instant::now()) {
            (st.tracked, st.started_wall, st.cache_cpu, st.cache_rss_kb)
        } else {
            (0, None, None, 0)
        }
    }

    /// 内核状态:受管 pid 失效时 /proc 重新发现并收养(结果缓存 2s)。
    pub fn status_now(&self) -> KernelStatus {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let cache_valid = st
            .cache_at
            .map(|t| now.duration_since(t) < Duration::from_secs(2))
            .unwrap_or(false)
            && st.cache_pid != 0
            && st.cache_pid == st.tracked
            && st.sample(now);
        let mut adopted = false;
        let pid;
        if !cache_valid {
            let discovered = procfs::rediscover_all_in_venv(self.port(), 0, &self.cfg.venv_dir)
                .and_then(|all| {
                    let best = all.iter().copied().max().unwrap_or(0);
                    if best == 0 {
                        return Ok((0, None));
                    }
                    match procfs::process_identity(best) {
                        Ok(Some(identity)) => Ok((best, Some(identity))),
                        Ok(None) => Err(format!(
                            "rediscovered kernel PID {best} exited before status identity check"
                        )),
                        Err(error) => Err(error),
                    }
                });
            let (best, identity) = match discovered {
                Ok(discovered) => discovered,
                Err(error) => {
                    if let Ok(mut log) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.cfg.log_file)
                    {
                        use std::io::Write as _;
                        let _ = writeln!(log, "[kernel] status rediscovery failed: {error}");
                    }
                    (0, None)
                }
            };
            if best != st.tracked
                || identity.map(|id| id.starttime) != st.tracked_identity.map(|id| id.starttime)
            {
                adopted = best > 0;
                st.tracked = best;
                st.tracked_identity = identity;
                st.started_wall = if best > 0 {
                    procfs::started_wall(best)
                } else {
                    None
                };
                if best > 0 {
                    let _ =
                        crate::install::atomic_write_0600(&self.cfg.pid_file, &format!("{best}\n"));
                }
            }
            pid = st.tracked;
            st.cache_pid = pid;
            st.cache_at = Some(now);
        } else {
            return KernelStatus {
                pid: st.cache_pid,
                running: true,
                adopted: false,
                started_at: st.started_wall,
                cpu_percent: st.cache_cpu,
                rss_kb: st.cache_rss_kb,
                port: self.port(),
            };
        }
        let running = pid > 0 && procfs::process_alive(pid);
        let started_wall = if running { st.started_wall } else { None };
        let (cpu, rss) = if running && st.sample(now) {
            (st.cache_cpu, st.cache_rss_kb)
        } else {
            (None, 0)
        };
        KernelStatus {
            pid,
            running,
            adopted,
            started_at: started_wall,
            cpu_percent: cpu,
            rss_kb: rss,
            port: self.port(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_status_cache_reuses_the_cpu_sample_without_another_sleep() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-status-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kernel = KernelManager::new(
            KernelConfig {
                venv_dir: dir.join("venv").to_string_lossy().into_owned(),
                pid_file: dir.join("kernel.pid").to_string_lossy().into_owned(),
                log_file: dir.join("kernel.log").to_string_lossy().into_owned(),
                share_dir: dir.to_string_lossy().into_owned(),
                etc_dir: dir.to_string_lossy().into_owned(),
                log_level: "info".into(),
            },
            crate::api::test_support::free_port(),
        );
        let pid = std::process::id() as i32;
        {
            let mut state = kernel.state.lock().unwrap();
            state.cache_pid = pid;
            state.cache_at = Some(Instant::now());
            state.tracked = pid;
            let id = procfs::process_identity(pid).unwrap().unwrap();
            state.tracked_identity = Some(id);
            state.last_ticks = Some((
                pid,
                id.starttime,
                Instant::now(),
                procfs::cpu_ticks(id).unwrap(),
            ));
            state.cache_cpu = Some(42.0);
            state.cache_rss_kb = 1024;
        }

        let started = Instant::now();
        let status = kernel.status_now();

        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(status.pid, pid);
        assert_eq!(status.cpu_percent, Some(42.0));
        assert_eq!(status.rss_kb, 1024);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn display_readers_share_one_probe_and_discard_the_invalidated_environment() {
        use crate::api::test_support::{install_python, TestDir};
        use std::{fs, sync::mpsc, time::Duration};
        let dir = TestDir::new("display-version-change");
        let state = dir.state(crate::api::test_support::free_port());
        assert!(!state.kernel.installed());
        assert!(state.kernel.state.lock().unwrap().version_cache_at.is_none());
        let python = install_python(&dir, &state, "");
        fs::write(&python, "#!/bin/sh\nd=$(dirname \"$0\"); v=$(cat \"$d/version\"); echo call >> \"$d/calls\"; while [ ! -f \"$d/release\" ]; do sleep .02; done; echo \"$v\"\n").unwrap();
        let bin = python.parent().unwrap();
        fs::write(bin.join("version"), "old").unwrap();
        let (tx, rx) = mpsc::channel();
        for _ in 0..10 {
            let tx = tx.clone();
            assert_eq!(
                state.kernel.display_version(move || tx.send(()).unwrap()),
                ""
            );
        }
        let start = Instant::now();
        while !bin.join("calls").exists() {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(10));
        }
        state.kernel.invalidate_version();
        fs::write(bin.join("version"), "new").unwrap();
        fs::write(bin.join("release"), "1").unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            state.kernel.display_version(move || tx.send(()).unwrap()),
            ""
        );
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            state
                .kernel
                .display_version(|| panic!("unexpected extra probe")),
            "new"
        );
        assert_eq!(
            fs::read_to_string(bin.join("calls")).unwrap(),
            "call\ncall\n"
        );
    }

    #[test]
    fn first_cpu_sample_and_mismatched_process_identity_are_not_reported_as_zero_load() {
        let pid = std::process::id() as i32;
        let id = procfs::process_identity(pid).unwrap().unwrap();
        let now = Instant::now();
        let mut s = State {
            tracked: pid,
            tracked_identity: Some(id),
            ..Default::default()
        };
        assert!(s.sample(now));
        assert!(s.cache_cpu.is_none());
        assert!(s.cache_rss_kb > 0);
        assert!(s.sample(now + Duration::from_secs(1)));
        assert!(s.cache_cpu.is_some_and(|v| (0.0..=100.0).contains(&v)));
        s.tracked_identity.as_mut().unwrap().starttime += 1;
        assert!(!s.sample(now + Duration::from_secs(2)));
    }

    #[test]
    fn kernel_version_is_cached_and_explicitly_invalidated() {
        let dir =
            std::env::temp_dir().join(format!("qwenpaw-version-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("venv/bin")).unwrap();
        let python = dir.join("venv/bin/python3");
        std::fs::write(
            &python,
            "#!/bin/bash\nprintf 'call\\n' >> \"$(dirname \"$0\")/calls\"\nsleep 0.1\nprintf '2.3.0\\n'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&python).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&python, permissions).unwrap();
        let kernel = KernelManager::new(
            KernelConfig {
                venv_dir: dir.join("venv").to_string_lossy().into_owned(),
                pid_file: dir.join("kernel.pid").to_string_lossy().into_owned(),
                log_file: dir.join("kernel.log").to_string_lossy().into_owned(),
                share_dir: dir.to_string_lossy().into_owned(),
                etc_dir: dir.to_string_lossy().into_owned(),
                log_level: "info".into(),
            },
            crate::api::test_support::free_port(),
        );
        assert_eq!(kernel.kernel_version(), "2.3.0");
        assert_eq!(kernel.kernel_version(), "2.3.0");
        assert_eq!(
            std::fs::read_to_string(dir.join("venv/bin/calls"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        kernel.invalidate_version();
        let kernel = std::sync::Arc::new(kernel);
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let kernel = kernel.clone();
                std::thread::spawn(move || kernel.kernel_version())
            })
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), "2.3.0");
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("venv/bin/calls"))
                .unwrap()
                .lines()
                .count(),
            2,
            "concurrent cache misses must share one metadata probe"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::{
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Barrier,
        },
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

    struct KernelTestCleanup {
        kernel: Arc<KernelManager>,
        dir: PathBuf,
        helper_pids: Arc<Mutex<Vec<i32>>>,
        helper_script: PathBuf,
        helper_token: String,
    }

    impl Drop for KernelTestCleanup {
        fn drop(&mut self) {
            self.kernel.stop_sync();
            let mut pids = self
                .helper_pids
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Ok(contents) = std::fs::read_to_string(self.dir.join("helper-pids")) {
                pids.extend(contents.lines().filter_map(|line| line.parse::<i32>().ok()));
            }
            pids.sort_unstable();
            pids.dedup();
            for pid in &pids {
                if is_test_helper(*pid, &self.helper_script, &self.helper_token) {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                        libc::kill(*pid, libc::SIGKILL);
                    }
                }
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline
                && pids
                    .iter()
                    .any(|pid| is_test_helper(*pid, &self.helper_script, &self.helper_token))
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn new_test_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        loop {
            let serial = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "qwenpaw-lifecycle-{}-{nonce}-{serial}",
                std::process::id()
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return dir,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create lifecycle test directory: {error}"),
            }
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn is_test_helper(pid: i32, helper_script: &Path, helper_token: &str) -> bool {
        if !procfs::process_alive(pid) || procfs::group_of(pid) != pid {
            return false;
        }
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            return false;
        };
        let script = helper_script.to_string_lossy();
        cmdline
            .split(|byte| *byte == 0)
            .any(|arg| arg == script.as_bytes() || arg == helper_token.as_bytes())
    }

    fn wait_for_test_helper(pid: i32, helper_script: &Path, helper_token: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if is_test_helper(pid, helper_script, helper_token) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_helper_pids(path: &Path, minimum: usize) -> Vec<i32> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path) {
                if contents.lines().count() >= minimum {
                    return contents
                        .lines()
                        .map(|line| line.parse::<i32>().expect("helper must record a valid PID"))
                        .collect();
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for helper PID log at {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // 工单10 回归:spawn/stop 必须互不交错。
    // 无 lifecycle 锁时,两个线程的 spawn 可以同时通过"是否已在运行"检查,
    // 结果是两份进程或一次假失败。
    #[test]
    fn runtime_log_level_reaches_spawn_and_rejects_invalid_values() {
        let dir =
            std::env::temp_dir().join(format!("qwenpaw-runtime-loglevel-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let python = dir.join("bin/python3");
        std::fs::write(&python, "#!/bin/bash\nprintf '%s\\n' \"$@\" > argv\nprintf '%s' \"$QWENPAW_LOG_LEVEL\" > level\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let kernel = KernelManager::new(
            KernelConfig {
                venv_dir: dir.to_string_lossy().into_owned(),
                pid_file: dir.join("kernel.pid").to_string_lossy().into_owned(),
                log_file: dir.join("kernel.log").to_string_lossy().into_owned(),
                share_dir: dir.to_string_lossy().into_owned(),
                etc_dir: dir.to_string_lossy().into_owned(),
                log_level: "info".into(),
            },
            crate::api::test_support::free_port(),
        );
        assert_eq!(kernel.log_level(), "info");
        kernel.set_log_level("debug").unwrap();
        assert!(kernel.set_log_level("verbose").is_err());
        assert_eq!(kernel.log_level(), "debug");
        kernel.spawn().unwrap();
        for _ in 0..100 {
            if dir.join("level").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(dir.join("level")).unwrap(), "debug");
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        assert!(argv.contains("--host\n127.0.0.1\n"));
        assert!(argv.ends_with("--log-level\ndebug\n"));
        kernel.stop_sync();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lifecycle_lock_serializes_spawn_and_stop() {
        const SPAWNERS: usize = 12;
        let dir = new_test_dir();
        std::fs::create_dir_all(dir.join("venv/bin")).unwrap();
        let helper_script = dir.join("venv/bin/python3");
        let helper_token = format!(
            "qwenpaw-lifecycle-helper-{}",
            dir.file_name().unwrap().to_string_lossy()
        );
        let helper_pid_log = dir.join("helper-pids");
        let helper_body = format!(
            "#!/bin/bash\nprintf '%s\\n' \"$$\" >> {}\nexec -a {} /bin/sleep 120\n",
            shell_quote(&helper_pid_log.to_string_lossy()),
            shell_quote(&helper_token),
        );
        std::fs::write(&helper_script, helper_body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper_script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let km = Arc::new(KernelManager::new(
            KernelConfig {
                venv_dir: dir.join("venv").to_string_lossy().into_owned(),
                pid_file: dir.join("kernel.pid").to_string_lossy().into_owned(),
                log_file: dir.join("kernel.log").to_string_lossy().into_owned(),
                share_dir: dir.to_string_lossy().into_owned(),
                etc_dir: dir.to_string_lossy().into_owned(),
                log_level: "info".into(),
            },
            crate::api::test_support::free_port(),
        ));
        let spawned_pids = Arc::new(Mutex::new(Vec::new()));
        let _cleanup = KernelTestCleanup {
            kernel: Arc::clone(&km),
            dir: dir.clone(),
            helper_pids: Arc::clone(&spawned_pids),
            helper_script: helper_script.clone(),
            helper_token: helper_token.clone(),
        };

        let start = Arc::new(Barrier::new(SPAWNERS + 1));
        let mut spawn_threads = Vec::with_capacity(SPAWNERS);
        for _ in 0..SPAWNERS {
            let kernel = Arc::clone(&km);
            let gate = Arc::clone(&start);
            let pids = Arc::clone(&spawned_pids);
            spawn_threads.push(std::thread::spawn(move || {
                gate.wait();
                let result = kernel.spawn();
                if let Ok(pid) = result {
                    pids.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
                    Ok(pid)
                } else {
                    result
                }
            }));
        }
        start.wait();
        let first_results: Vec<_> = spawn_threads
            .into_iter()
            .map(|thread| thread.join())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|result| result.expect("spawn worker panicked"))
            .collect();
        let first_successes: Vec<i32> = first_results
            .iter()
            .filter_map(|result| result.as_ref().ok().copied())
            .collect();
        assert_eq!(
            first_successes.len(),
            1,
            "a concurrent spawn wave must create exactly one helper: {first_results:?}"
        );

        let first_helper_pids = wait_for_helper_pids(&helper_pid_log, 1);
        assert_eq!(first_helper_pids, first_successes);
        let pid_file = dir.join("kernel.pid");
        let pid_content = std::fs::read_to_string(&pid_file).expect("spawn must write a PID file");
        assert!(
            pid_content.ends_with('\n'),
            "PID file must end with a newline: {pid_content:?}"
        );
        let pid_file_pid = pid_content
            .trim()
            .parse::<i32>()
            .expect("PID file must contain an integer");
        assert_eq!(pid_file_pid, first_successes[0]);
        let helper_ready = wait_for_test_helper(pid_file_pid, &helper_script, &helper_token);
        let helper_cmdline = std::fs::read(format!("/proc/{pid_file_pid}/cmdline"))
            .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
            .unwrap_or_else(|error| format!("<read failed: {error}>"));
        let helper_group = procfs::group_of(pid_file_pid);
        assert!(
            helper_ready,
            "helper identity did not become readable: pid={pid_file_pid}, group={helper_group}, cmdline={helper_cmdline:?}"
        );
        assert_eq!(procfs::group_of(pid_file_pid), pid_file_pid);
        assert_eq!(
            km.state.lock().unwrap_or_else(|e| e.into_inner()).tracked,
            pid_file_pid
        );

        let race = Arc::new(Barrier::new(SPAWNERS + 2));
        let mut race_threads = Vec::with_capacity(SPAWNERS);
        for _ in 0..SPAWNERS {
            let kernel = Arc::clone(&km);
            let gate = Arc::clone(&race);
            let pids = Arc::clone(&spawned_pids);
            race_threads.push(std::thread::spawn(move || {
                gate.wait();
                let result = kernel.spawn();
                if let Ok(pid) = result {
                    pids.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
                    Ok(pid)
                } else {
                    result
                }
            }));
        }
        let stopper = {
            let kernel = Arc::clone(&km);
            let gate = Arc::clone(&race);
            std::thread::spawn(move || {
                gate.wait();
                kernel.stop_sync();
            })
        };
        race.wait();
        let race_results: Vec<_> = race_threads
            .into_iter()
            .map(|thread| thread.join())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|result| result.expect("spawn/stop race worker panicked"))
            .collect();
        stopper.join().expect("stop worker panicked");
        let race_successes: Vec<i32> = race_results
            .iter()
            .filter_map(|result| result.as_ref().ok().copied())
            .collect();
        assert!(
            race_successes.len() <= 1,
            "one concurrent stop/spawn wave must not admit multiple spawns: {race_results:?}"
        );

        let all_spawned = spawned_pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let all_helper_pids = wait_for_helper_pids(&helper_pid_log, all_spawned.len());
        assert_eq!(all_helper_pids.len(), all_spawned.len());
        let mut sorted_helper_pids = all_helper_pids.clone();
        let mut sorted_spawned_pids = all_spawned.clone();
        sorted_helper_pids.sort_unstable();
        sorted_spawned_pids.sort_unstable();
        assert_eq!(sorted_helper_pids, sorted_spawned_pids);
        let consistency_deadline = Instant::now() + Duration::from_secs(2);
        let live_helpers: Vec<i32> = loop {
            let live: Vec<i32> = all_helper_pids
                .iter()
                .copied()
                .filter(|pid| is_test_helper(*pid, &helper_script, &helper_token))
                .collect();
            let tracked = km.state.lock().unwrap_or_else(|e| e.into_inner()).tracked;
            let consistent = match live.as_slice() {
                [] => tracked == 0,
                [pid] => tracked == *pid,
                _ => true,
            };
            if consistent || Instant::now() >= consistency_deadline {
                break live;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            live_helpers.len() <= 1,
            "at most one helper process may be alive after concurrent lifecycle operations: {live_helpers:?}"
        );
        let tracked = km.state.lock().unwrap_or_else(|e| e.into_inner()).tracked;
        if let Some(pid) = live_helpers.first() {
            assert_eq!(
                tracked, *pid,
                "the sole live helper must be the managed PID"
            );
            let content =
                std::fs::read_to_string(&pid_file).expect("live helper must have a PID file");
            assert_eq!(content.trim().parse::<i32>().unwrap(), *pid);
        } else {
            let diagnostics = km
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .stop_diagnostics
                .clone();
            assert_eq!(
                tracked, 0,
                "stopping the only helper must clear tracked state; diagnostics={diagnostics:?}; helpers={all_helper_pids:?}"
            );
            assert!(
                !pid_file.exists(),
                "a stopped helper must not leave a PID file"
            );
        }

        km.stop_sync();
        assert_eq!(
            km.state.lock().unwrap_or_else(|e| e.into_inner()).tracked,
            0
        );
        assert!(km.last_stop_clean(), "a fully verified stop must be clean");
        assert!(!pid_file.exists(), "final stop must remove the PID file");
        assert!(
            all_helper_pids
                .iter()
                .all(|pid| !is_test_helper(*pid, &helper_script, &helper_token)),
            "final stop must terminate every helper started by the test: {all_helper_pids:?}"
        );
    }

    fn install_discoverable_python(
        dir: &crate::api::test_support::TestDir,
        state: &crate::api::AppState,
    ) -> PathBuf {
        let bin = std::path::Path::new(&state.home_dir).join("venv/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let python = bin.join("python3");
        std::fs::copy("/usr/bin/python3", &python).unwrap();
        let package = dir.0.join("qwenpaw");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(
            package.join("__main__.py"),
            r#"import os, pathlib, time
root = pathlib.Path(os.environ['QWENPAW_TEST_ROOT'])
(root / ('ready-' + str(os.getpid()))).write_text('ready')
time.sleep(3600)
"#,
        )
        .unwrap();
        python
    }

    fn spawn_discoverable_kernel(
        dir: &crate::api::test_support::TestDir,
        python: &Path,
        port: u16,
    ) -> crate::api::test_support::ChildGuard {
        use std::os::unix::process::CommandExt;

        let child = std::process::Command::new(python)
            .args(["-m", "qwenpaw", "app", "--port", &port.to_string()])
            .env("PYTHONPATH", &dir.0)
            .env("QWENPAW_TEST_ROOT", &dir.0)
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(3);
        while !dir.0.join(format!("ready-{pid}")).exists() {
            assert!(
                Instant::now() < deadline,
                "kernel fixture PID {pid} did not become ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        crate::api::test_support::ChildGuard(child)
    }

    #[test]
    fn ordinary_stop_terminates_every_instance_from_the_configured_venv() {
        let dir = crate::api::test_support::TestDir::new("stop-all-venv-instances");
        let port = crate::api::test_support::free_port();
        let state = dir.state(port);
        let python = install_discoverable_python(&dir, &state);
        let first = spawn_discoverable_kernel(&dir, &python, port);
        let second = spawn_discoverable_kernel(&dir, &python, port);
        let first_identity = procfs::process_identity(first.0.id() as i32)
            .unwrap()
            .expect("first fixture identity");
        let second_identity = procfs::process_identity(second.0.id() as i32)
            .unwrap()
            .expect("second fixture identity");
        let mut discovered = procfs::rediscover_all_in_venv(
            port,
            0,
            &std::path::Path::new(&state.home_dir)
                .join("venv")
                .to_string_lossy(),
        )
        .unwrap();
        discovered.sort_unstable();
        let mut expected = vec![first.0.id() as i32, second.0.id() as i32];
        expected.sort_unstable();
        assert_eq!(discovered, expected);

        state.kernel.stop_sync();

        assert!(state.kernel.last_stop_clean());
        assert_eq!(
            procfs::identity_status(first_identity),
            procfs::IdentityStatus::Gone
        );
        assert_eq!(
            procfs::identity_status(second_identity),
            procfs::IdentityStatus::Gone
        );
    }

    #[test]
    fn same_port_process_outside_venv_is_neither_adopted_nor_stopped() {
        let dir = crate::api::test_support::TestDir::new("ignore-external-kernel");
        let port = crate::api::test_support::free_port();
        let state = dir.state(port);
        let python = install_discoverable_python(&dir, &state);
        let external = dir.0.join("external-python3");
        std::fs::copy(python, &external).unwrap();
        let foreign = spawn_discoverable_kernel(&dir, &external, port);

        let status = state.kernel.status_now();
        state.kernel.stop_sync();

        assert_eq!(status.pid, 0);
        assert!(!status.adopted);
        assert!(procfs::process_alive(foreign.0.id() as i32));
    }

    #[test]
    fn unverifiable_target_retains_pid_and_marks_stop_unclean() {
        let dir = new_test_dir();
        let pid_file = dir.join("kernel.pid");
        let pid = std::process::id() as i32;
        let mut stale_identity = procfs::process_identity(pid).unwrap().unwrap();
        stale_identity.starttime = stale_identity.starttime.saturating_add(1);
        std::fs::write(&pid_file, format!("{pid}\n")).unwrap();
        let kernel = KernelManager::new(
            KernelConfig {
                venv_dir: dir.join("venv").to_string_lossy().into_owned(),
                pid_file: pid_file.to_string_lossy().into_owned(),
                log_file: dir.join("kernel.log").to_string_lossy().into_owned(),
                share_dir: dir.to_string_lossy().into_owned(),
                etc_dir: dir.to_string_lossy().into_owned(),
                log_level: "info".into(),
            },
            crate::api::test_support::free_port(),
        );
        {
            let mut state = kernel
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.tracked = pid;
            state.tracked_identity = Some(stale_identity);
        }

        kernel.stop_sync();

        assert!(!kernel.last_stop_clean());
        assert_eq!(
            kernel
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .tracked,
            pid
        );
        assert_eq!(
            std::fs::read_to_string(&pid_file).unwrap(),
            format!("{pid}\n")
        );
        assert!(kernel
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("identity changed")));
        assert!(std::fs::read_to_string(dir.join("kernel.log"))
            .unwrap()
            .contains("stop verification failed"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
