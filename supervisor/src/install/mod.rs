mod channel;
mod io;
mod pip;
#[allow(dead_code)]
mod process;
mod transaction;

pub use channel::*;
pub(crate) use io::{atomic_write_0600, sweep_stale_tmp_files};
use pip::{PipSpawner, RealSpawner};
pub(crate) use process::run_with_timeout;
use process::{signal_process_group, version_matches, ExecutableIdentity, ProcessIdentity};
pub(crate) use process::{wait_for_version_cancellable, wait_for_version_deadline};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
pub use transaction::*;
use transaction::{check_crash_point, pending_transactions};

#[cfg(test)]
use channel::is_safe_exact_version;
#[cfg(test)]
use process::wait_for_version;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use transaction::{copy_tree, write_tx_record};

pub fn truncate_utf8(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_string();
    }
    let end = raw
        .char_indices()
        .map(|(idx, _)| idx)
        .take_while(|&idx| idx <= max_bytes)
        .last()
        .unwrap_or(0);
    format!("{}…", &raw[..end])
}

#[derive(Clone, Debug)]
pub struct InstallSnapshot {
    pub phase: Phase,
    pub fetched: u64,
    pub message: String,
    pub error: String,
    pub seq: u64,
    pub running: bool,
    pub recovery_required: bool,
    pub recovery_error: Option<String>,
}

#[derive(Clone)]
struct Inner {
    phase: Phase,
    fetched: u64,
    message: String,
    error: String,
    seq: u64,
    running: bool,
    pid: u32,
    pgid: i32,
    session: i32,
    starttime: u64,
    executable: Option<ExecutableIdentity>,
    cancelled: bool,
    shutdown: bool,
    worker_active: bool,
    recovery_error: Option<String>,
}

type InstallInitializer = dyn Fn(&InstallState, &Path) -> Result<(), String> + Send + Sync;
const SHUTDOWN_RECOVERY_LATCH: &str = "监督者正在关闭";

