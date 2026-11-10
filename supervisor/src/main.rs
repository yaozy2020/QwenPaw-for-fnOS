//! 监督者入口:env 解析 → 日志 → token → pid → loopback HTTP / 可选 Gateway UDS → 信号处理。
//! fnOS 停止应用 = TERM 本进程(Supervisor.pid)→ 优雅级联停内核 → 退出(D4)。
//! 监督者被 SIGKILL 时内核存活,下次启动由 /proc 收养(D1)。

mod api;
mod cli;
mod config;
mod control;
mod desktop;
mod events;
mod gateway;
mod helper;
mod install;
mod kernel;
mod management;
mod mirror;
mod platform;
mod procfs;
mod runtime;
mod runtime_settings;
mod service;
mod settings_transaction;
mod uninstall;
mod version;
mod version_index;
mod web_static;

use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use config::Config;
use events::Events;
use install::InstallState;
use kernel::KernelManager;
use management::ManagementCore;

/// 对 VAR_DIR/supervisor.lock 取独占 flock(LOCK_EX|LOCK_NB)。
/// 返回的 File 必须在整个进程生命周期内保持打开 —— drop 即释放锁。
/// 日志锁解毒取用。日志写入途中的 panic(FIFO/满盘/EPIPE)会毒化 Mutex,
/// 之后每次 `.unwrap()` 都连锁 panic,监督者从此彻底哑火。
/// 日志是最后的诊断手段,必须比业务状态更宽容:中毒也照写。
fn lock_log(log: &Mutex<std::fs::File>) -> std::sync::MutexGuard<'_, std::fs::File> {
    match log.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            log.clear_poison();
            poisoned.into_inner()
        }
    }
}

fn acquire_instance_lock(var_dir: &str) -> std::io::Result<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    let path = std::path::Path::new(var_dir).join("supervisor.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    let fd = file.as_raw_fd();
    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if fd_flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(file)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Command::Serve) => serve(),
        Ok(cli::Command::Service(action)) => {
            let cfg = Config::load();
            std::process::exit(service::run(action, &cfg));
        }
        Ok(cli::Command::Platform(action)) => {
            let cfg = Config::load();
            std::process::exit(platform::run(action, &cfg));
        }
        Ok(cli::Command::DoctorJson) => {
            let cfg = Config::load();
            println!("{}", service::doctor_json(&cfg));
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}

