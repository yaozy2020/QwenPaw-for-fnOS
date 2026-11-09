use std::sync::Arc;

use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;

use super::{
    begin_operation, check_token, err_json, management_error_response, render_snapshot, AppState,
    AuthCache,
};
/// 重置管理员密码:移除 auth.json,下次打开 Web 端重新设定(旧 reset_auth 语义)。
/// 事务式:先备份重命名 → 重启 → 健康检查通过才删备份;失败恢复备份并如实报错。
/// 历史版本直接删文件 + `spawn().unwrap_or(0)`,重启失败时既丢了认证又假报成功。
pub(super) async fn kernel_reset_auth(
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
    let auth = std::path::Path::new(&state.share_dir)
        .parent()
        .map(|p| p.join(".qwenpaw.secret/auth.json"))
        .unwrap_or_default();
    if auth.as_os_str().is_empty() {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, "认证文件路径解析失败");
    }
    if !auth.exists() {
        return Json(json!({"ok": true, "success": true, "message": "密码本就未设置"}))
            .into_response();
    }
    let backup = auth.with_extension("json.bak-reset");
    // 删除会影响运行中内核的登录态,先备份而非直接删;重命名失败即中止
    if let Err(e) = std::fs::rename(&auth, &backup) {
        return err_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("认证文件备份失败: {e}"),
        );
    }
    state.kernel.stop().await;
    let kernel = Arc::clone(&state.kernel);
    let spawn_result = tokio::task::spawn_blocking(move || kernel.spawn())
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    let mut healthy = spawn_result.is_ok();
    if spawn_result.is_ok() {
        healthy = wait_healthy_blocking(&state).await;
    }
    if healthy {
        let backup_c = backup.clone();
        let backup_removed = tokio::task::spawn_blocking(move || std::fs::remove_file(&backup_c))
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e)));
        let snap_state = state.clone();
        tokio::task::spawn_blocking(move || {
            snap_state.events.publish(render_snapshot(&snap_state));
        });
        let msg = match backup_removed {
            Ok(_) => "管理员密码已重置,服务已重启".to_string(),
            Err(e) => format!(
                "管理员密码已重置,服务已重启;但备份删除失败({e}),残留于 {}",
                backup.display()
            ),
        };
        Json(json!({"ok": true, "success": true, "message": msg})).into_response()
    } else {
        state.kernel.stop().await;
        let backup_c = backup.clone();
        let auth_c = auth.clone();
        let restore = tokio::task::spawn_blocking(move || std::fs::rename(&backup_c, &auth_c))
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e)));
        let respawn = if let Err(error) = &restore {
            state
                .install
                .block_recovery(format!("认证文件恢复失败: {error}"));
            Err("认证文件未恢复,服务保持停止".to_string())
        } else {
            let kernel = Arc::clone(&state.kernel);
            tokio::task::spawn_blocking(move || kernel.spawn())
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
        };
        let respawn_healthy = if respawn.is_ok() {
            wait_healthy_blocking(&state).await
        } else {
            false
        };
        let snap_state = state.clone();
        tokio::task::spawn_blocking(move || {
            snap_state.events.publish(render_snapshot(&snap_state));
        });
        let detail = match spawn_result {
            Err(e) => format!("服务重启失败: {e}"),
            Ok(_) => "服务重启后健康检查未通过".to_string(),
        };
        let restore_msg = match (&restore, &respawn, respawn_healthy) {
            (Err(e), _, _) => {
                format!("认证文件恢复失败({e}),备份保留在 {}", backup.display())
            }
            (_, Err(e), _) => format!("已恢复原认证文件,但旧实例重启失败: {e},请手动启动服务"),
            (_, _, false) => "已恢复原认证文件,但旧实例健康检查未通过,请检查内核日志".to_string(),
            (_, _, true) => "已恢复原认证文件".to_string(),
        };
        err_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{detail};{restore_msg}"),
        )
    }
}

/// 阻塞式健康检查(10s 上限);spawn_blocking 包裹避免占死 IO 线程
async fn wait_healthy_blocking(state: &AppState) -> bool {
    let kernel = Arc::clone(&state.kernel);
    tokio::task::spawn_blocking(move || kernel.wait_healthy(std::time::Duration::from_secs(10)))
        .await
        .unwrap_or(false)
}

