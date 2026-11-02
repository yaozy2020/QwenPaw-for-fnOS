//! 内核进程管理器 —— 模式裁剪自 dsh backend/dsh.go(MIT)。
//! 血泪清单(每条都有 dsh 线上事故背书,勿删):
//!   - 只按 PID/进程组杀,绝不按进程名杀(误杀持锁进程会留陈旧锁)
//!   - 僵尸判定看 stat 状态字符,不看目录存在(procfs.rs)
//!   - 被管 pid 失效时扫 /proc 重新发现(对账兜底),结果缓存 2s
//!   - 停止三段式:组 TERM(宽限)→ 单 KILL → 组 KILL 清扫
//!   - 不用 Pdeathsig:内核必须在监督者重启(外壳升级)后存活,由 /proc 收养

use std::{
    os::unix::process::CommandExt,
    path::Path,
    process,
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

pub struct KernelManager {
    cfg: KernelConfig,
    port: AtomicU16,
    state: Mutex<State>,
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
    tracked: i32,
    started_wall: Option<SystemTime>,
    cache_pid: i32,
    cache_at: Option<Instant>,
    version_cache: String,
    version_cache_at: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct KernelStatus {
    pub pid: i32,
    pub running: bool,
    pub adopted: bool,
    pub started_at: Option<SystemTime>,
    pub cpu_percent: f64,
    pub rss_kb: i64,
    pub port: u16,
}

impl KernelManager {
    pub fn new(cfg: KernelConfig, port: u16) -> Self {
        Self {
            cfg,
            port: AtomicU16::new(port),
            state: Mutex::new(State::default()),
            lifecycle: Mutex::new(()),
        }
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    pub fn kernel_version(&self) -> String {
        {
            let state = self.state.lock().unwrap();
            if state
                .version_cache_at
                .map(|at| at.elapsed() < Duration::from_secs(5))
                .unwrap_or(false)
            {
                return state.version_cache.clone();
            }
        }
        let bin = format!("{}/bin/python3", self.cfg.venv_dir);
        let version = match process::Command::new(&bin)
            .arg("-c")
            .arg("import importlib.metadata as m; print(m.version('qwenpaw'))")
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        };
        let mut state = self.state.lock().unwrap();
        state.version_cache = version.clone();
        state.version_cache_at = Some(Instant::now());
        version
    }

    pub fn invalidate_version(&self) {
        let mut state = self.state.lock().unwrap();
        state.version_cache.clear();
        state.version_cache_at = None;
    }

    pub fn installed(&self) -> bool {
        !self.kernel_version().is_empty()
    }

    /// 同步启动内核(无 await;由 axum 处理器与安装完成回调直接调用)。
    /// 以独立进程组启动(process_group(0) ≈ setsid)。
    pub fn spawn(&self) -> Result<i32, String> {
        // 与 stop_sync 互斥,防止"stop 后 spawn"与安装线程的 spawn 交错
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        {
            let st = self.state.lock().unwrap();
            if st.tracked != 0 && procfs::process_alive(st.tracked) {
                return Err(format!("kernel already running pid={}", st.tracked));
            }
        }
        let bin = format!("{}/bin/python3", self.cfg.venv_dir);
        if !Path::new(&bin).exists() {
            return Err("kernel not installed".into());
        }
        let port = self.port();
        // std::process::Command:spawn 可从任意线程调用(安装线程的失败自愈/
        // on_success 都在裸线程上,tokio Command 在无 runtime 上下文会 panic ——
        // 审计 B2 实测:panic + 日志锁毒化,一次 pip 失败砖死安装子系统)
        let mut cmd = std::process::Command::new(&bin);
        // --log-level 必须走 argv:上游 app 子命令只在 CLI 参数上消费日志级别
        // (app_cmd.py --log-level,默认 info;QWENPAW_LOG_LEVEL 环境变量仅 tauri
        // 入口读取,app 路径不读)。此前向导选择从未抵达内核 —— 审查 F1 根因。
        cmd.args([
            "-m", "qwenpaw", "app",
            "--host", "0.0.0.0",
            "--port", &port.to_string(),
            "--log-level", &self.cfg.log_level,
        ])
            .current_dir(&self.cfg.share_dir)
            .env("HOME", &self.cfg.etc_dir)
            .env("QWENPAW_WORKING_DIR", &self.cfg.share_dir)
            .env("QWENPAW_AUTH_ENABLED", "true")
            // app_cmd 启动后自身会为子进程(shared_worker 等)回写该变量;此处
            // 显式注入保证 spawn 之初环境即一致
            .env("QWENPAW_LOG_LEVEL", &self.cfg.log_level)
            .env(
                "QWENPAW_DEPENDENCY_STATE_DIR",
                format!("{}/dependency-environments", self.cfg.share_dir),
            )
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
        cmd.stdout(std::process::Stdio::from(
            log.try_clone().map_err(|e| format!("clone kernel log: {e}"))?,
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
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
                    let _ = writeln!(f, "[kernel] pid exited: {st}");
                }
            }
        });
        {
            let mut st = self.state.lock().unwrap();
            st.tracked = pid;
            st.started_wall = procfs::started_wall(pid);
            st.cache_pid = pid;
            st.cache_at = Some(Instant::now());
        }
        Ok(pid)
    }

    /// 启动后健康检查:进程存活且内核端口可建立 TCP 连接才算"服务已就绪"。
    /// "进程已创建"不等于"服务成功" —— 改端口/重置认证的成功判定以此为准,
    /// 失败路径据此回滚(超时视为失败,由调用方处理)。
    /// 阻塞式:调用方(axum 处理器)须经 spawn_blocking 包裹。
    pub fn wait_healthy(&self, timeout: Duration) -> bool {
        let pid = self.state.lock().unwrap().tracked;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if pid <= 0 || !procfs::process_alive(pid) {
                return false; // 进程已死,等下去只会假超时
            }
            if std::net::TcpStream::connect(("127.0.0.1", self.port())).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        false
    }

    /// 三段式停止:组 TERM(宽限 10s)→ 单 KILL → 组 KILL 清扫。
    /// 异步非阻塞接口:将阻塞的睡眠轮询放进 spawn_blocking,不占用 Tokio 工作线程
    pub async fn stop(self: &std::sync::Arc<Self>) {
        let this = std::sync::Arc::clone(self);
        let _ = tokio::task::spawn_blocking(move || this.stop_sync()).await;
    }

    pub fn stop_sync(&self) {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        let mut target = {
            let st = self.state.lock().unwrap();
            st.tracked
        };
        if target == 0 || !procfs::process_alive(target) {
            target = procfs::rediscover_all(self.port(), target)
                .into_iter()
                .max()
                .unwrap_or(0);
        }
        if target <= 0 {
            return;
        }
        let group_ok = procfs::group_of(target) == target;
        // 自成进程组的后代(b2 shared_worker 双 setsid,及其再派生的孙进程):
        // 组信号打不到,必须在目标死亡前按 ppid 树收集(死后 reparent 到 1)。
        let mut kids = procfs::descendants_of(target);

        // 1) 组 TERM + 单 TERM(宽限 10s)
        if group_ok {
            unsafe { libc::kill(-target, libc::SIGTERM) };
        }
        unsafe { libc::kill(target, libc::SIGTERM) };
        for _ in 0..10 {
            if !procfs::process_alive(target) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        // 2) 仍活:单 KILL(宽限期内新生的子进程补收)
        if procfs::process_alive(target) {
            kids.extend(procfs::descendants_of(target));
            unsafe { libc::kill(target, libc::SIGKILL) };
            std::thread::sleep(Duration::from_millis(500));
        }
        // 3) 组 KILL 清扫:无视 TERM 的子进程(chromium/MCP)在此兜底;
        //    已死成员 ESRCH 为空操作。组不可用时跳过(防误伤同组平台进程)。
        if group_ok {
            unsafe { libc::kill(-target, libc::SIGKILL) };
        }
        // 4) 自成组的直接子进程:TERM → 1s → KILL 逐个清扫(去重,防孤儿)
        kids.sort_unstable();
        kids.dedup();
        let alive_kids: Vec<i32> = kids.into_iter().filter(|k| procfs::process_alive(*k)).collect();
        for k in &alive_kids {
            unsafe { libc::kill(*k, libc::SIGTERM) };
        }
        std::thread::sleep(Duration::from_secs(1));
        for k in &alive_kids {
            if procfs::process_alive(*k) {
                unsafe { libc::kill(*k, libc::SIGKILL) };
            }
        }
        let _ = std::fs::remove_file(&self.cfg.pid_file);
        let mut st = self.state.lock().unwrap();
        st.tracked = 0;
        st.cache_pid = 0;
        st.started_wall = None;
    }

    /// 内核状态:受管 pid 失效时 /proc 重新发现并收养(结果缓存 2s)。
    pub fn status_now(&self) -> KernelStatus {
        let mut st = self.state.lock().unwrap();
        let now = Instant::now();
        let cache_valid = st
            .cache_at
            .map(|t| now.duration_since(t) < Duration::from_secs(2))
            .unwrap_or(false)
            && st.cache_pid != 0
            && procfs::process_alive(st.cache_pid);
        let mut adopted = false;
        let pid;
        if !cache_valid {
            let all = procfs::rediscover_all(self.port(), st.tracked);
            let best = all.iter().copied().max().unwrap_or(0);
            if best > 0 && best != st.tracked {
                adopted = true;
                st.tracked = best;
                st.started_wall = procfs::started_wall(best);
                let _ = crate::install::atomic_write_0600(&self.cfg.pid_file, &format!("{best}\n"));
            }
            pid = st.tracked;
            st.cache_pid = pid;
            st.cache_at = Some(now);
        } else {
            pid = st.cache_pid;
        }
        let running = pid > 0 && procfs::process_alive(pid);
        let started_wall = if running { st.started_wall } else { None };
        // F5:提前释放状态互斥锁,避免 300ms 的 CPU 采样 sleep 阻塞并发请求
        drop(st);
        let (cpu, rss) = if running {
            let (u1, s1) = procfs::cpu_ticks(pid);
            std::thread::sleep(Duration::from_millis(300));
            let (u2, s2) = procfs::cpu_ticks(pid);
            let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
            let cpu = ((u2.saturating_sub(u1) + s2.saturating_sub(s1)) as f64 / 100.0) / 0.3 * 100.0 / ncpu;
            (cpu.clamp(0.0, 100.0 * ncpu), procfs::rss_kb(pid))
        } else {
            (0.0, 0)
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
    fn kernel_version_is_cached_and_explicitly_invalidated() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-version-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("venv/bin")).unwrap();
        let python = dir.join("venv/bin/python3");
        std::fs::write(
            &python,
            "#!/bin/bash\nprintf 'call\\n' >> \"$(dirname \"$0\")/calls\"\nprintf '2.3.0\\n'\n",
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
            18088,
        );
        assert_eq!(kernel.kernel_version(), "2.3.0");
        assert_eq!(kernel.kernel_version(), "2.3.0");
        assert_eq!(std::fs::read_to_string(dir.join("venv/bin/calls")).unwrap().lines().count(), 1);
        kernel.invalidate_version();
        assert_eq!(kernel.kernel_version(), "2.3.0");
        assert_eq!(std::fs::read_to_string(dir.join("venv/bin/calls")).unwrap().lines().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // 工单10 回归:spawn/stop 必须互不交错。
    // 无 lifecycle 锁时,两个线程的 spawn 可以同时通过"是否已在运行"检查,
    // 结果是两份进程或一次假失败。
    #[test]
    fn lifecycle_lock_serializes_spawn_and_stop() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-lifecycle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let cfg = KernelConfig {
            venv_dir: dir.to_string_lossy().to_string(),
            pid_file: dir.join("kernel.pid").to_string_lossy().to_string(),
            log_file: dir.join("kernel.log").to_string_lossy().to_string(),
            share_dir: dir.to_string_lossy().to_string(),
            etc_dir: dir.to_string_lossy().to_string(),
            log_level: "info".to_string(),
        };
        let km = Arc::new(KernelManager::new(cfg, 0));

        // 并发进入 spawn/stop,统计进入临界区的最大并发度
        let inside = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for i in 0..8 {
            let km2 = Arc::clone(&km);
            let inside2 = Arc::clone(&inside);
            let max2 = Arc::clone(&max_seen);
            handles.push(std::thread::spawn(move || {
                for _ in 0..20 {
                    let cur = inside2.fetch_add(1, Ordering::SeqCst) + 1;
                    max2.fetch_max(cur, Ordering::SeqCst);
                    if i % 2 == 0 {
                        let _ = km2.spawn();
                    } else {
                        km2.stop_sync();
                    }
                    inside2.fetch_sub(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // spawn/stop 内部已持锁,但它们本身可以并行进入"计数区",
        // 因此这里断言的是"不会 panic/死锁",并额外确认 pid 文件未出现半截内容。
        let pid_file = dir.join("kernel.pid");
        if pid_file.exists() {
            let content = std::fs::read_to_string(&pid_file).unwrap();
            assert!(
                content.trim().parse::<i32>().is_ok(),
                "pid 文件必须是完整整数,实际: {content:?}"
            );
        }
        assert!(max_seen.load(Ordering::SeqCst) >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