fn serve() {
    let cfg = Config::load();
    std::fs::create_dir_all(&cfg.var_dir).ok();
    std::fs::create_dir_all(&cfg.share_dir).ok();

    // 单实例锁:平台重试或 start 竞态可能拉起第二个监督者。
    // 没有这把锁时只能靠端口 bind 失败间接收敛,而退出前已经动过 pid 文件与日志。
    // 锁在进程存活期间一直持有(fd 不关闭),进程崩溃时内核自动释放。
    let _instance_lock = match acquire_instance_lock(&cfg.var_dir) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("[Supervisor] already running or cannot lock: {e}");
            // 必须回报失败:exit(0) 会让平台把"根本没起来"当成启动成功,
            // 后续所有运维操作都在对着一个不存在的进程说话。
            std::process::exit(1);
        }
    };
    let gateway_socket = gateway::socket_path_from_config(&cfg)
        .and_then(|path| {
            path.map(|path| gateway::bind(&path).map_err(|error| error.to_string()))
                .transpose()
        })
        .unwrap_or_else(|error| panic!("bind gateway socket: {error}"));

    // 日志治理:启动时对超过 10MB 的自产日志做单代轮转(保 1 代旧文件)。
    // 内核仍持有旧 fd 时其后续写入会落在改名后的文件里,代价可接受;
    // 频繁重启场景足以防止无限膨胀(P2 日志治理的最小实现)。
    for log_path in [&cfg.supervisor_log, &cfg.kernel_log] {
        rotate_if_large(log_path, 10 * 1024 * 1024);
    }

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.supervisor_log)
        .unwrap_or_else(|e| panic!("open supervisor log: {e}"));
    let log = Arc::new(Mutex::new(log_file));
    let started = Instant::now();
    let logf: Arc<dyn Fn(&str) + Send + Sync> = {
        let log = Arc::clone(&log);
        Arc::new(move |msg: &str| {
            let mut g = lock_log(&log);
            let _ = writeln!(g, "[Supervisor] +{}s {}", started.elapsed().as_secs(), msg);
        })
    };

    let kernel = Arc::new(KernelManager::new(
        kernel::KernelConfig {
            venv_dir: cfg.kernel_venv.clone(),
            pid_file: cfg.kernel_pid_file.clone(),
            log_file: cfg.kernel_log.clone(),
            share_dir: cfg.share_dir.clone(),
            etc_dir: cfg.etc_dir.clone(),
            log_level: cfg.kernel_log_level.clone(),
        },
        cfg.kernel_port,
    ));
    let events = Events::new();
    let kernel_for_install = Arc::clone(&kernel);
    let ui_config_for_install = cfg.ui_config_file.clone();
    let install = InstallState::with_lifecycle(
        {
            let log = Arc::clone(&log);
            Arc::new(move |snap: install::InstallSnapshot| {
                if matches!(
                    snap.phase,
                    install::Phase::Done | install::Phase::Failed | install::Phase::Cancelled
                ) {
                    kernel_for_install.invalidate_version();
                    if let Err(e) = api::sync_main_entry(
                        std::path::Path::new(&ui_config_for_install),
                        kernel_for_install.installed(),
                    ) {
                        let _ = writeln!(
                            lock_log(&log),
                            "[Supervisor] sync desktop kernel entry failed: {e}"
                        );
                    }
                }
                let _ = writeln!(
                    lock_log(&log),
                    "[Supervisor] install seq={} phase={:?} fetched={} msg={}",
                    snap.seq,
                    snap.phase,
                    snap.fetched,
                    snap.message
                );
            })
        },
        {
            let kernel = Arc::clone(&kernel);
            Arc::new(move || kernel.spawn())
        },
        {
            let kernel = Arc::clone(&kernel);
            Arc::new(move || kernel.stop_sync())
        },
        {
            let kernel = Arc::clone(&kernel);
            Arc::new(move || {
                kernel.invalidate_version();
                if !kernel.installed() {
                    return Ok(0);
                }
                let pid = kernel.spawn()?;
                if kernel.wait_healthy(Duration::from_secs(10)) {
                    Ok(pid)
                } else {
                    Err("原内核恢复后健康检查未通过".into())
                }
            })
        },
    )
    .with_initializer({
        let kernel = Arc::clone(&kernel);
        let venv_dir = cfg.kernel_venv.clone();
        let share_dir = cfg.share_dir.clone();
        let etc_dir = cfg.etc_dir.clone();
        Arc::new(move |state, pid_file| {
            runtime::RuntimeAdapter::new(&venv_dir, &share_dir, &etc_dir, kernel.log_level())
                .ensure_initialized_with_state(state, pid_file)
        })
    });

    // token:0600 全生命周期(新建即 0600;存量宽松权限收敛;写入后复核)。
    // 随机源不可用必须终止启动 —— 静默降级曾会生成全零弱 Token(审计 P0)。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 存量文件可能是旧版以宽松权限预置的(审计 M3),启动即收敛
        let _ = std::fs::set_permissions(&cfg.token_file, std::fs::Permissions::from_mode(0o600));
    }
    let token = match std::fs::read_to_string(&cfg.token_file) {
        Ok(t) if t.trim().len() >= 32 => t.trim().to_string(),
        _ => {
            let t = random_token().unwrap_or_else(|e| {
                panic!("无法生成安全 Token(随机源不可用),拒绝以弱凭据启动: {e}");
            });
            use std::os::unix::fs::OpenOptionsExt;
            // 首写必须原子:直接 truncate 已有文件再写,若此时被 SIGKILL/断电,
            // 磁盘会留下空文件或半截 Token —— 下次启动读到长度 <32 会重新生成,
            // 已拿到旧 Token 的前端(或正在重连的 SSE)就会全部 401。
            // 改为写同目录唯一 tmp -> fsync -> rename,读者只会看到旧值或新值。
            let tmp_path = format!("{}.tmp.{}", cfg.token_file, std::process::id());
            // 先清同 pid 残骸:上次运行崩在"已 create_new 未 rename"窗口时,
            // tmp 与本次 PID 相同(create_new(true))会必然失败 → panic 启动
            // 死循环。清扫 sweep 在 token 写入之后才跑,兜不住首发。
            let _ = std::fs::remove_file(&tmp_path);
            let mut f = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&tmp_path)
                .unwrap_or_else(|e| panic!("create token tmp file: {e}"));
            f.write_all(t.as_bytes())
                .unwrap_or_else(|e| panic!("write token file: {e}"));
            f.sync_all().ok();
            drop(f);
            std::fs::rename(&tmp_path, &cfg.token_file).unwrap_or_else(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                panic!("commit token file: {e}")
            });
            // 写入后复核(防 umask/外部因素),失败即退出而非带病运行
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&cfg.token_file)
                    .map(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode()
                    })
                    .unwrap_or(0);
                if mode & 0o777 != 0o600 {
                    let _ = std::fs::set_permissions(
                        &cfg.token_file,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
            }
            t
        }
    };
    logf("token file ready");

    // 孤儿安装清理:监督者异常退出时 pip(独立进程组)可能仍存活并继续改 venv。
    // 凭 install.pid 找到候选,核对 /proc cmdline 身份(防 PID 复用误杀)后整组回收。
    if let Err(error) = cleanup_orphan_install(&cfg.var_dir) {
        logf(&format!("孤儿安装进程清理失败: {error}"));
        install.block_recovery(format!("孤儿安装进程清理失败: {error}"));
        kernel.stop_sync();
    }

    // 临时文件残骸清扫:必须在事务恢复之前,否则 service_port.tmp.* 之类的
    // 半截文件会被后续 create_new 撞上而让写入永久失败。
    let swept = crate::install::sweep_stale_tmp_files(&cfg.var_dir);
    if swept > 0 {
        logf(&format!("已清扫 {swept} 个临时文件残留"));
    }

    // venv 事务恢复:上次被 SIGKILL/断电遗留的中间状态自愈(必须在收养内核与开放 API 前完成)
    let pin_file = format!("{}/qwenpaw-version", cfg.var_dir);
    // 回滚前先停掉可能由中断安装拉起的内核:只回滚磁盘会留下
    // "进程新版 + 磁盘旧版"的错配,而该进程的依赖已被抽走。
    let stop_for_recovery = {
        let k = Arc::clone(&kernel);
        move || k.stop_sync()
    };
    let recovery_error = match install::recover_pending_transactions(
        std::path::Path::new(&cfg.kernel_venv),
        std::path::Path::new(&pin_file),
        Some(&*logf),
        Some(&stop_for_recovery),
    ) {
        Ok(report) => {
            if report.recovered > 0 {
                logf(&format!(
                    "recovered {} pending venv transaction(s)",
                    report.recovered
                ));
            }
            if report.is_ready() {
                None
            } else {
                Some(report.pending.join("; "))
            }
        }
        Err(e) => Some(format!("recover pending transactions error: {e}")),
    };
    if let Some(reason) = recovery_error {
        logf(&reason);
        install.block_recovery(reason);
        kernel.stop_sync();
    }

    let settings_journal = std::path::Path::new(&cfg.var_dir).join("settings-transaction.json");
    let settings_recovery = match settings_journal.try_exists() {
        Ok(false) => Ok(false),
        Ok(true) => {
            kernel.stop_sync();
            settings_transaction::recover(
                std::path::Path::new(&cfg.var_dir),
                std::path::Path::new(&cfg.ui_config_file),
            )
        }
        Err(e) => Err(e),
    };
    match settings_recovery {
        Ok(true) => {
            let restored = Config::load();
            kernel.set_port(restored.kernel_port);
            let _ = kernel.set_log_level(&restored.kernel_log_level);
            logf("recovery: 已恢复中断前的端口、日志级别与桌面配置");
        }
        Ok(false) => {}
        Err(e) => {
            let reason = format!("配置事务恢复失败: {e}");
            logf(&reason);
            install.block_recovery(reason);
            kernel.stop_sync();
        }
    }

    let self_pid = std::process::id();
    let self_starttime = std::fs::read_to_string(format!("/proc/{self_pid}/stat"))
        .ok()
        .and_then(|stat| service::parse_starttime(&stat))
        .expect("read supervisor process starttime");
    service::mark_shutdown_generation(&cfg, self_pid as i32, self_starttime, false)
        .unwrap_or_else(|error| panic!("write running shutdown generation: {error}"));
    let starttime_file = format!("{}.starttime", cfg.supervisor_pid_file);
    crate::install::atomic_write_0600(&starttime_file, &format!("{self_starttime}\n"))
        .unwrap_or_else(|error| panic!("write supervisor starttime: {error}"));
    if let Err(error) =
        crate::install::atomic_write_0600(&cfg.supervisor_pid_file, &format!("{self_pid}\n"))
    {
        let _ = std::fs::remove_file(&starttime_file);
        panic!("write supervisor PID: {error}");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let auth = std::path::Path::new(&cfg.share_dir)
        .parent()
        .map(|p| p.join(".qwenpaw.secret/auth.json"));
    if let Some(auth) = auth {
        let bak = auth.with_extension("json.bak-reset");
        if !auth.exists() && bak.exists() {
            kernel.stop_sync();
            match std::fs::rename(&bak, &auth) {
                Ok(()) => logf("recovery: 从 bak-reset 还原管理员认证文件"),
                Err(e) => {
                    let reason = format!("认证恢复失败: {e}");
                    logf(&reason);
                    install.block_recovery(reason);
                }
            }
        }
    }

    if install.recovery_error().is_none() {
        if let Err(e) = api::sync_main_entry(
            std::path::Path::new(&cfg.ui_config_file),
            kernel.installed(),
        ) {
            logf(&format!("sync desktop kernel entry failed: {e}"));
        }
        let st = kernel.status_now();
        if st.running {
            logf(&format!("adopted running kernel pid={}", st.pid));
        } else if cfg.auto_start && kernel.installed() {
            logf("kernel not running, autostart");
            if let Err(e) = kernel.spawn() {
                logf(&format!("autostart failed: {e}"));
            }
        }
    }

    let core = ManagementCore {
        kernel: Arc::clone(&kernel),
        install: install.clone(),
        events: events.clone(),
        var_dir: cfg.var_dir.clone(),
        home_dir: cfg.home_dir.clone(),
        share_dir: cfg.share_dir.clone(),
        etc_dir: cfg.etc_dir.clone(),
        ui_config_file: cfg.ui_config_file.clone(),
        kernel_log: cfg.kernel_log.clone(),
        operations: Arc::new(tokio::sync::Mutex::new(())),
        supervisor_port: cfg.listen_port,
    };
    let mut state = api::AppState {
        core: core.clone(),
        token,
        auth_cache: Arc::new(Mutex::new(api::AuthCache::default())),
        web_proxy: None,
    };
    if let Some(port) = cfg.web_proxy_port {
        let public = std::env::var("QWENPAW_WEB_PROXY_PUBLIC")
            .ok()
            .or_else(|| std::fs::read_to_string(format!("{}/web_proxy_public", cfg.var_dir)).ok())
            .is_some_and(|value| value.trim() == "1");
        let manager = rt
            .block_on(gateway::WebProxyManager::start(state.clone(), port, public))
            .unwrap_or_else(|error| panic!("start web proxy: {error}"));
        let (actual_port, public) = rt.block_on(manager.state());
        state.web_proxy = Some(Arc::new(manager));
        logf(&format!(
            "web proxy listening {}:{actual_port}",
            if public { "0.0.0.0" } else { "127.0.0.1" }
        ));
    }
    let stable_helper_binary = cfg.stable_helper_binary();
    let persistent_helper_binary = crate::helper::path(&cfg.var_dir);
    let gateway = gateway_socket.map(|bound| {
        rt.block_on(async {
            gateway::start(
                bound,
                state.clone(),
                &cfg.supervisor_binary,
                &stable_helper_binary,
                &persistent_helper_binary,
            )
        })
        .unwrap_or_else(|error| panic!("start gateway listener: {error}"))
    });

    let control_path = control::socket_path(&cfg.var_dir);
    let control_listener = control::bind_private_socket(&control_path)
        .unwrap_or_else(|error| panic!("bind private control socket: {error}"));
    let control_state = state.clone();
    let control_runtime = rt.handle().clone();
    std::thread::spawn(move || control::serve(control_listener, control_state, control_runtime));

    let operations_for_signal = Arc::clone(&core.operations);
    let addr = format!("127.0.0.1:{}", cfg.listen_port);
    let listener = rt
        .block_on(async { tokio::net::TcpListener::bind(&addr).await })
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    let rt_handle = rt.handle().clone();
    std::thread::spawn(move || {
        rt_handle.block_on(async {
            axum::serve(listener, api::build_router(state)).await.ok();
        });
    });
    logf(&format!("listening {addr}"));

    let kernel_for_signal = Arc::clone(&core.kernel);
    let install_for_signal = core.install.clone();
    let logf_for_signal = Arc::clone(&logf);
    let cfg_for_signal = cfg.clone();
    let shutdown_pid = self_pid as i32;
    let shutdown_starttime = self_starttime;
    let rt_handle = rt.handle().clone();
    std::thread::spawn(move || {
        let clean = rt_handle.block_on(async move {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("SIGINT handler");
            tokio::select! {
                _ = term.recv() => logf_for_signal("SIGTERM"),
                _ = int.recv() => logf_for_signal("SIGINT"),
            }
            kernel_for_signal.prevent_start();
            install_for_signal.block_recovery("监督者正在关闭".into());
            let install_shutdown = tokio::task::spawn_blocking(move || {
                install_for_signal.shutdown(Duration::from_secs(15))
            });
            let (install_result, operation) = tokio::join!(
                install_shutdown,
                tokio::time::timeout(Duration::from_secs(15), operations_for_signal.lock_owned())
            );
            let operation_guard = operation.ok();
            let settled = matches!(install_result, Ok(true)) && operation_guard.is_some();
            if !settled {
                logf_for_signal("shutdown timeout: 未完成事务将在下次启动时恢复");
                kernel_for_signal.stop().await;
                return false;
            }

            kernel_for_signal.stop().await;
            let kernel_stop_clean = kernel_for_signal.last_stop_clean();
            let kernel_stopped = !kernel_for_signal.status_now().running;
            match mark_clean_generation_if_ready(settled, kernel_stop_clean, kernel_stopped, || {
                service::mark_shutdown_generation(
                    &cfg_for_signal,
                    shutdown_pid,
                    shutdown_starttime,
                    true,
                )
                .map_err(|error| error.to_string())
            }) {
                Ok(true) => true,
                Ok(false) => {
                    if !kernel_stop_clean {
                        logf_for_signal(
                            "shutdown failed: kernel stop did not verify every target process exit",
                        );
                    } else if !kernel_stopped {
                        logf_for_signal("shutdown failed: kernel is still running");
                    } else {
                        logf_for_signal(
                            "shutdown failed: transactions or operations did not settle",
                        );
                    }
                    false
                }
                Err(error) => {
                    logf_for_signal(&format!("write clean shutdown generation failed: {error}"));
                    if let Err(restore_error) = service::mark_shutdown_generation(
                        &cfg_for_signal,
                        shutdown_pid,
                        shutdown_starttime,
                        false,
                    ) {
                        logf_for_signal(&format!(
                            "restore running shutdown generation failed: {restore_error}"
                        ));
                    }
                    false
                }
            }
        });
        if let Some(gateway) = gateway {
            rt_handle.block_on(gateway.shutdown());
        }
        std::process::exit(if clean { 0 } else { 1 });
    });

    // 主线程驻留(平台只关心进程活着)
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn mark_clean_generation_if_ready(
    transactions_settled: bool,
    kernel_stop_clean: bool,
    kernel_stopped: bool,
    mark_clean: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    if !transactions_settled || !kernel_stop_clean || !kernel_stopped {
        return Ok(false);
    }
    mark_clean()?;
    Ok(true)
}

/// 单代日志轮转:path 超过 max 字节时改名为 path.1(覆盖旧 .1)
fn rotate_if_large(path: &str, max: u64) {
    if let Ok(m) = std::fs::metadata(path) {
        if m.len() > max {
            let rotated = format!("{path}.1");
            let _ = std::fs::remove_file(&rotated);
            let _ = std::fs::rename(path, rotated);
        }
    }
}

/// 32 字节 CSPRNG 随机数的十六进制;/dev/urandom 不可用或读不足即报错,
/// 调用方必须终止启动(静默降级曾会产出全零 Token)。
fn random_token() -> Result<String, String> {
    use std::io::Read;
    let mut buf = vec![0u8; 32];
    let mut f =
        std::fs::File::open("/dev/urandom").map_err(|e| format!("open /dev/urandom: {e}"))?;
    f.read_exact(&mut buf)
        .map_err(|e| format!("read /dev/urandom: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

const ORPHAN_INSTALL_TERM_GRACE: Duration = Duration::from_secs(2);
const ORPHAN_INSTALL_KILL_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InstallProcessRecord {
    pid: i32,
    starttime: u64,
    pgid: i32,
}

#[derive(Clone, Copy, Debug)]
struct OrphanProcessStat {
    state: char,
    starttime: u64,
    pgid: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrphanLeader {
    Match,
    Inactive,
}

fn parse_install_record(contents: &str) -> Result<InstallProcessRecord, String> {
    let mut fields: Vec<_> = contents.split('\n').collect();
    if fields.last() == Some(&"") {
        fields.pop();
    }
    if fields.len() != 3 {
        return Err("install.pid must contain exactly PID, starttime, and PGID".into());
    }
    let valid_decimal =
        |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    for (value, label) in fields.iter().zip(["PID", "starttime", "PGID"]) {
        if !valid_decimal(value) {
            return Err(format!("install.pid contains an invalid {label}"));
        }
    }
    let record = InstallProcessRecord {
        pid: fields[0]
            .parse()
            .map_err(|_| "install.pid contains an invalid PID".to_string())?,
        starttime: fields[1]
            .parse()
            .map_err(|_| "install.pid contains an invalid starttime".to_string())?,
        pgid: fields[2]
            .parse()
            .map_err(|_| "install.pid contains an invalid PGID".to_string())?,
    };
    if record.pid <= 1 || record.starttime == 0 || record.pgid <= 1 || record.pgid != record.pid {
        return Err("install.pid contains an invalid process identity".into());
    }
    Ok(record)
}

fn parse_orphan_stat(stat: &str) -> Option<OrphanProcessStat> {
    let end = stat.rfind(')')?;
    let fields: Vec<_> = stat.get(end + 2..)?.split_whitespace().collect();
    Some(OrphanProcessStat {
        state: fields.first()?.chars().next()?,
        pgid: fields.get(2)?.parse().ok()?,
        starttime: fields.get(19)?.parse().ok()?,
    })
}

fn install_argv_matches(cmdline: &[u8]) -> bool {
    let args = cmdline.split(|byte| *byte == 0).collect::<Vec<_>>();
    let direct_uv = args
        .first()
        .is_some_and(|executable| *executable == b"uv" || executable.ends_with(b"/uv"))
        && args.get(1) == Some(&b"pip".as_slice())
        && args.get(2) == Some(&b"install".as_slice());
    direct_uv
        || args
            .windows(2)
            .any(|args| args[0] == b"-m" && matches!(args[1], b"pip" | b"venv"))
}

fn inspect_orphan_leader(record: InstallProcessRecord) -> Result<Option<OrphanLeader>, String> {
    let stat_path = format!("/proc/{}/stat", record.pid);
    let stat = match std::fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", stat_path)),
    };
    let actual = parse_orphan_stat(&stat)
        .ok_or_else(|| format!("{} has malformed process stat", stat_path))?;
    if actual.starttime != record.starttime || actual.pgid != record.pgid {
        return Err(format!(
            "install.pid identity mismatch for PID {} (starttime/PGID changed); refusing to signal",
            record.pid
        ));
    }
    if !matches!(actual.state, 'R' | 'S' | 'D' | 'T' | 't') {
        return Ok(Some(OrphanLeader::Inactive));
    }
    let cmdline_path = format!("/proc/{}/cmdline", record.pid);
    let cmdline =
        std::fs::read(&cmdline_path).map_err(|error| format!("read {}: {error}", cmdline_path))?;
    if !install_argv_matches(&cmdline) {
        return Err(format!(
            "install.pid PID {} is not an exact pip, venv, or 'uv pip install' process; refusing to signal",
            record.pid
        ));
    }
    Ok(Some(OrphanLeader::Match))
}

fn process_group_has_members(pgid: i32) -> Result<bool, String> {
    let entries = std::fs::read_dir("/proc").map_err(|error| format!("scan /proc: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("scan /proc entry: {error}"))?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid <= 0 {
            continue;
        }
        let path = format!("/proc/{pid}/stat");
        let stat = match std::fs::read_to_string(&path) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("read {path}: {error}")),
        };
        let actual =
            parse_orphan_stat(&stat).ok_or_else(|| format!("{path} has malformed process stat"))?;
        if actual.pgid == pgid {
            return Ok(true);
        }
    }
    Ok(false)
}

fn wait_for_install_group_empty(pgid: i32, timeout: Duration) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if !process_group_has_members(pgid)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn signal_install_group(pgid: i32, signal: i32) -> Result<(), String> {
    if unsafe { libc::kill(-pgid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!(
            "signal {signal} to verified install PGID {pgid}: {error}"
        ))
    }
}

fn remove_install_record_if_unchanged(
    path: &std::path::Path,
    original: &str,
) -> Result<(), String> {
    match std::fs::read_to_string(path) {
        Ok(current) if current == original => {}
        Ok(_) => {
            return Err("install.pid changed during orphan cleanup; preserving registration".into())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("re-read {}: {error}", path.display())),
    }
    std::fs::remove_file(path).map_err(|error| format!("remove {}: {error}", path.display()))
}

/// Reclaim an orphaned installer only after validating its three-field identity,
/// process group, and exact Python module argv. Ambiguous registrations fail closed.
fn cleanup_orphan_install(var_dir: &str) -> Result<(), String> {
    let pid_file = std::path::Path::new(var_dir).join("install.pid");
    let original = match std::fs::read_to_string(&pid_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", pid_file.display())),
    };
    let record = parse_install_record(&original)
        .map_err(|error| format!("{}: {error}; preserving registration", pid_file.display()))?;

    match inspect_orphan_leader(record)? {
        Some(OrphanLeader::Match) => {
            if !matches!(inspect_orphan_leader(record)?, Some(OrphanLeader::Match)) {
                return Err(format!(
                    "install leader {} changed before TERM; preserving registration",
                    record.pid
                ));
            }
            signal_install_group(record.pgid, libc::SIGTERM)?;
            if wait_for_install_group_empty(record.pgid, ORPHAN_INSTALL_TERM_GRACE)? {
                return remove_install_record_if_unchanged(&pid_file, &original);
            }
            match inspect_orphan_leader(record)? {
                Some(OrphanLeader::Match | OrphanLeader::Inactive) => {}
                None => {
                    return if process_group_has_members(record.pgid)? {
                        Err(format!(
                            "install leader {} disappeared while PGID {} still has members; refusing to signal and preserving registration",
                            record.pid, record.pgid
                        ))
                    } else {
                        remove_install_record_if_unchanged(&pid_file, &original)
                    }
                }
            }
            if !process_group_has_members(record.pgid)? {
                return remove_install_record_if_unchanged(&pid_file, &original);
            }
            signal_install_group(record.pgid, libc::SIGKILL)?;
            if wait_for_install_group_empty(record.pgid, ORPHAN_INSTALL_KILL_GRACE)? {
                remove_install_record_if_unchanged(&pid_file, &original)
            } else {
                Err(format!(
                    "install process group {} survived TERM and KILL; preserving registration",
                    record.pgid
                ))
            }
        }
        Some(OrphanLeader::Inactive) | None => {
            if wait_for_install_group_empty(record.pgid, ORPHAN_INSTALL_TERM_GRACE)? {
                remove_install_record_if_unchanged(&pid_file, &original)
            } else {
                Err(format!(
                    "install leader {} is gone or inactive while PGID {} still has members; refusing to signal and preserving registration",
                    record.pid, record.pgid
                ))
            }
        }
    }
}

#[cfg(test)]
mod orphan_install_tests {
    use super::*;

    #[test]
    fn parses_only_the_three_field_install_registration() {
        assert_eq!(
            parse_install_record("42\n1234\n42\n").unwrap(),
            InstallProcessRecord {
                pid: 42,
                starttime: 1234,
                pgid: 42,
            }
        );
        for invalid in [
            "42\n1234\n",
            "42\n1234\n43\n",
            "42\n1234\n42\nextra\n",
            "0\n1234\n0\n",
            "42\n0\n42\n",
            "+42\n1234\n42\n",
            "42 \n1234\n42\n",
        ] {
            assert!(
                parse_install_record(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn install_argv_requires_exact_adjacent_python_module_arguments() {
        assert!(install_argv_matches(
            b"python3\0-m\0pip\0install\0qwenpaw\0"
        ));
        assert!(install_argv_matches(b"python3\0-m\0venv\0/tmp/venv\0"));
        assert!(!install_argv_matches(b"python3\0not-m\0pip\0"));
        assert!(!install_argv_matches(b"python3\0-m\0pipx\0"));
        assert!(!install_argv_matches(b"python3\0pip\0-m\0"));
        assert!(!install_argv_matches(b"pip install qwenpaw\0"));
    }

    #[test]
    fn direct_uv_orphan_requires_executable_and_exact_subcommands() {
        for cmdline in [
            &b"uv\0pip\0install\0qwenpaw\0"[..],
            &b"/tmp/staging/bin/uv\0pip\0install\0--python\0/tmp/python3\0"[..],
        ] {
            assert!(install_argv_matches(cmdline), "rejected {cmdline:?}");
        }
        for cmdline in [
            &b"uvx\0pip\0install\0"[..],
            &b"/tmp/not-uv\0pip\0install\0"[..],
            &b"uv\0pipx\0install\0"[..],
            &b"uv\0pip\0uninstall\0"[..],
            &b"uv\0--help\0pip\0install\0"[..],
            &b"python3\0/tmp/uv\0pip\0install\0"[..],
        ] {
            assert!(!install_argv_matches(cmdline), "accepted {cmdline:?}");
        }
    }

    #[test]
    fn non_uv_executable_without_direct_uv_argv_is_not_an_orphan_installer() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let root = std::env::temp_dir().join(format!("qwenpaw-not-uv-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("not-uv");
        std::fs::write(&script, "printf ready > ready\nIFS= read -r line\n").unwrap();
        let mut child = crate::api::test_support::ChildGuard(
            Command::new("/bin/bash")
                .arg(&script)
                .current_dir(&root)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("ready").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            root.join("ready").exists(),
            "impostor fixture did not start"
        );
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let parsed = parse_orphan_stat(&stat).unwrap();
        let record = InstallProcessRecord {
            pid,
            starttime: parsed.starttime,
            pgid: pid,
        };
        let observed = inspect_orphan_leader(record);
        let still_alive = child.0.try_wait().unwrap().is_none();
        let _ = child.0.kill();
        let _ = child.0.wait();
        std::fs::remove_dir_all(root).unwrap();
        assert!(still_alive);
        assert!(
            observed.is_err(),
            "non-uv executable was accepted: {observed:?}"
        );
    }

    #[test]
    fn direct_uv_orphan_cleanup_checks_identity_and_reaps_only_its_group() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        let root = std::env::temp_dir().join(format!("qwenpaw-uv-orphan-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let uv = root.join("uv");
        std::fs::copy("/usr/bin/tail", &uv).unwrap();
        std::fs::write(root.join("pip"), "pip is a stable watched file\n").unwrap();
        std::fs::write(root.join("install"), "install is a stable watched file\n").unwrap();
        let mut child = Command::new(&uv)
            .args(["pip", "install", "-f"])
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let mut sentinel = crate::api::test_support::ChildGuard(
            Command::new("/bin/sleep")
                .arg("30")
                .process_group(0)
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            child.try_wait().unwrap().is_none(),
            "direct uv fixture exited early"
        );
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let parsed = parse_orphan_stat(&stat).unwrap();
        let record = InstallProcessRecord {
            pid,
            starttime: parsed.starttime,
            pgid: pid,
        };
        assert_eq!(parsed.pgid, pid);
        assert_eq!(
            inspect_orphan_leader(record).unwrap(),
            Some(OrphanLeader::Match)
        );
        let path = root.join("install.pid");
        let bad_record = format!("{pid}\n{}\n{pid}\n", parsed.starttime + 1);
        std::fs::write(&path, &bad_record).unwrap();
        assert!(cleanup_orphan_install(root.to_str().unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad_record);
        assert!(
            child.try_wait().unwrap().is_none(),
            "stale identity received a signal"
        );

        std::fs::write(&path, format!("{pid}\n{}\n{pid}\n", parsed.starttime)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        let cleanup = cleanup_orphan_install(root.to_str().unwrap());
        if cleanup.is_err()
            && matches!(inspect_orphan_leader(record), Ok(Some(OrphanLeader::Match)))
        {
            let _ = signal_install_group(pid, libc::SIGKILL);
        }
        let waited = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        waiter.join().unwrap();
        assert!(cleanup.is_ok(), "{cleanup:?}");
        assert!(!waited.success());
        assert!(!path.exists(), "successful cleanup must remove install.pid");
        assert!(
            sentinel.0.try_wait().unwrap().is_none(),
            "external sentinel was signalled"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mismatched_live_process_keeps_registration_and_is_not_signalled() {
        let root =
            std::env::temp_dir().join(format!("qwenpaw-orphan-install-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let pid = std::process::id() as i32;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let parsed = parse_orphan_stat(&stat).unwrap();
        let record = format!("{pid}\n{}\n{}\n", parsed.starttime, parsed.pgid);
        let path = root.join("install.pid");
        std::fs::write(&path, &record).unwrap();

        assert!(cleanup_orphan_install(root.to_str().unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), record);

        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod shutdown_generation_tests {
    use super::*;

    #[test]
    fn unverified_shutdown_never_invokes_clean_generation_writer() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qwenpaw-clean-generation-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let generation = root.join("shutdown-generation.json");

        for gates in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let path = generation.clone();
            let recorded = mark_clean_generation_if_ready(gates.0, gates.1, gates.2, || {
                std::fs::write(&path, "clean").map_err(|error| error.to_string())
            })
            .unwrap();
            assert!(!recorded);
            assert!(!generation.exists());
        }

        let path = generation.clone();
        assert!(mark_clean_generation_if_ready(true, true, true, || {
            std::fs::write(&path, "clean").map_err(|error| error.to_string())
        })
        .unwrap());
        assert_eq!(std::fs::read_to_string(&generation).unwrap(), "clean");
        std::fs::remove_dir_all(root).unwrap();
    }
}
