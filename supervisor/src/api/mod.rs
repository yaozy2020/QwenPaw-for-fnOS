//! 路由与处理器。鉴权:X-Supervisor-Token 头(vs VAR_DIR/supervisor-token,0600)。
//! loopback HTTP 提供本机 API;Gateway UDS 在 peer/角色/CSRF 门禁后复用这些处理器。
//! 响应形状供桌面控制台和 Gateway API 使用。

use std::sync::Arc;

use axum::{
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use crate::management::{ManagementCore, ManagementError, OperationGuard};

mod installation;
mod lifecycle;
mod logs;
mod settings;
mod status;

pub use crate::desktop::sync_main_entry;
pub use status::{render_snapshot, AuthCache};

#[cfg(test)]
use crate::install::InstallState;

use installation::{install_status, kernel_install, kernel_pin, kernel_uninstall, kernel_versions};
use lifecycle::{restart, start, stop};
use logs::{install_log, logs, logs_clear};
use settings::{kernel_reset_auth, port, settings};
use status::{events, status};

#[derive(Clone)]
pub struct AppState {
    pub core: ManagementCore,
    pub token: String,
    pub auth_cache: Arc<std::sync::Mutex<AuthCache>>,
    pub web_proxy: Option<Arc<crate::gateway::WebProxyManager>>,
}

impl std::ops::Deref for AppState {
    type Target = ManagementCore;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl std::ops::DerefMut for AppState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/start", post(start))
        .route("/stop", post(stop))
        .route("/restart", post(restart))
        .route("/events", get(events))
        .route("/logs", get(logs))
        .route("/logs/clear", post(logs_clear))
        .route("/kernel/install", post(kernel_install))
        .route("/kernel/uninstall", post(kernel_uninstall))
        .route("/kernel/install-status", get(install_status))
        .route("/kernel/install-log", get(install_log))
        .route("/kernel/versions", get(kernel_versions))
        .route("/kernel/pin", post(kernel_pin))
        .route("/kernel/reset-auth", post(kernel_reset_auth))
        .route("/port", post(port))
        .route(
            "/web-proxy/port",
            get(settings::web_proxy_port_get).post(settings::web_proxy_port_set),
        )
        .route("/settings", post(settings))
        .route(
            "/mirror",
            get(settings::mirror_get).post(settings::mirror_set),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            finish_mutation,
        ))
        .with_state(state)
}

// Once a complete, authenticated write request is accepted, its handler owns the
// operation until commit/rollback finishes. Dropping an HTTP connection (or the
// Gateway's response deadline) only drops this waiter, never the operation task.
async fn finish_mutation(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if request.method() != axum::http::Method::POST {
        return next.run(request).await;
    }
    if !check_token(&state.token, &request) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let (parts, body) = request.into_parts();
    let body = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        axum::body::to_bytes(body, 64 * 1024),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => return err_json(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"),
        Err(_) => return err_json(StatusCode::REQUEST_TIMEOUT, "request body timed out"),
    };
    let request = Request::from_parts(parts, axum::body::Body::from(body));
    let (finished, response) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        // The observer also survives disconnect, so a task panic cannot silently
        // release the gate without marking the manager as requiring recovery.
        let result = match tokio::spawn(async move { next.run(request).await }).await {
            Ok(response) => response,
            Err(error) => {
                state
                    .install
                    .block_recovery(format!("管理任务异常退出: {error}"));
                err_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "管理任务异常退出，请检查恢复状态",
                )
            }
        };
        let _ = finished.send(result);
    });
    response
        .await
        .unwrap_or_else(|_| err_json(StatusCode::INTERNAL_SERVER_ERROR, "管理任务结果不可用"))
}

fn check_token(token: &str, req: &Request<axum::body::Body>) -> bool {
    match req.headers().get("X-Supervisor-Token") {
        Some(v) => subtle_eq(v.to_str().unwrap_or(""), token),
        None => false,
    }
}

fn subtle_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn err_json(code: StatusCode, msg: &str) -> axum::response::Response {
    (
        code,
        Json(json!({"ok": false, "success": false, "error": msg, "message": msg})),
    )
        .into_response()
}

