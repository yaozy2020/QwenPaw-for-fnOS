//! 监督者入口:env 解析 → 日志 → token → pid → HTTP(127.0.0.1)→ 信号处理。
//! fnOS 停止应用 = TERM 本进程(Supervisor.pid)→ 优雅级联停内核 → 退出(D4)。
//! 监督者被 SIGKILL 时内核存活,下次启动由 /proc 收养(D1)。

mod api;
mod config;
mod events;
mod install;
mod kernel;
mod procfs;

use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use config::Config;
use events::Events;
use install::InstallState;
use kernel::KernelManager;

fn main() {
    let cfg = Config::load();
    std::fs::create_dir_all(&cfg.var_dir).ok();
    std::fs::create_dir_all(&cfg.share_dir).ok();

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
            let mut g = log.lock().unwrap();
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
        },
        cfg.kernel_port,
    ));
    let events = Events::new();
    let kernel_for_install = Arc::clone(&kernel);
    let install = InstallState::new(
        {
            let log = Arc::clone(&log);
            Arc::new(move |snap: install::InstallSnapshot| {
                let mut g = log.lock().unwrap();
                let _ = writeln!(
                    g,
                    "[Supervisor] install seq={} phase={:?} fetched={} msg={}",
                    snap.seq, snap.phase, snap.fetched, snap.message
                );
                // pip 失败不能把服务留在停机态(独立审计 major):失败终态尝试
                // 拉起现有内核;半改写 venv 时 spawn 可能失败,如实落日志
                if snap.phase == install::Phase::Failed {
                    let _ = writeln!(g, "[Supervisor] install failed, attempting kernel restart");
                    match kernel_for_install.spawn() {
                        Ok(pid) => { let _ = writeln!(g, "[Supervisor] kernel restarted after failed install, pid={pid}"); }
                        Err(e) => { let _ = writeln!(g, "[Supervisor] kernel restart after failed install FAILED: {e}"); }
                    }
                }
            })
        },
        {
            let kernel = Arc::clone(&kernel);
            Arc::new(move || {
                if let Err(e) = kernel.spawn() {
                    eprintln!("[Supervisor] autostart after install: {e}");
                }
            })
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
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&cfg.token_file)
                .unwrap_or_else(|e| panic!("create token file: {e}"));
            f.write_all(t.as_bytes())
                .unwrap_or_else(|e| panic!("write token file: {e}"));
            f.sync_all().ok();
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

    // supervisor 自身 pid(平台 stop 的目标)
    std::fs::write(&cfg.supervisor_pid_file, format!("{}\n", std::process::id())).ok();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

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
        kernel_log: cfg.kernel_log.clone(),
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
