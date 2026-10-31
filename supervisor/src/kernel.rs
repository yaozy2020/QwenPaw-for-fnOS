//! 内核进程管理器 —— 模式裁剪自 dsh backend/dsh.go(MIT)。
//! 血泪清单(每条都有 dsh 线上事故背书,勿删):
//!   - 只按 PID/进程组杀,绝不按进程名杀(误杀持锁进程会留陈旧锁)
//!   - 僵尸判定看 stat 状态字符,不看目录存在(procfs.rs)
//!   - 被管 pid 失效时扫 /proc 重新发现(对账兜底),结果缓存 2s
//!   - 停止三段式:组 TERM(宽限)→ 单 KILL → 组 KILL 清扫
//!   - 不用 Pdeathsig:内核必须在监督者重启(外壳升级)后存活,由 /proc 收养

use std::{
    os::unix::io::{FromRawFd, IntoRawFd},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use tokio::process::Command;

use crate::procfs;

#[derive(Clone, Debug)]
pub struct KernelConfig {
    pub venv_dir: String,
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
    pub port: u16,
    pub share_dir: String,
}

#[derive(Clone, Debug)]
pub struct KernelStatus {
    pub pid: i32,
    pub running: bool,
    pub adopted: bool,
    pub started_at: Option<std::time::SystemTime>,
    pub cpu_percent: f64,
    pub rss_kb: i64,
    pub port: u16,
}

pub struct KernelManager {
    cfg: KernelConfig,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    tracked: i32,
    started_wall: Option<std::time::SystemTime>,
    cache_pid: i32,
    cache_at: Option<Instant>,
}

impl KernelManager {
    pub fn new(cfg: KernelConfig) -> Self {
        Self { cfg, state: Mutex::new(State::default()) }
    }

    pub fn port(&self) -> u16 {
        self.cfg.port
    }

    pub fn kernel_version(&self) -> String {
        let bin = format!("{}/bin/python3", self.cfg.venv_dir);
        match std::process::Command::new(&bin)
            .arg("-c")
            .arg("import importlib.metadata as m; print(m.version('qwenpaw'))")
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        }
    }

    pub fn installed(&self) -> bool {
        !self.kernel_version().is_empty()
    }

    /// 以独立进程组启动内核(process_group(0) ≈ setsid)。
    pub fn spawn(&self) -> Result<i32, String> {
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
        let mut cmd = Command::new(&bin);
        cmd.args(["-m", "qwenpaw", "app", "--host", "0.0.0.0", "--port", &self.cfg.port.to_string()])
            .current_dir(&self.cfg.share_dir)
            .env(
                "QWENPAW_DEPENDENCY_STATE_DIR",
                format!("{}/dependency-environments", self.cfg.share_dir),
            )
            .process_group(0) // 独立进程组:停止时组信号可连带浏览器/MCP 子进程
            .kill_on_drop(false);
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.cfg.log_file)
            .map_err(|e| format!("open kernel log: {e}"))?;
        let log_err = log
            .try_clone()
            .map_err(|e| format!("clone kernel log: {e}"))?;
        use std::os::unix::io::IntoRawFd;
        cmd.stdout(unsafe { std::fs::File::from_raw_fd(log.into_raw_fd()) });
        cmd.stderr(unsafe { std::fs::File::from_raw_fd(log_err.into_raw_fd()) });
        let mut child = cmd.spawn().map_err(|e| format!("spawn kernel: {e}"))?;
        let pid = child.id().unwrap_or(0) as i32;
        let _ = std::fs::write(&self.cfg.pid_file, format!("{pid}\n"));
        // 后台进程:交由内核自身管理生命周期;退出后 tokio 孤儿回收器收尸
        std::thread::spawn(move || {
            let _ = child.wait();
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

    /// 三段式停止:组 TERM(宽限 10s)→ 单 KILL → 组 KILL 清扫。
    /// 组信号仅在内核为自身进程组长(setsid 启动)时启用,防误伤同组平台进程。
    pub async fn stop(&self) {
        let mut target = {
            let st = self.state.lock().unwrap();
            st.tracked
        };
        if target == 0 || !procfs::process_alive(target) {
            target = procfs::rediscover_all(self.cfg.port, target)
                .into_iter()
                .max()
                .unwrap_or(0);
        }
        if target <= 0 {
            return;
        }
        let group_ok = procfs::group_of(target) == target;

        // 1) 组 TERM + 单 TERM(宽限 10s)
        if group_ok {
            unsafe { libc::kill(-target, libc::SIGTERM) };
        }
        unsafe { libc::kill(target, libc::SIGTERM) };
        for _ in 0..10 {
            if !procfs::process_alive(target) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        // 2) 仍活:单 KILL
        if procfs::process_alive(target) {
            unsafe { libc::kill(target, libc::SIGKILL) };
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // 3) 组 KILL 清扫:无视 TERM 的子进程(chromium/MCP)在此兜底;
        //    已死成员 ESRCH 为空操作。组不可用时跳过(防误伤同组平台进程)。
        if group_ok {
            unsafe { libc::kill(-target, libc::SIGKILL) };
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
        let mut pid = st.tracked;
        if !cache_valid {
            let all = procfs::rediscover_all(self.cfg.port, st.tracked);
            let best = all.iter().copied().max().unwrap_or(0);
            if best > 0 && best != st.tracked {
                adopted = true;
                st.tracked = best;
                st.started_wall = procfs::started_wall(best);
                let _ = std::fs::write(&self.cfg.pid_file, format!("{best}\n"));
            }
            pid = st.tracked;
            st.cache_pid = pid;
            st.cache_at = Some(now);
        } else {
            pid = st.cache_pid;
        }
        let running = pid > 0 && procfs::process_alive(pid);
        let started_wall = if running { st.started_wall } else { None };
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
            port: self.cfg.port,
        }
    }
}