#[derive(serde::Deserialize)]
struct PortReq {
    port: u16,
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
        Ok(()) => Json(json!({"success": true, "message": "下载源已保存"})).into_response(),
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
    Json(json!({"success": true, "port": manager.port().await})).into_response()
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
        return err_json(StatusCode::BAD_REQUEST, "端口已被内核或管理器使用");
    }
    let Some(manager) = state.web_proxy.clone() else {
        return err_json(StatusCode::NOT_FOUND, "Web 反代未启用");
    };
    match manager.reconfigure(request.port).await {
        Ok(port) => {
            let snap_state = state.clone();
            tokio::task::spawn_blocking(move || {
                snap_state.events.publish(render_snapshot(&snap_state));
            });
            Json(json!({"success": true, "port": port, "message": "Web 反代端口已切换"}))
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
    let outcome = crate::runtime_settings::apply(&state.core, patch).await;
    if outcome.notify_observers {
        *state
            .auth_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = AuthCache::default();
        let snapshot_state = state.clone();
        let _ = tokio::task::spawn_blocking(move || {
            snapshot_state
                .events
                .publish(render_snapshot(&snapshot_state));
        })
        .await;
    }
    match outcome.result {
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

    #[tokio::test]
    async fn settings_handler_publishes_snapshots_after_success_and_rollback() {
        let dir = TestDir::new("settings-observers");
        let state = dir.state(crate::api::test_support::free_port());
        std::fs::write(
            &state.ui_config_file,
            json!({".url": {"qwenpaw.main": {"port": state.kernel.port().to_string()}}})
                .to_string(),
        )
        .unwrap();

        let mut events = state.events.subscribe();
        let request = Request::builder()
            .method("POST")
            .uri("/settings")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::from(
                json!({"log_level": "debug"}).to_string(),
            ))
            .unwrap();
        let response = settings(State(state.clone()), request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot: serde_json::Value =
            serde_json::from_str(&events.try_recv().unwrap()).unwrap();
        assert_eq!(snapshot["success"], true);

        std::fs::write(&state.ui_config_file, "invalid json").unwrap();
        let mut events = state.events.subscribe();
        let request = Request::builder()
            .method("POST")
            .uri("/settings")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::from(
                json!({"log_level": "warning"}).to_string(),
            ))
            .unwrap();
        let response = settings(State(state.clone()), request).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let snapshot: serde_json::Value =
            serde_json::from_str(&events.try_recv().unwrap()).unwrap();
        assert_eq!(snapshot["success"], true);
        assert_eq!(
            std::fs::read_to_string(dir.0.join("var/qwenpaw-log-level")).unwrap(),
            "debug\n"
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
        let python = install_python(
            &dir,
            &state,
            r#"import time
port = int(sys.argv[sys.argv.index('--port') + 1])
(root / ('ready-' + str(port))).write_text('ready')
time.sleep(3600)
"#,
        );
        let old = ChildGuard(
            std::process::Command::new(&python)
                .args(["-m", "qwenpaw", "app", "--port", &old_port.to_string()])
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
        let candidates = crate::procfs::rediscover_all(old_port, 0);
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
            std::fs::canonicalize("/usr/bin/python3").unwrap()
        );
        let cmdline = std::fs::read(format!("/proc/{}/cmdline", old.0.id())).unwrap();
        let argv: Vec<_> = cmdline
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(String::from_utf8_lossy)
            .collect();
        assert!(argv.iter().any(|arg| arg == &python.to_string_lossy()));
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
    async fn auth_restore_failure_does_not_restart_without_original_auth() {
        let dir = TestDir::new("auth-restore");
        let port = crate::api::test_support::free_port();
        let state = dir.state(port);
        let _kernel = KernelGuard(Arc::clone(&state.kernel));
        crate::api::test_support::release_port_reservation(port);
        let secret = dir.0.join(".qwenpaw.secret");
        std::fs::create_dir(&secret).unwrap();
        let auth = secret.join("auth.json");
        std::fs::write(&auth, "original-auth").unwrap();
        install_python(
            &dir,
            &state,
            r#"with (root / 'starts').open('a') as calls:
    calls.write('start\n')
auth = root / '.qwenpaw.secret/auth.json'
auth.mkdir(exist_ok=True)
(auth / 'block-restore').write_text('keep')
sys.exit(1)
"#,
        );
        let req = Request::builder()
            .method("POST")
            .uri("/kernel/reset-auth")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::empty())
            .unwrap();

        let response = kernel_reset_auth(State(state.clone()), req).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(payload["error"]
            .as_str()
            .unwrap()
            .contains("认证文件恢复失败"));
        assert_eq!(
            std::fs::read_to_string(auth.with_extension("json.bak-reset")).unwrap(),
            "original-auth"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("starts"))
                .unwrap()
                .lines()
                .count(),
            1,
            "restore failure must not start a service without the original auth"
        );
        assert!(
            state.install.recovery_error().is_some(),
            "future starts must remain blocked until auth is restored"
        );
    }
}
