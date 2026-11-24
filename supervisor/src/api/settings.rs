#[cfg(test)]
use std::sync::Arc;

use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;

use super::{
    begin_operation, check_token, err_json, err_json_code, management_error_response, AppState,
};

pub(super) async fn kernel_reset_auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        password: String,
    }
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), 8192).await else {
        return err_json(StatusCode::BAD_REQUEST, "密码请求过大或无效");
    };
    if !serde_json::from_slice::<Input>(&bytes).is_ok_and(|v| !v.password.trim().is_empty()) {
        return err_json(StatusCode::BAD_REQUEST, "请提供非空的新密码");
    }
    let _operation = match begin_operation(&state) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
    let unavailable = || {
        err_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "请确认已启动带本版密码恢复适配的 QwenPaw 内核",
        )
    };
    if !state.kernel.supports_native_proxy() {
        if state.kernel.native_proxy_restart_required() {
            return err_json_code(
                StatusCode::SERVICE_UNAVAILABLE,
                "native_auth_adopted_restart_required",
                "当前内核由管理服务重启后接管，请先在概览重启 QwenPaw 服务，再重置密码",
            );
        }
        return unavailable();
    }
    let Ok(request) = Request::post(format!(
        "{}/api/auth/fnos-admin-password",
        state.kernel.base_path()
    ))
    .header("x-fnos-entry-command", "admin-password")
    .header("content-type", "application/json")
    .body(axum::body::Body::from(bytes)) else {
        return unavailable();
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(6),
        crate::entry_login::send(&state.kernel, request),
    )
    .await
    {
        Ok(Ok(reply)) => match reply.status() {
            StatusCode::NO_CONTENT => Json(json!({"success":true})).into_response(),
            StatusCode::CONFLICT => err_json_code(
                StatusCode::CONFLICT,
                "native_auth_uninitialized",
                "未启用原生鉴权或尚无已注册账号",
            ),
            StatusCode::PRECONDITION_FAILED => err_json_code(
                StatusCode::CONFLICT,
                "native_auth_mismatch",
                "原生账号与飞牛入口绑定不一致，未更改",
            ),
            StatusCode::SERVICE_UNAVAILABLE => err_json_code(
                StatusCode::SERVICE_UNAVAILABLE,
                "native_auth_unconfirmed",
                "无法确认账号记录或写入结果，请核对内核状态",
            ),
            _ => unavailable(),
        },
        _ => err_json_code(
            StatusCode::SERVICE_UNAVAILABLE,
            "native_auth_no_ack",
            "未收到内核确认，请先用新密码尝试登录",
        ),
    }
}

#[derive(serde::Deserialize)]
struct PortReq {
    port: u16,
    public: Option<bool>,
}

#[derive(serde::Deserialize)]
struct MirrorReq {
    mirror: String,
}

pub(super) async fn mirror_get(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    match crate::mirror::read(&state.var_dir) {
        Ok(mirror) => Json(json!({"success": true, "mirror": mirror})).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "mirror": "",
                "message": format!("下载源读取失败: {error}")
            })),
        )
            .into_response(),
    }
}

