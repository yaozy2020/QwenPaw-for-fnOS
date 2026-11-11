use std::sync::Arc;

use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;

use super::{begin_operation, check_token, err_json, render_snapshot, AppState};

pub(super) async fn start(
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
    let kernel = Arc::clone(&state.kernel);
    // installed() 要 stat venv 目录,不能占着 async 线程做
    let installed = tokio::task::spawn_blocking({
        let k = Arc::clone(&kernel);
        move || k.installed()
    })
    .await
    .unwrap_or(false);
    if !installed {
        return err_json(
            StatusCode::CONFLICT,
            "QwenPaw 内核尚未安装,请在「版本更新」中选择版本安装",
        );
    }
    let res = tokio::task::spawn_blocking(move || kernel.spawn())
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(pid) => {
            let snap_state = state.clone();
            tokio::task::spawn_blocking(move || {
                snap_state.events.publish(render_snapshot(&snap_state));
            });
            Json(json!({"ok": true, "success": true, "message": "QwenPaw 已启动", "pid": pid}))
                .into_response()
        }
        Err(e) if e.contains("already running") => {
            Json(json!({"ok": true, "success": true, "message": "QwenPaw 已在运行"}))
                .into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

pub(super) fn stop_verification_error(kernel: &crate::kernel::KernelManager) -> Option<String> {
    kernel.stop_verification_error()
}

pub(super) async fn stop(
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
    state.kernel.stop().await;
    if let Some(error) = stop_verification_error(&state.kernel) {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    let snap_state = state.clone();
    tokio::task::spawn_blocking(move || {
        snap_state.events.publish(render_snapshot(&snap_state));
    });
    Json(json!({"ok": true, "success": true, "message": "QwenPaw 已停止"})).into_response()
}

pub(super) async fn restart(
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
    state.kernel.stop().await;
    if let Some(error) = stop_verification_error(&state.kernel) {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    let kernel = Arc::clone(&state.kernel);
    let res = tokio::task::spawn_blocking(move || kernel.spawn())
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(pid) => {
            let snap_state = state.clone();
            tokio::task::spawn_blocking(move || {
                snap_state.events.publish(render_snapshot(&snap_state));
            });
            Json(json!({"ok": true, "success": true, "message": "QwenPaw 已重启", "pid": pid}))
                .into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;

    fn request(state: &AppState) -> Request<axum::body::Body> {
        Request::builder()
            .method("POST")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn stop_returns_server_error_when_stop_verification_fails() {
        let dir = TestDir::new("api-stop-unclean");
        let state = dir.state(crate::api::test_support::free_port());
        std::fs::create_dir(std::path::Path::new(&state.var_dir).join("kernel.pid")).unwrap();

        let response = stop(State(state.clone()), request(&state)).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(value["success"], false);
        assert!(!state.kernel.last_stop_clean());
    }

    #[tokio::test]
    async fn restart_does_not_spawn_after_unclean_stop() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new("api-restart-unclean");
        let state = dir.state(crate::api::test_support::free_port());
        let spawned = dir.0.join("spawned");
        let python = std::path::Path::new(&state.home_dir).join("venv/bin/python3");
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(
            &python,
            format!("#!/bin/sh\ntouch '{}'\n", spawned.display()),
        )
        .unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let probe = std::process::Command::new(&python)
            .arg("probe")
            .status()
            .unwrap();
        assert!(probe.success());
        assert!(spawned.exists());
        std::fs::remove_file(&spawned).unwrap();
        std::fs::create_dir(std::path::Path::new(&state.var_dir).join("kernel.pid")).unwrap();

        let response = restart(State(state.clone()), request(&state)).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(!state.kernel.last_stop_clean());
        assert!(!spawned.exists(), "restart must not spawn a new kernel");
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(value["success"], false);
        assert!(
            value["message"]
                .as_str()
                .unwrap_or_default()
                .contains("停止核验"),
            "restart must report the failed stop, not a later spawn error: {value}"
        );
    }
}