#[derive(Clone)]
pub struct InstallState {
    inner: Arc<Mutex<Inner>>,
    settled: Arc<std::sync::Condvar>,
    on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
    on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    on_stop: Arc<dyn Fn() + Send + Sync>,
    on_restore: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    on_initialize: Arc<InstallInitializer>,
    spawner: Arc<dyn PipSpawner>,
    base_python: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl InstallState {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    ) -> Self {
        Self::with_spawner(Arc::new(RealSpawner), on_change, on_success)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_stop(
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
        on_stop: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::with_lifecycle(on_change, on_success, on_stop, Arc::new(|| Ok(0)))
    }

    pub fn with_lifecycle(
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_start: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
        on_stop: Arc<dyn Fn() + Send + Sync>,
        on_restore: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    ) -> Self {
        let mut me = Self::with_spawner(Arc::new(RealSpawner), on_change, on_start);
        me.on_stop = on_stop;
        me.on_restore = on_restore;
        me
    }

    fn with_spawner(
        spawner: Arc<dyn PipSpawner>,
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                phase: Phase::Idle,
                fetched: 0,
                message: String::new(),
                error: String::new(),
                seq: 0,
                running: false,
                pid: 0,
                pgid: 0,
                session: 0,
                starttime: 0,
                executable: None,
                cancelled: false,
                shutdown: false,
                worker_active: false,
                recovery_error: None,
            })),
            settled: Arc::new(std::sync::Condvar::new()),
            on_change,
            on_success,
            on_stop: Arc::new(|| {}),
            on_restore: Arc::new(|| Ok(0)),
            on_initialize: Arc::new(|_, _| Ok(())),
            spawner,
            base_python: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_base_python(&self, python: std::path::PathBuf) {
        *self
            .base_python
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(python);
    }

    fn snapshot_inner(g: &Inner) -> InstallSnapshot {
        InstallSnapshot {
            phase: g.phase.clone(),
            fetched: g.fetched,
            message: g.message.clone(),
            error: g.error.clone(),
            seq: g.seq,
            running: g.running,
            recovery_required: g.recovery_error.is_some(),
            recovery_error: g.recovery_error.clone(),
        }
    }

    pub fn snapshot(&self) -> InstallSnapshot {
        Self::snapshot_inner(&self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn is_busy(&self) -> bool {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.running || g.worker_active
    }

    fn emit(&self, snap: InstallSnapshot) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.on_change)(snap)));
    }

    fn set(&self, f: impl FnOnce(&mut Inner)) {
        let snap = {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut g);
            Self::snapshot_inner(&g)
        };
        self.emit(snap);
    }

    pub fn block_recovery(&self, reason: String) {
        self.set(|g| g.recovery_error = Some(reason));
    }

    pub fn recovery_error(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recovery_error
            .clone()
    }

    pub fn cancel(&self) {
        let (pid, identity) = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if !state.running && state.pid == 0 {
                return;
            }
            state.cancelled = true;
            let identity = (state.pid != 0).then_some(ProcessIdentity {
                pid: state.pid,
                starttime: state.starttime,
                pgid: state.pgid,
                session: state.session,
                executable: state.executable,
            });
            (state.pid, identity)
        };
        if pid != 0 {
            match identity {
                Some(identity) => {
                    if let Err(error) = signal_process_group(identity, libc::SIGTERM) {
                        self.block_recovery(format!("无法安全取消安装进程组: {error}"));
                    }
                }
                None => self.block_recovery("安装进程身份不完整，拒绝发送进程组信号".into()),
            }
        }
    }

    pub fn shutdown(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.shutdown = true;
            if state.recovery_error.as_deref() == Some(SHUTDOWN_RECOVERY_LATCH) {
                state.recovery_error = None;
            }
        }
        self.cancel();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        while g.worker_active || g.pid != 0 || g.pgid != 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, _) = self
                .settled
                .wait_timeout(g, remaining)
                .unwrap_or_else(|e| e.into_inner());
            g = next;
        }
        !g.running && g.pid == 0 && g.pgid == 0
    }

    fn cancelled(&self) -> bool {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.cancelled || g.shutdown
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if self.cancelled() {
            Err("安装已被停止".into())
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    fn finish_cancelled(&self) {
        self.set(|g| {
            g.running = false;
            g.pid = 0;
            g.pgid = 0;
            g.session = 0;
            g.starttime = 0;
            g.executable = None;
            g.phase = Phase::Cancelled;
            g.error = "安装已被停止".into();
        });
    }

    #[cfg(test)]
    fn abort_if_cancelled(&self) -> bool {
        if self.cancelled() {
            self.finish_cancelled();
            true
        } else {
            false
        }
    }

    fn stop_kernel(&self) -> Result<(), String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.on_stop)()))
            .map_err(|_| "停止内核回调异常".into())
    }

    fn finish_install(
        &self,
        mut transaction: Option<&mut VenvTransaction>,
        active: &Path,
        outcome: Result<String, String>,
    ) {
        if let Ok(actual) = &outcome {
            let snap = {
                let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if !g.cancelled && !g.shutdown {
                    g.running = false;
                    g.phase = Phase::Done;
                    g.message = format!("安装完成 v{actual}，服务已就绪");
                    Some(Self::snapshot_inner(&g))
                } else {
                    None
                }
            };
            if let Some(snap) = snap {
                self.emit(snap);
                return;
            }
        }
        let mut error = outcome.err().unwrap_or_default();
        let mut restored_service = false;
        if let Err(e) = self.stop_kernel() {
            self.block_recovery(e.clone());
            error = format!("{error}；{e}");
        } else {
            if self.recovery_error().is_some() {
                error = format!("{error}；进程身份无法确认，已保留现场并阻断自动恢复");
            } else {
                let rollback = match transaction.as_mut() {
                    Some(tx)
                        if !tx.committed_or_rolled_back
                            && tx.record.phase == TxPhase::Committed =>
                    {
                        Err(std::io::Error::other("提交已持久化但清理未完成,须恢复检查"))
                    }
                    Some(tx) if !tx.committed_or_rolled_back => tx.rollback(),
                    _ => Ok(()),
                };
                if let Err(e) = rollback {
                    error = format!("{error}；恢复原内核环境失败: {e}");
                    self.block_recovery(error.clone());
                }
                match pending_transactions(active) {
                    Ok(pending) if !pending.is_empty() => {
                        self.block_recovery(format!("未决事务: {}", pending.join("; ")))
                    }
                    Err(e) => self.block_recovery(format!("无法核查事务残留: {e}")),
                    _ => {}
                }
                let had_active = transaction
                    .as_ref()
                    .map(|tx| tx.record.had_active)
                    .unwrap_or_else(|| active.join("bin/python3").exists());
                if !self.cancelled() && self.recovery_error().is_none() && had_active {
                    restored_service = true;
                    let restored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (self.on_restore)()
                    }))
                    .unwrap_or_else(|_| Err("恢复内核回调异常".into()));
                    if let Err(e) = restored {
                        error = format!("{error}；原内核服务恢复失败: {e}");
                    }
                }
            }
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if (g.cancelled || g.shutdown) && restored_service {
            drop(g);
            if let Err(e) = self.stop_kernel() {
                self.block_recovery(e);
            }
            g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        }
        g.running = false;
        if g.cancelled || g.shutdown {
            g.phase = Phase::Cancelled;
            g.error = if g.recovery_error.is_some() {
                format!("安装已被停止；{error}")
            } else {
                "安装已被停止".into()
            };
        } else {
            g.phase = Phase::Failed;
            g.error = error;
        }
        let snap = Self::snapshot_inner(&g);
        drop(g);
        self.emit(snap);
    }

    /// 运行 pip 安装(独立进程组 + 专用阻塞线程;终态唯一写入者是安装线程)。
    /// 返回是否成功启动(重复触发返回 false,对齐 dsh 的 busy 语义)。
    ///
    /// `pin_path` 是版本钉;成功(且元数据校验通过)后原子写入实际安装版本。
    pub fn with_initializer(mut self, initializer: Arc<InstallInitializer>) -> Self {
        self.on_initialize = initializer;
        self
    }

    pub fn try_start(
        &self,
        venv_dir: String,
        kernel_port: u16,
        channel: InstallChannel,
        mirror: String,
        pip_log: String,
        pin_path: String,
    ) -> bool {
        let pid_file = match process::install_pid_file(Path::new(&pip_log)) {
            Ok(path) => path,
            Err(_) => return false,
        };
        let task_seq = {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if g.running
                || g.worker_active
                || g.shutdown
                || g.recovery_error.is_some()
                || g.pid != 0
                || g.pgid != 0
            {
                return false;
            }
            match pending_transactions(Path::new(&venv_dir)) {
                Ok(pending) if pending.is_empty() => {}
                Ok(pending) => {
                    g.recovery_error = Some(format!("未决事务: {}", pending.join("; ")));
                    return false;
                }
                Err(e) => {
                    g.recovery_error = Some(format!("无法扫描安装事务: {e}"));
                    return false;
                }
            }
            g.seq += 1;
            g.running = true;
            g.worker_active = true;
            g.phase = Phase::Downloading;
            g.fetched = 0;
            g.message = "准备独立安装环境".into();
            g.error = String::new();
            g.pid = 0;
            g.pgid = 0;
            g.session = 0;
            g.starttime = 0;
            g.executable = None;
            g.cancelled = false;
            g.seq
        };
        self.emit(self.snapshot());

        let this = self.clone();
        let on_success = std::sync::Arc::clone(&self.on_success);
        let on_initialize = std::sync::Arc::clone(&self.on_initialize);
        let base_python = self
            .base_python
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        std::thread::spawn(move || {
            let mut pending_transaction = None;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<String, String> {
                    this.check_cancelled()?;
                    let target_hint = match &channel {
                        InstallChannel::Exact(v) => v.as_str(),
                        _ => "",
                    };
                    let python = base_python.unwrap_or_else(|| {
                        std::env::var("QWENPAW_PYTHON_BIN")
                            .map(std::path::PathBuf::from)
                            .unwrap_or_else(|_| {
                                std::path::PathBuf::from("/var/apps/python312/target/bin/python3")
                            })
                    });
                    crate::runtime::RuntimeAdapter::ensure_supported_python(&python)?;
                    let transaction = VenvTransaction::prepare_with_python_cancellable(
                        Path::new(&venv_dir),
                        task_seq,
                        target_hint,
                        Path::new(&pin_path),
                        &python,
                        &this,
                        &pid_file,
                    )
                    .map_err(|e| format!("无法准备独立安装环境: {e}"))?;
                    pending_transaction = Some(transaction);
                    let transaction = pending_transaction.as_mut().unwrap();
                    let python = transaction.python();
                    this.run_uv(&python, &channel, &mirror, &pip_log)?;
                    let mut metadata = std::process::Command::new(&python);
                    metadata.args([
                        "-c",
                        "import importlib.metadata as m; print(m.version('qwenpaw'))",
                    ]);
                    let metadata = this
                        .run_cancellable_command(
                            metadata,
                            Duration::from_secs(30),
                            "读取内核版本",
                            &pid_file,
                        )
                        .map_err(|error| format!("读取内核版本失败: {error}"))?;
                    if !metadata.status.success() {
                        return Err(format!(
                            "读取内核版本失败: {}",
                            String::from_utf8_lossy(&metadata.stderr).trim()
                        ));
                    }
                    let actual = String::from_utf8_lossy(&metadata.stdout).trim().to_string();
                    this.check_cancelled()?;
                    if actual.is_empty() {
                        return Err("安装命令成功但无法读取内核元数据版本,视为失败".into());
                    }
                    if let InstallChannel::Exact(want) = &channel {
                        if !version_matches(&actual, want) {
                            return Err(format!(
                                "安装版本({actual})与请求版本({want})不一致,视为失败"
                            ));
                        }
                    }
                    this.check_cancelled()?;
                    transaction
                        .activate()
                        .map_err(|e| format!("切换内核运行环境失败: {e}"))?;
                    this.check_cancelled()?;
                    (on_initialize)(&this, &pid_file)
                        .map_err(|error| format!("QwenPaw 初始化失败: {error}"))?;
                    this.check_cancelled()?;
                    this.set(|g| {
                        g.phase = Phase::Starting;
                        g.message = format!("内核包已安装 v{actual}，等待服务就绪");
                    });
                    this.check_cancelled()?;
                    (on_success)().map_err(|e| format!("内核启动失败 (spawn_failed): {e}"))?;
                    if !wait_for_version_cancellable(kernel_port, &actual, 30, &|| this.cancelled())
                    {
                        this.check_cancelled()?;
                        return Err(
                            "内核包已安装，但在宽限期内启动健康检查超时 (health_timeout)".into(),
                        );
                    }
                    this.check_cancelled()?;
                    transaction.record.resolved_version = Some(actual.clone());
                    check_crash_point(CrashPoint::AfterSpawnBeforeCommit);
                    transaction
                        .health_checked()
                        .map_err(|e| format!("安装状态落盘失败 (tx_persist_failed): {e}"))?;
                    this.check_cancelled()?;
                    transaction
                        .commit(Some(&format!("{actual}\n")))
                        .map_err(|e| format!("安装提交失败 (commit_failed): {e}"))?;
                    Ok(actual)
                },
            ));
            let outcome = outcome.unwrap_or_else(|panic| {
                let detail = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                Err(format!("安装线程异常终止 (panic): {detail}"))
            });
            this.finish_install(pending_transaction.as_mut(), Path::new(&venv_dir), outcome);
            drop(pending_transaction);
            let mut g = this.inner.lock().unwrap_or_else(|e| e.into_inner());
            g.worker_active = false;
            this.settled.notify_all();
        });
        true
    }
}

