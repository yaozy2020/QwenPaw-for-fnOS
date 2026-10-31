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

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.supervisor_log)
        .unwrap_or_else(|e| panic!("open supervisor log: {e}"));
    let log = Arc::new(Mutex::new(log_file));
    let started = Instant::now();
    let logf: Arc<dyn Fn(&str) + Send + Sync> = {
        let log = Arc::clone(&log);
        let started = started;
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

    // token:0600,缺失则生成;cgi 与监督者同读
    let token = match std::fs::read_to_string(&cfg.token_file) {
        Ok(t) if t.trim().len() >= 32 => t.trim().to_string(),
        _ => {
            let t = random_token();
            let _ = std::fs::write(&cfg.token_file, &t);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perm = std::fs::Permissions::from_mode(0o600);
                let _ = std::fs::set_permissions(&cfg.token_file, perm);
            }
            t
        }
    };
    logf("token file ready");

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
        etc_dir: cfg.etc_dir.clone(),
        share_dir: cfg.share_dir.clone(),
        kernel_log: cfg.kernel_log.clone(),
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

    // 信号:TERM/INT → 级联停内核 → 退出
    let kernel_for_signal = Arc::clone(&kernel);
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
            logf_for_signal("stopping kernel before exit");
            kernel_for_signal.stop().await;
        });
        std::process::exit(0);
    });

    // 主线程驻留(平台只关心进程活着)
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn random_token() -> String {
    let mut buf = vec![0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut buf);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
