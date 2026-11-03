//! 监督者入口:env 解析 → 日志 → token → pid → HTTP(127.0.0.1)→ 信号处理。
//! fnOS 停止应用 = TERM 本进程(Supervisor.pid)→ 优雅级联停内核 → 退出(D4)。
//! 监督者被 SIGKILL 时内核存活,下次启动由 /proc 收养(D1)。

mod api;
mod config;
mod events;
mod install;
mod kernel;
mod procfs;
mod version;

use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use config::Config;
use events::Events;
use install::InstallState;
use kernel::KernelManager;

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
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(file)
}

fn main() {
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
    if let Err(e) = api::sync_main_entry(
        std::path::Path::new(&cfg.ui_config_file),
        cfg.kernel_port,
        kernel.installed(),
    ) {
        logf(&format!("sync desktop kernel entry failed: {e}"));
    }
    let events = Events::new();
    let kernel_for_install = Arc::clone(&kernel);
    let ui_config_for_install = cfg.ui_config_file.clone();
    let install = InstallState::with_stop(
        {
            let log = Arc::clone(&log);
            Arc::new(move |snap: install::InstallSnapshot| {
                if matches!(snap.phase, install::Phase::Done | install::Phase::Failed | install::Phase::Cancelled) {
                    kernel_for_install.invalidate_version();
                    if let Err(e) = api::sync_main_entry(
                        std::path::Path::new(&ui_config_for_install),
                        kernel_for_install.port(),
                        kernel_for_install.installed(),
                    ) {
                        let _ = writeln!(lock_log(&log), "[Supervisor] sync desktop kernel entry failed: {e}");
                    }
                }
                let mut g = lock_log(&log);
                let _ = writeln!(
                    g,
                    "[Supervisor] install seq={} phase={:?} fetched={} msg={}",
                    snap.seq, snap.phase, snap.fetched, snap.message
                );
                // 失败终态统一先停掉可能未通过健康检查的新进程,再启动
                // 未受影响的旧环境(或回滚后的环境),避免进程与 venv 错配。
                if snap.phase == install::Phase::Failed {
                    let _ = writeln!(g, "[Supervisor] install failed, restoring previous kernel service");
                    kernel_for_install.stop_sync();
                    match kernel_for_install.spawn() {
                        Ok(pid) => { let _ = writeln!(g, "[Supervisor] kernel restarted after failed install, pid={pid}"); }
                        Err(e) => { let _ = writeln!(g, "[Supervisor] kernel restart after failed install FAILED: {e}"); }
                    }
                }
            })
        },
        {
            let kernel = Arc::clone(&kernel);
            Arc::new(move || kernel.spawn())
        },
        {
            // 安装取消/回滚前必须先停掉已拉起的新内核
            let kernel = Arc::clone(&kernel);
            Arc::new(move || kernel.stop_sync())
        },
    );

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
            let tmp_path = format!(
                "{}.tmp.{}",
                cfg.token_file,
                std::process::id()
            );
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
    cleanup_orphan_install(&cfg.var_dir);

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
    match install::recover_pending_transactions(
        std::path::Path::new(&cfg.kernel_venv),
        std::path::Path::new(&pin_file),
        Some(&*logf),
        Some(&stop_for_recovery),
    ) {
        Ok(n) if n > 0 => logf(&format!("recovered {n} pending venv transaction(s)")),
        Ok(_) => {}
        Err(e) => logf(&format!("recover pending transactions error: {e}")),
    }

    // supervisor 自身 pid(平台 stop 的目标)。
    // 必须原子写:裸 fs::write 先截断再写,若此刻被 SIGKILL/断电,磁盘上会留下
    // 空文件或半截数字。平台 stop 读到空/非法 pid 会判定"未运行"而跳过停止,
    // 于是真实例变成无法通过平台停下的孤儿(只剩手工 kill)。
    match crate::install::atomic_write_0600(
        &cfg.supervisor_pid_file,
        &format!("{}\n", std::process::id()),
    ) {
        Ok(()) => {}
        Err(e) => logf(&format!("write supervisor pid error: {e}")),
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // 崩溃恢复:管理员重置流程在 rename(auth→bak-reset) 之后、respawn 健康
    // 之前被杀时,重启后 auth.json 缺失会被判"未初始化",任何人都能重新
    // 设置管理员接管实例 —— 必须先从备份还原再对外服务。
    {
        let auth = std::path::Path::new(&cfg.share_dir)
            .parent()
            .map(|p| p.join(".qwenpaw.secret/auth.json"));
        if let Some(auth) = auth {
            let bak = auth.with_extension("json.bak-reset");
            if !auth.exists() && bak.exists() {
                match std::fs::rename(&bak, &auth) {
                    Ok(()) => logf("recovery: 从 bak-reset 还原管理员认证文件"),
                    Err(e) => logf(&format!("recovery: 还原认证文件失败: {e}")),
                }
            }
        }
    }

    // 启动时收养存活内核(D1);未安装且 autostart 开启时拉起(缺失则走 not_installed)
    let (adopt_pid, running) = rt.block_on(async {
        let st = kernel.status_now();
        (st.pid, st.running)
    });
    if running {
        logf(&format!("adopted running kernel pid={adopt_pid}"));
    } else if cfg.auto_start && kernel.installed() {
        logf("kernel not running, autostart");
        let k = Arc::clone(&kernel);
        let logf = Arc::clone(&logf);
        rt.spawn(async move {
            if let Err(e) = k.spawn() {
                logf(&format!("autostart failed: {e}"));
            }
        });
    }

    let state = api::AppState {
        kernel: Arc::clone(&kernel),
        install: install.clone(),
        events: events.clone(),
        token: token.clone(),
        var_dir: cfg.var_dir.clone(),
        home_dir: cfg.home_dir.clone(),
        share_dir: cfg.share_dir.clone(),
        ui_config_file: cfg.ui_config_file.clone(),
        kernel_log: cfg.kernel_log.clone(),
        operations: Arc::new(tokio::sync::Mutex::new(())),
        auth_cache: Arc::new(Mutex::new(api::AuthCache::default())),
        supervisor_port: cfg.listen_port,
    };
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

    // 信号:TERM/INT → 先取消进行中的安装(整组回收 pip)→ 级联停内核 → 退出。
    // 顺序不可换:内核停了安装线程还可能继续改 venv;先停任务源再停运行态。
    let kernel_for_signal = Arc::clone(&kernel);
    let install_for_signal = install.clone();
    let logf_for_signal = Arc::clone(&logf);
    let rt_handle = rt.handle().clone();
    std::thread::spawn(move || {
        rt_handle.block_on(async move {
            // 信号流注册必须在 runtime 上下文内(否则 panic: no reactor running)
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("SIGINT handler");
            tokio::select! {
                _ = term.recv() => logf_for_signal("SIGTERM"),
                _ = int.recv() => logf_for_signal("SIGINT"),
            }
            install_for_signal.cancel();
            logf_for_signal("install cancelled, stopping kernel before exit");
            kernel_for_signal.stop().await;
        });
        std::process::exit(0);
    });

    // 主线程驻留(平台只关心进程活着)
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
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
    let mut f = std::fs::File::open("/dev/urandom").map_err(|e| format!("open /dev/urandom: {e}"))?;
    f.read_exact(&mut buf)
        .map_err(|e| format!("read /dev/urandom: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// 上一次监督者运行遗留的孤儿 pip 清理:install.pid → /proc cmdline 身份核对
/// (须含 venv pip 特征,防 PID 复用误杀)→ 进程组 TERM → 宽限 → KILL。
fn cleanup_orphan_install(var_dir: &str) {
    let pid_file = std::path::Path::new(var_dir).join("install.pid");
    let pid: i32 = match std::fs::read_to_string(&pid_file) {
        Ok(s) => match s.trim().parse() {
            Ok(v) => v,
            Err(_) => {
                let _ = std::fs::remove_file(&pid_file);
                return;
            }
        },
        Err(_) => return,
    };
    let _ = std::fs::remove_file(&pid_file);
    if pid <= 1 || !crate::procfs::process_alive(pid) {
        return;
    }
    // 身份核对:cmdline 必须包含 pip 特征,避免陈旧 PID 文件指向无关进程
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    if !cmdline.contains("pip") {
        return;
    }
    unsafe { libc::kill(-pid, libc::SIGTERM) };
    for _ in 0..5 {
        if !crate::procfs::process_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    unsafe { libc::kill(-pid, libc::SIGKILL) };
}