#[cfg(test)]
include!("tests.rs");

#[cfg(test)]
mod initializer_regression_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    fn fake_install_env() -> std::path::PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "qwenpaw-init-rollback-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let bin = dir.join("venv/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let python = bin.join("python3");
        std::fs::write(
            &python,
            r##"#!/bin/bash
if [ "$1" = "-m" ] && [ "$2" = "pip" ]; then cp "$0" "$(dirname "$0")/uv"; exit 0; fi
if [ "$(basename "$0")" = "uv" ] && [ "$1" = "pip" ] && [ "$2" = "install" ]; then printf new > "$(dirname "$0")/../installed"; exit 0; fi
printf '2.0.0\n'
"##,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("venv/installed"), "old").unwrap();
        super::tests::fake_base_python(&dir);
        dir
    }

    fn wait_for_install_terminal(state: &InstallState) -> InstallSnapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.snapshot().running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        state.snapshot()
    }

    #[test]
    fn initialization_runs_after_activation_before_start_and_rolls_back_on_failure() {
        let dir = fake_install_env();
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();
        let starts = Arc::new(AtomicU64::new(0));
        let start_count = Arc::clone(&starts);
        let initializes = Arc::new(AtomicU64::new(0));
        let initialize_count = Arc::clone(&initializes);
        let active_for_init = active.clone();
        let state = InstallState::with_lifecycle(
            Arc::new(|_| {}),
            Arc::new(move || {
                start_count.fetch_add(1, Ordering::SeqCst);
                Ok(1)
            }),
            Arc::new(|| {}),
            Arc::new(|| Ok(0)),
        )
        .with_initializer(Arc::new(move |_, _| {
            initialize_count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                std::fs::read_to_string(active_for_init.join("installed")).unwrap(),
                "new",
                "初始化只能在新 venv 激活后执行"
            );
            Err("injected init failure".into())
        }));
        state.set_base_python(dir.join("venv/base-python"));

        assert!(state.try_start(
            active.to_string_lossy().into_owned(),
            crate::api::test_support::free_port(),
            InstallChannel::Latest,
            String::new(),
            dir.join("pip.log").to_string_lossy().into_owned(),
            pin.to_string_lossy().into_owned(),
        ));
        let snapshot = wait_for_install_terminal(&state);

        assert_eq!(snapshot.phase, Phase::Failed, "{}", snapshot.error);
        assert!(snapshot.error.contains("injected init failure"));
        assert_eq!(initializes.load(Ordering::SeqCst), 1);
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old"
        );
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