pub(crate) fn management_error_status(error: &ManagementError) -> StatusCode {
    match error {
        ManagementError::OperationInProgress
        | ManagementError::RecoveryPending(_)
        | ManagementError::InstallBusy => StatusCode::CONFLICT,
        ManagementError::InvalidSettings(_) => StatusCode::BAD_REQUEST,
        ManagementError::OperationFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub(super) fn management_error_response(error: ManagementError) -> axum::response::Response {
    err_json(management_error_status(&error), &error.to_string())
}

type OperationResult = Result<OperationGuard, Box<axum::response::Response>>;

fn try_operation(lock: &Arc<tokio::sync::Mutex<()>>) -> OperationResult {
    ManagementCore::try_operation_lock(lock)
        .map_err(|error| Box::new(management_error_response(error)))
}

fn begin_operation(state: &AppState) -> OperationResult {
    state
        .core
        .begin_operation()
        .map_err(|error| Box::new(management_error_response(error)))
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        collections::HashMap,
        net::TcpListener,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Mutex, OnceLock,
        },
    };

    use super::{AppState, AuthCache};
    use crate::{
        events::Events,
        install::InstallState,
        kernel::{KernelConfig, KernelManager},
        management::ManagementCore,
    };

    struct PortReservation {
        id: u64,
        listener: TcpListener,
    }

    fn port_reservations() -> &'static Mutex<HashMap<u16, PortReservation>> {
        static RESERVATIONS: OnceLock<Mutex<HashMap<u16, PortReservation>>> = OnceLock::new();
        RESERVATIONS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn next_port_reservation_id() -> u64 {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    fn port_reservation_id(port: u16) -> Option<u64> {
        port_reservations()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&port)
            .map(|reservation| reservation.id)
    }

    pub(crate) fn free_port() -> u16 {
        loop {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            if matches!(port, 18088 | 18099 | 18100 | 19090 | 19091) {
                continue;
            }
            port_reservations()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    port,
                    PortReservation {
                        id: next_port_reservation_id(),
                        listener,
                    },
                );
            return port;
        }
    }

    pub(crate) fn release_port_reservation(port: u16) {
        port_reservations()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&port);
    }

    pub(crate) fn serve_reserved_port(port: u16) -> TestServer {
        let reservation = port_reservations()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&port)
            .unwrap_or_else(|| panic!("no test port reservation exists for {port}"));
        TestServer::from_listener(reservation.listener)
    }

    pub(crate) fn free_port_other_than(other: u16) -> u16 {
        loop {
            let port = free_port();
            if port != other {
                return port;
            }
            release_port_reservation(port);
        }
    }

    #[test]
    fn free_port_reservation_lives_until_test_dir_drops() {
        let port = free_port();
        let port_reservation = port_reservation_id(port).unwrap();
        let first_bind = TcpListener::bind(("127.0.0.1", port)).unwrap_err();
        assert_eq!(first_bind.kind(), std::io::ErrorKind::AddrInUse);

        let dir = TestDir::new("port-reservation");
        let state = dir.state(port);
        let supervisor_port = state.supervisor_port;
        let supervisor_reservation = port_reservation_id(supervisor_port).unwrap();
        let state_bind = TcpListener::bind(("127.0.0.1", port)).unwrap_err();
        assert_eq!(state_bind.kind(), std::io::ErrorKind::AddrInUse);
        let supervisor_bind = TcpListener::bind(("127.0.0.1", supervisor_port)).unwrap_err();
        assert_eq!(supervisor_bind.kind(), std::io::ErrorKind::AddrInUse);
        drop(state);
        assert_eq!(port_reservation_id(port), Some(port_reservation));
        assert_eq!(
            port_reservation_id(supervisor_port),
            Some(supervisor_reservation)
        );
        drop(dir);

        assert_ne!(port_reservation_id(port), Some(port_reservation));
        assert_ne!(
            port_reservation_id(supervisor_port),
            Some(supervisor_reservation)
        );
    }

    #[test]
    fn test_dir_does_not_release_replaced_port_reservation() {
        let dir = TestDir::new("replaced-port-reservation");
        let port = free_port();
        dir.track_reserved_port(port);
        let replacement_id = next_port_reservation_id();
        {
            let mut reservations = port_reservations()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let previous = reservations.remove(&port).unwrap();
            reservations.insert(
                port,
                PortReservation {
                    id: replacement_id,
                    listener: previous.listener,
                },
            );
        }
        drop(dir);

        assert_eq!(port_reservation_id(port), Some(replacement_id));
        release_port_reservation(port);
    }

    #[test]
    fn free_port_excludes_fixed_application_ports() {
        for _ in 0..8 {
            let port = free_port();
            assert!(port > 0);
            assert!(!matches!(port, 18088 | 18099 | 18100 | 19090 | 19091));
        }
    }

    pub(crate) struct TestDir(pub(crate) std::path::PathBuf, Mutex<Vec<(u16, u64)>>);

    impl TestDir {
        pub(crate) fn new(label: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "qwenpaw-api-{label}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path, Mutex::new(Vec::new()))
        }

        fn track_reserved_port(&self, port: u16) {
            if let Some(reservation_id) = port_reservation_id(port) {
                let mut ports = self.1.lock().unwrap_or_else(|error| error.into_inner());
                if !ports
                    .iter()
                    .any(|(reserved_port, _)| *reserved_port == port)
                {
                    ports.push((port, reservation_id));
                }
            }
        }

        pub(crate) fn state(&self, port: u16) -> AppState {
            let supervisor_port = free_port_other_than(port);
            self.track_reserved_port(port);
            self.track_reserved_port(supervisor_port);
            let home = self.0.join("home");
            let var = self.0.join("var");
            let share = self.0.join("share");
            for path in [&home, &var, &share] {
                std::fs::create_dir_all(path).unwrap();
            }
            AppState {
                core: ManagementCore {
                    kernel: Arc::new(KernelManager::new(
                        KernelConfig {
                            venv_dir: home.join("venv").to_string_lossy().into_owned(),
                            pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
                            log_file: var.join("kernel.log").to_string_lossy().into_owned(),
                            share_dir: share.to_string_lossy().into_owned(),
                            etc_dir: self.0.to_string_lossy().into_owned(),
                            log_level: "info".into(),
                        },
                        port,
                    )),
                    install: InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0))),
                    events: Events::new(),
                    var_dir: var.to_string_lossy().into_owned(),
                    home_dir: home.to_string_lossy().into_owned(),
                    share_dir: share.to_string_lossy().into_owned(),
                    etc_dir: self.0.to_string_lossy().into_owned(),
                    ui_config_file: self.0.join("ui-config").to_string_lossy().into_owned(),
                    kernel_log: var.join("kernel.log").to_string_lossy().into_owned(),
                    operations: Arc::new(tokio::sync::Mutex::new(())),
                    supervisor_port,
                },
                token: "test-token".into(),
                auth_cache: Arc::new(std::sync::Mutex::new(AuthCache::default())),
                web_proxy: None,
            }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let ports = self.1.get_mut().unwrap_or_else(|error| error.into_inner());
            let mut reservations = port_reservations()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for (port, reservation_id) in ports.drain(..) {
                if matches!(reservations.get(&port), Some(reservation) if reservation.id == reservation_id)
                {
                    reservations.remove(&port);
                }
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) struct TestServer {
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl TestServer {
        pub(crate) fn from_listener(listener: TcpListener) -> Self {
            listener.set_nonblocking(true).unwrap();
            let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let thread_shutdown = Arc::clone(&shutdown);
            let worker = std::thread::spawn(move || {
                use std::io::{Read, Write};
                while !thread_shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream
                                .set_read_timeout(Some(std::time::Duration::from_millis(300)));
                            let mut request = [0; 4096];
                            let size = stream.read(&mut request).unwrap_or_default();
                            let body: &[u8] = if request[..size].starts_with(b"GET /api/version ") {
                                br#"{"version":"2.3.0"}"#
                            } else {
                                br#"{"enabled":true,"has_users":true}"#
                            };
                            let _ = write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(body);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                shutdown,
                worker: Some(worker),
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.shutdown
                .store(true, std::sync::atomic::Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    pub(crate) fn install_python(
        dir: &TestDir,
        state: &AppState,
        body: &str,
    ) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = std::path::Path::new(&state.home_dir).join("venv/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let python = bin.join("python3");
        std::fs::write(
            &python,
            format!(
                "#!/usr/bin/python3\nimport pathlib, sys\nroot = pathlib.Path({})\nif '-c' in sys.argv:\n    print('2.3.0')\n    sys.exit(0)\n{body}",
                serde_json::to_string(&dir.0.to_string_lossy()).unwrap()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let python_template = serde_json::to_string(&python.to_string_lossy()).unwrap();
        let base_python = dir.0.join("base-python");
        std::fs::write(
            &base_python,
            format!(
                "#!/usr/bin/python3\nimport pathlib, shutil, sys\nif sys.argv[1:] == ['--version']:\n    print('Python 3.12.0')\n    sys.exit(0)\nif len(sys.argv) < 4 or sys.argv[1:3] != ['-m', 'venv']:\n    sys.exit(2)\nbin_dir = pathlib.Path(sys.argv[3]) / 'bin'\nbin_dir.mkdir(parents=True)\nshutil.copy2({python_template}, bin_dir / 'python3')\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&base_python, std::fs::Permissions::from_mode(0o755)).unwrap();
        state.install.set_base_python(base_python);
        python
    }

    pub(crate) struct ChildGuard(pub(crate) std::process::Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    pub(crate) struct KernelGuard(pub(crate) Arc<KernelManager>);

    impl Drop for KernelGuard {
        fn drop(&mut self) {
            self.0.stop_sync();
        }
    }
}

pub(super) fn instance_id() -> &'static str {
    static INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| crate::random_token().expect("generate supervisor instance identity"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_guard_rejects_concurrent_changes() {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let _guard = try_operation(&lock).unwrap();
        let response = try_operation(&lock).unwrap_err();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn terminal_observer_keeps_mutations_blocked_without_stopping_the_kernel() {
        use std::time::Duration;
        use test_support::{free_port, install_python, serve_reserved_port, KernelGuard, TestDir};

        let dir = TestDir::new("terminal-operation-gate");
        let port = free_port();
        let mut state = dir.state(port);
        let _server = serve_reserved_port(port);
        let _kernel = KernelGuard(Arc::clone(&state.kernel));
        install_python(
            &dir,
            &state,
            r#"if 'pip' in sys.argv:
    sys.exit(1)
import time
time.sleep(3600)
"#,
        );
        let before = state.kernel.spawn().unwrap();
        assert!(state.kernel.wait_healthy(Duration::from_secs(3)));
        let snapshot: serde_json::Value = serde_json::from_str(&render_snapshot(&state)).unwrap();
        assert_eq!(snapshot["authStatus"], "protected");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        state.install = InstallState::new(
            Arc::new(move |snap| {
                if snap.phase == crate::install::Phase::Failed {
                    entered_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(20))
                        .unwrap();
                }
            }),
            Arc::new(|| Ok(0)),
        );
        state.install.set_base_python(dir.0.join("base-python"));
        assert!(state.install.try_start(
            format!("{}/venv", state.home_dir),
            port,
            crate::install::InstallChannel::Exact("2.0.0".into()),
            String::new(),
            format!("{}/pip.log", state.var_dir),
            format!("{}/qwenpaw-version", state.var_dir),
        ));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let operation = begin_operation(&state)
            .err()
            .map(|response| response.status());
        let request = Request::builder()
            .method("POST")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::from(r#"{"version":"2.0.0"}"#))
            .unwrap();
        let response = kernel_install(axum::extract::State(state.clone()), request).await;
        let after = state.kernel.status_now().pid;
        release_tx.send(()).unwrap();
        assert!(state.install.shutdown(Duration::from_secs(2)));
        assert_eq!(operation, Some(StatusCode::CONFLICT));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            before, after,
            "busy installation must not stop the existing kernel"
        );
    }

    #[tokio::test]
    async fn slow_operation_does_not_block_status_and_rejects_concurrent_ops() {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let operations = Arc::clone(&lock);

        // 模拟耗时运维操作（持有互斥锁）
        let guard = operations.clone().try_lock_owned().unwrap();
        let slow_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            drop(guard);
        });

        // 1. 并发运维变更立即返回 409 Conflict
        let concurrent_err = try_operation(&operations).unwrap_err();
        assert_eq!(concurrent_err.status(), StatusCode::CONFLICT);

        // 2. 状态查询不被拖住，毫秒级即时响应
        let start = std::time::Instant::now();
        let inst = InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0)));
        let snap = inst.snapshot();
        assert!(!snap.running);
        assert!(
            start.elapsed() < std::time::Duration::from_millis(50),
            "status 绝不被耗时运维操作拖住"
        );

        slow_task.await.unwrap();
        // 慢操作结束后锁成功释放
        assert!(try_operation(&operations).is_ok());
    }
}