pub(super) async fn mirror_set(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let body = match axum::body::to_bytes(req.into_body(), 4 * 1024).await {
        Ok(body) => body,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let request = match serde_json::from_slice::<MirrorReq>(&body) {
        Ok(request) => request,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    match state.core.set_mirror(&request.mirror) {
        Ok(()) => Json(json!({"success": true, "code": "mirror_saved", "message": "下载源已保存"}))
            .into_response(),
        Err(error) => management_error_response(error),
    }
}

pub(super) async fn port(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    settings_request(state, req, true).await
}

pub(super) async fn web_proxy_port_get(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let Some(manager) = state.web_proxy.clone() else {
        return err_json(StatusCode::NOT_FOUND, "Web 反代未启用");
    };
    let (port, public) = manager.state().await;
    let mut data = json!({"success":true,"port":port,"public":public});
    match crate::entry_login::selected(&state.kernel.entry_record()) {
        Ok(true) => data["entryLogin"] = json!(true),
        Ok(false) => {}
        Err(_) => return err_json(StatusCode::SERVICE_UNAVAILABLE, "无法确认入口登录状态"),
    }
    Json(data).into_response()
}

pub(super) async fn entry_migrate(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let _operation = match begin_operation(&state) {
        Ok(g) => g,
        Err(response) => return *response,
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Proof {
        token: String,
    }
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), 8192).await else {
        return err_json(StatusCode::BAD_REQUEST, "请求无效");
    };
    let Ok(proof) = serde_json::from_slice::<Proof>(&bytes) else {
        return err_json(StatusCode::BAD_REQUEST, "旧会话格式无效");
    };
    match crate::entry_login::migrate(&state.kernel, proof.token).await {
        Ok(()) => {
            state.events.invalidate();
            Json(json!({"success":true})).into_response()
        }
        Err((status, msg)) => err_json(status, msg),
    }
}

pub(super) async fn web_proxy_port_set(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let _operation = match begin_operation(&state) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
    let body = match axum::body::to_bytes(req.into_body(), 4 * 1024).await {
        Ok(body) => body,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let request = match serde_json::from_slice::<PortReq>(&body) {
        Ok(request) if request.port == 0 || request.port >= 1024 => request,
        Ok(_) => {
            return err_json(
                StatusCode::BAD_REQUEST,
                "端口需为 0（自动分配）或 1024-65535",
            )
        }
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    if request.port != 0
        && (request.port == state.kernel.port() || request.port == state.supervisor_port)
    {
        return err_json_code(
            StatusCode::BAD_REQUEST,
            "port_in_use",
            "端口已被内核或管理器使用",
        );
    }
    let Some(manager) = state.web_proxy.clone() else {
        return err_json_code(
            StatusCode::NOT_FOUND,
            "web_proxy_disabled",
            "Web 反代未启用",
        );
    };
    let public = match request.public {
        Some(public) => public,
        None => manager.state().await.1,
    };
    match manager.reconfigure(request.port, public).await {
        Ok(port) => {
            state.events.invalidate();
            Json(json!({"success": true, "port": port, "code": "web_proxy_port_changed", "message": "Web 反代端口已切换"}))
                .into_response()
        }
        Err(error) => err_json(
            StatusCode::CONFLICT,
            &format!("无法绑定 Web 反代端口，已保留原端口: {error}"),
        ),
    }
}

pub(super) async fn settings(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    settings_request(state, req, false).await
}

async fn settings_request(
    state: AppState,
    req: Request<axum::body::Body>,
    port_only: bool,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let _operation = match begin_operation(&state) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
    let body = match axum::body::to_bytes(req.into_body(), 4 * 1024).await {
        Ok(body) => body,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let patch = if port_only {
        match serde_json::from_slice::<PortReq>(&body) {
            Ok(req) => crate::runtime_settings::SettingsPatch {
                port: Some(req.port),
                log_level: None,
            },
            Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
        }
    } else {
        match serde_json::from_slice::<crate::runtime_settings::SettingsPatch>(&body) {
            Ok(patch) => patch,
            Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
        }
    };
    if let Err(error) = patch.validate(state.supervisor_port) {
        return management_error_response(error);
    }
    match state.apply_runtime_settings(patch).await {
        Ok(message) => Json(json!({"ok": true, "success": true, "message": message,
            "port": state.kernel.port(), "log_level": state.kernel.log_level()}))
        .into_response(),
        Err(error) => management_error_response(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{install_python, ChildGuard, KernelGuard, TestDir, TestServer};
    use std::time::Duration;

    struct SlowAuthServer {
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl SlowAuthServer {
        fn start(listener: std::net::TcpListener) -> Self {
            listener.set_nonblocking(true).unwrap();
            let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let thread_shutdown = Arc::clone(&shutdown);
            let worker = std::thread::spawn(move || {
                use std::io::{Read, Write};
                while !thread_shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                            let mut request = [0; 1024];
                            let size = stream.read(&mut request).unwrap_or_default();
                            let auth_probe = request[..size].starts_with(b"GET /api/auth/status ");
                            if auth_probe {
                                std::thread::sleep(Duration::from_millis(500));
                            }
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
                            std::thread::sleep(Duration::from_millis(5));
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

    impl Drop for SlowAuthServer {
        fn drop(&mut self) {
            self.shutdown
                .store(true, std::sync::atomic::Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[tokio::test]
    async fn mirror_handlers_reject_missing_tokens_without_touching_the_mirror() {
        let dir = TestDir::new("mirror-token");
        let state = dir.state(crate::api::test_support::free_port());
        let path = std::path::Path::new(&state.var_dir).join("pip-mirror");
        let original = b"https://existing.example/simple\n";
        std::fs::write(&path, original).unwrap();

        let get = Request::builder()
            .method("GET")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            mirror_get(State(state.clone()), get).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let post = Request::builder()
            .method("POST")
            .body(axum::body::Body::from(
                r#"{"mirror":"https://attacker.example/simple"}"#,
            ))
            .unwrap();
        assert_eq!(
            mirror_set(State(state), post).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn settings_response_does_not_wait_for_slow_auth_snapshot() {
        let dir = TestDir::new("settings-slow-auth-snapshot");
        let port = crate::api::test_support::free_port();
        let state = dir.state(port);
        let _server =
            SlowAuthServer::start(crate::api::test_support::release_port_reservation(port));
        let _kernel = KernelGuard(Arc::clone(&state.kernel));
        install_python(&dir, &state, "import time\ntime.sleep(3600)\n");
        state.kernel.spawn().unwrap();
        assert!(
            tokio::task::spawn_blocking({
                let kernel = Arc::clone(&state.kernel);
                move || kernel.wait_healthy(Duration::from_secs(3))
            })
            .await
            .unwrap(),
            "kernel fixture must be healthy before settings mutation"
        );
        assert!(state.kernel.status_now().running);
        std::fs::write(
            &state.ui_config_file,
            json!({".url": {"qwenpaw.main": {"port": port.to_string()}}}).to_string(),
        )
        .unwrap();
        let events = state.events.subscribe();
        let request = Request::builder()
            .method("POST")
            .uri("/settings")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::from(
                json!({"log_level": "info"}).to_string(),
            ))
            .unwrap();

        let started = std::time::Instant::now();
        let response = settings(State(state.clone()), request).await;
        let elapsed = started.elapsed();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(events.has_changed().unwrap());
        assert!(
            elapsed < Duration::from_millis(275),
            "settings response waited for the slow auth probe in snapshot rendering: {elapsed:?}"
        );
        let render_started = std::time::Instant::now();
        let snapshot_state = state.clone();
        tokio::task::spawn_blocking(move || crate::api::render_snapshot(&snapshot_state))
            .await
            .unwrap();
        assert!(
            render_started.elapsed() >= Duration::from_millis(250),
            "fixture must keep snapshot auth probing slow"
        );
    }

    #[tokio::test]
    async fn port_change_stops_untracked_old_instance_before_changing_port() {
        use std::os::unix::process::CommandExt;
        let dir = TestDir::new("port-adoption");
        let old_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let new_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let old_port = old_listener.local_addr().unwrap().port();
        let new_port = new_listener.local_addr().unwrap().port();
        let state = dir.state(old_port);
        let _old_server = TestServer::from_listener(old_listener);
        let _new_server = TestServer::from_listener(new_listener);
        let _kernel = KernelGuard(Arc::clone(&state.kernel));
        std::fs::write(
            &state.ui_config_file,
            json!({".url": {"qwenpaw.main": {"port": old_port.to_string()}}}).to_string(),
        )
        .unwrap();
        let python = install_python(&dir, &state, "import time\ntime.sleep(3600)\n");
        let real_python = python.with_file_name("python3.real");
        std::fs::copy("/usr/bin/python3", &real_python).unwrap();
        let package = dir.0.join("qwenpaw");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(
            package.join("__main__.py"),
            r#"import os, pathlib, sys, time
port = int(sys.argv[sys.argv.index('--port') + 1])
root = pathlib.Path(os.environ['QWENPAW_TEST_ROOT'])
(root / ('ready-' + str(port))).write_text('ready')
time.sleep(3600)
"#,
        )
        .unwrap();
        let old = ChildGuard(
            std::process::Command::new(&real_python)
                .args(["-m", "qwenpaw", "app", "--port", &old_port.to_string()])
                .env("PYTHONPATH", &dir.0)
                .env("QWENPAW_TEST_ROOT", &dir.0)
                .process_group(0)
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !dir.0.join(format!("ready-{old_port}")).exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "old instance did not become ready"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let candidates = crate::procfs::rediscover_all_in_venv(
            old_port,
            0,
            &std::path::Path::new(&state.home_dir)
                .join("venv")
                .to_string_lossy(),
        )
        .unwrap();
        assert_eq!(candidates, vec![old.0.id() as i32]);
        let old_identity = crate::procfs::process_identity(old.0.id() as i32)
            .unwrap()
            .expect("fixture process identity must be readable before signaling");
        assert_eq!(old_identity.pgrp, old.0.id() as i32);
        assert_eq!(
            crate::procfs::identity_status(old_identity),
            crate::procfs::IdentityStatus::Alive
        );
        let executable = std::fs::read_link(format!("/proc/{}/exe", old.0.id())).unwrap();
        assert_eq!(
            executable.canonicalize().unwrap(),
            std::fs::canonicalize(&real_python).unwrap()
        );
        let cmdline = std::fs::read(format!("/proc/{}/cmdline", old.0.id())).unwrap();
        let argv: Vec<_> = cmdline
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(String::from_utf8_lossy)
            .collect();
        assert!(argv.iter().any(|arg| arg == &real_python.to_string_lossy()));
        assert!(argv.iter().any(|arg| arg == "-m"));
        assert!(argv.iter().any(|arg| arg == "qwenpaw"));
        assert!(argv.iter().any(|arg| arg == &old_port.to_string()));
        let req = Request::builder()
            .method("POST")
            .uri("/port")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::from(
                json!({"port": new_port}).to_string(),
            ))
            .unwrap();

        let response = port(State(state.clone()), req).await;
        let old_running = crate::procfs::process_alive(old.0.id() as i32);
        let old_status = crate::procfs::identity_status(old_identity);
        state.kernel.stop().await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !old_running,
            "old instance must be stopped using the old port before set_port"
        );
        assert_eq!(old_status, crate::procfs::IdentityStatus::Gone);
        assert_eq!(state.kernel.port(), new_port);
        assert_eq!(
            std::fs::read_to_string(dir.0.join("var/service_port")).unwrap(),
            format!("{new_port}\n")
        );
    }

    #[tokio::test]
    async fn reset_requires_existing_managed_kernel_and_valid_input() {
        let dir = TestDir::new("native-password");
        let state = dir.state(crate::api::test_support::free_port());
        let secret = dir.0.join(".qwenpaw.secret");
        std::fs::create_dir(&secret).unwrap();
        let auth = secret.join("auth.json");
        std::fs::write(&auth, "unchanged").unwrap();
        for (body, status) in [
            ("{}", StatusCode::BAD_REQUEST),
            (r#"{"password":"   "}"#, StatusCode::BAD_REQUEST),
            (r#"{"password":"fixture"}"#, StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let req = Request::post("/kernel/admin-password")
                .header("X-Supervisor-Token", &state.token)
                .body(axum::body::Body::from(body))
                .unwrap();
            assert_eq!(
                kernel_reset_auth(State(state.clone()), req).await.status(),
                status
            );
            assert_eq!(std::fs::read_to_string(&auth).unwrap(), "unchanged");
        }
    }
}
