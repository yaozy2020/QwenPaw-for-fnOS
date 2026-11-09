use std::sync::Arc;

use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;

use super::{
    begin_operation, check_token, err_json, render_snapshot, sync_main_entry, try_operation,
    AppState,
};
use crate::{
    install::{phase_wire, InstallChannel},
    management::{InstallAcceptance, ManagementCore},
};

#[derive(serde::Deserialize)]
struct InstallReq {
    version: String,
    #[serde(default)]
    mirror: String,
}

pub(super) async fn kernel_install(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let body = match axum::body::to_bytes(req.into_body(), 64 * 1024).await {
        Ok(b) => b,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let req: InstallReq = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let (channel, mirror) = match ManagementCore::resolve_install_request(&req.version, &req.mirror)
    {
        Ok(request) => request,
        Err(error) => return super::management_error_response(error),
    };
    let operation = match try_operation(&state.operations) {
        Ok(operation) => operation,
        Err(response) => return *response,
    };
    let acceptance = match state
        .core
        .submit_install_prepared(channel, mirror, operation)
        .await
    {
        Ok(acceptance) => acceptance,
        Err(error) => return super::management_error_response(error),
    };
    match acceptance {
        InstallAcceptance::Started { seq } => {
            let snap_state = state.clone();
            let _ = tokio::task::spawn_blocking(move || {
                snap_state.events.publish(render_snapshot(&snap_state));
            })
            .await;
            Json(json!({"ok": true, "success": true, "message": "安装已开始", "seq": seq, "instance": super::instance_id()}))
                .into_response()
        }
        InstallAcceptance::Busy { seq } => Json(json!({
            "ok": true,
            "success": false,
            "message": "安装正在进行中,请等待完成后再试",
            "seq": seq,
            "instance": super::instance_id(),
            "running": true,
        }))
        .into_response(),
    }
}

fn version_index_url(mirror: &str) -> Result<reqwest::Url, crate::version_index::IndexError> {
    crate::version_index::index_url(mirror)
}

pub(super) async fn kernel_versions(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let mirror = match crate::mirror::read(&state.var_dir) {
        Ok(mirror) => mirror,
        Err(error) => {
            return err_json(
                StatusCode::BAD_GATEWAY,
                &format!("读取 PyPI 镜像配置失败: {error}"),
            )
        }
    };
    let url = match version_index_url(&mirror) {
        Ok(url) => url,
        Err(error) => return err_json(StatusCode::BAD_GATEWAY, &error.to_string()),
    };
    let raw_versions =
        match tokio::task::spawn_blocking(move || crate::version_index::fetch_versions(&url)).await
        {
            Ok(Ok(versions)) => versions,
            Ok(Err(error)) => return err_json(StatusCode::BAD_GATEWAY, &error.to_string()),
            Err(error) => {
                return err_json(
                    StatusCode::BAD_GATEWAY,
                    &format!("查询 PyPI Simple Index 失败: {error}"),
                )
            }
        };
    let k_state = state.kernel.clone();
    let cur_ver = tokio::task::spawn_blocking(move || k_state.kernel_version())
        .await
        .unwrap_or_default();
    Json(crate::version::build_version_list(&raw_versions, &cur_ver)).into_response()
}

/// 安装快照(桥的 upgrade_status 透传,替代每 800ms 一次的 python3 解析)
pub(super) async fn install_status(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let s = state.core.install_snapshot();
    Json(json!({
        "success": true,
        "upgrading": s.running,
        "phase": phase_wire(&s.phase),
        "seq": s.seq,
        "instance": super::instance_id(),
        "recovery_required": s.recovery_required,
        "recovery_error": s.recovery_error,
        "fetched": s.fetched,
        "message": s.message,
        "error": s.error,
    }))
    .into_response()
}

#[derive(Debug)]
enum RemoveKernelFilesError {
    BeforeRemoval(String),
    PartialRemoval(String),
}

fn remove_kernel_files(
    home: &std::path::Path,
    var: &std::path::Path,
) -> Result<Vec<String>, RemoveKernelFilesError> {
    let venv = home.join("venv");
    let pin = var.join("qwenpaw-version");
    if !venv.exists() {
        return match std::fs::remove_file(&pin) {
            Ok(()) => Ok(Vec::new()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(RemoveKernelFilesError::BeforeRemoval(format!(
                "删除版本钉失败: {e}"
            ))),
        };
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let trash = home.join(format!(".venv.uninstall.{}.{unique}", std::process::id()));
    let pin_backup = var.join(format!(
        ".qwenpaw-version.uninstall.{}.{unique}",
        std::process::id()
    ));
    let had_pin = pin.exists();
    if had_pin {
        std::fs::rename(&pin, &pin_backup)
            .map_err(|e| RemoveKernelFilesError::BeforeRemoval(format!("备份版本钉失败: {e}")))?;
    }
    if let Err(e) = std::fs::rename(&venv, &trash) {
        if had_pin {
            if let Err(restore_error) = std::fs::rename(&pin_backup, &pin) {
                return Err(RemoveKernelFilesError::PartialRemoval(format!(
                    "移出内核运行环境失败: {e}; 恢复版本钉失败: {restore_error}; 状态未能恢复且不可回滚"
                )));
            }
        }
        return Err(RemoveKernelFilesError::BeforeRemoval(format!(
            "移出内核运行环境失败: {e}"
        )));
    }
    let mut warnings = Vec::new();
    if let Err(e) = std::fs::remove_dir_all(&trash) {
        warnings.push(format!("运行环境残留于 {}: {e}", trash.display()));
    }
    if had_pin {
        if let Err(e) = std::fs::remove_file(&pin_backup) {
            warnings.push(format!("版本钉备份残留于 {}: {e}", pin_backup.display()));
        }
    }
    if !warnings.is_empty() {
        return Err(RemoveKernelFilesError::PartialRemoval(format!(
            "内核文件已部分删除，不可回滚；{}",
            warnings.join("；")
        )));
    }
    Ok(warnings)
}

pub(super) async fn kernel_uninstall(
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
    if let Some(error) = super::lifecycle::stop_verification_error(&state.kernel) {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    let home = state.home_dir.clone();
    let var = state.var_dir.clone();
    let res = tokio::task::spawn_blocking(move || {
        remove_kernel_files(std::path::Path::new(&home), std::path::Path::new(&var))
    })
    .await
    .unwrap_or_else(|e| {
        Err(RemoveKernelFilesError::PartialRemoval(format!(
            "卸载文件操作异常退出，文件状态未知且不可回滚: {e}"
        )))
    });

    let warnings = match res {
        Ok(warnings) => warnings,
        Err(RemoveKernelFilesError::BeforeRemoval(error)) => {
            let kernel = Arc::clone(&state.kernel);
            let _ = tokio::task::spawn_blocking(move || kernel.spawn()).await;
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, &error);
        }
        Err(RemoveKernelFilesError::PartialRemoval(error)) => {
            state.kernel.invalidate_version();
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, &error);
        }
    };
    state.kernel.invalidate_version();
    // 读+可能写 ui 配置文件:与全库 async 卫生纪律一致,移入 spawn_blocking
    let ui_cfg = state.ui_config_file.clone();
    let entry_result =
        tokio::task::spawn_blocking(move || sync_main_entry(std::path::Path::new(&ui_cfg), false))
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(format!("spawn_blocking join: {e}"))));
    let snap_state = state.clone();
    tokio::task::spawn_blocking(move || {
        snap_state.events.publish(render_snapshot(&snap_state));
    });
    if let Err(e) = entry_result {
        return err_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("内核运行环境与版本钉已删除且不可回滚；桌面入口更新失败: {e}"),
        );
    }
    let mut message = "内核已卸载，用户配置、记忆与管理员认证已保留".to_string();
    if !warnings.is_empty() {
        message.push_str(&format!("；{}", warnings.join("；")));
    }
    Json(json!({"ok": true, "success": true, "message": message})).into_response()
}

pub(super) async fn kernel_pin(
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
        Ok(b) => b,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let req: InstallReq = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    if req.version.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "缺少 version");
    }
    // 版本钉会被安装脚本拼回 `qwenpaw=={pin}`,必须与安装入口同一白名单;
    // 且钉语义是"精确恢复",latest/beta 不是合法钉值(venv 重建会还原成
    // `qwenpaw==latest` 这类坏规格)
    if !matches!(
        InstallChannel::parse(&req.version),
        Some(InstallChannel::Exact(_))
    ) {
        return err_json(StatusCode::BAD_REQUEST, "版本钉仅接受合法的精确版本号");
    }
    let pin_path = format!("{}/qwenpaw-version", state.var_dir);
    let pin_content = format!("{}\n", req.version);
    let wrote = tokio::task::spawn_blocking(move || {
        crate::install::atomic_write_0600(&pin_path, &pin_content)
    })
    .await
    .unwrap_or_else(|e| Err(std::io::Error::other(e)));
    match wrote {
        Ok(_) => {
            let snap_state = state.clone();
            let _ = tokio::task::spawn_blocking(move || {
                snap_state.events.publish(render_snapshot(&snap_state));
            })
            .await;
            Json(json!({"ok": true, "message": format!("版本钉已更新: {}", req.version)}))
                .into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{install_python, TestDir};

    fn install_request(state: &AppState, mirror: &str) -> Request<axum::body::Body> {
        Request::builder()
            .method("POST")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::from(
                json!({"version": "latest", "mirror": mirror}).to_string(),
            ))
            .unwrap()
    }

    fn install_python_with_pip_recorder(dir: &TestDir, state: &AppState) {
        install_python(
            dir,
            state,
            r#"
import json, time
if sys.argv[1:3] == ['-m', 'pip']:
    (root / 'pip-args.json').write_text(json.dumps(sys.argv[3:]))
    (root / 'pip-started').touch()
    while (root / 'block-pip').exists() and not (root / 'release-pip').exists():
        time.sleep(0.01)
    sys.exit(1)
"#,
        );
    }

    async fn wait_for_path(path: &std::path::Path) -> bool {
        // 受理返回后 worker 仍需调度并创建子进程；高负载 CI 中 3 秒不足以覆盖该窗口。
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !path.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        path.exists()
    }

    async fn wait_for_install(state: &AppState) -> bool {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while state.install.is_busy() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        !state.install.is_busy()
    }

    #[tokio::test]
    async fn invalid_mirror_is_rejected_without_install_or_config_side_effects() {
        let dir = TestDir::new("invalid-install-mirror");
        let state = dir.state(crate::api::test_support::free_port());
        install_python_with_pip_recorder(&dir, &state);
        let mirror_file = std::path::Path::new(&state.var_dir).join("pip-mirror");
        let pip_started = dir.0.join("pip-started");
        std::fs::write(dir.0.join("block-pip"), "block").unwrap();

        let response = kernel_install(
            axum::extract::State(state.clone()),
            install_request(&state, "https://mirror.example/simple;bad"),
        )
        .await;
        let status = response.status();
        let snapshot = state.install.snapshot();
        if snapshot.seq != 0 {
            let _ = wait_for_path(&pip_started).await;
        }
        let _ = std::fs::write(dir.0.join("release-pip"), "release");
        let settled = wait_for_install(&state).await;
        if !settled {
            let _ = state.install.shutdown(std::time::Duration::from_secs(3));
        }

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            snapshot.seq, 0,
            "invalid input must not create an install task"
        );
        assert!(!snapshot.running);
        assert!(!pip_started.exists(), "invalid input must not invoke pip");
        assert!(!std::path::Path::new(&state.var_dir)
            .join("qwenpaw-upgrade.log.pip")
            .exists());
        assert!(
            !mirror_file.exists(),
            "invalid input must not write mirror config"
        );
        assert!(settled, "no installation worker should need cleanup");
    }

    #[tokio::test]
    async fn accepted_mirror_is_snapshotted_before_a_later_mirror_change() {
        let dir = TestDir::new("install-mirror-snapshot");
        let state = dir.state(crate::api::test_support::free_port());
        install_python_with_pip_recorder(&dir, &state);
        let pip_started = dir.0.join("pip-started");
        std::fs::write(dir.0.join("block-pip"), "block").unwrap();

        let response = kernel_install(
            axum::extract::State(state.clone()),
            install_request(&state, "https://mirror.example/custom/simple/"),
        )
        .await;
        let status = response.status();
        let started = wait_for_path(&pip_started).await;
        let setter_rejected = state
            .core
            .set_mirror("https://later.example/other/simple")
            .is_err();
        let mirror_store_updated =
            crate::mirror::write(&state.var_dir, "https://later.example/other/simple").is_ok();
        let _ = std::fs::write(dir.0.join("release-pip"), "release");
        let settled = wait_for_install(&state).await;
        if !settled {
            let _ = state.install.shutdown(std::time::Duration::from_secs(3));
        }
        let args = std::fs::read_to_string(dir.0.join("pip-args.json"))
            .ok()
            .and_then(|contents| serde_json::from_str::<Vec<String>>(&contents).ok());
        let mirror_arg = args.as_ref().and_then(|args| {
            args.windows(2)
                .find(|pair| pair[0] == "--index-url")
                .map(|pair| pair[1].clone())
        });

        assert_eq!(status, StatusCode::OK);
        assert!(started, "accepted install should invoke pip");
        assert!(
            setter_rejected,
            "the management setter rejects concurrent changes"
        );
        assert!(
            mirror_store_updated,
            "the persisted mirror can change independently"
        );
        assert_eq!(
            mirror_arg.as_deref(),
            Some("https://mirror.example/custom/simple/")
        );
        assert!(settled, "pip test process should finish after release");
    }

    #[tokio::test]
    async fn empty_mirror_keeps_the_official_pip_index_semantics() {
        let dir = TestDir::new("install-empty-mirror");
        let state = dir.state(crate::api::test_support::free_port());
        install_python_with_pip_recorder(&dir, &state);
        let pip_started = dir.0.join("pip-started");

        let response = kernel_install(
            axum::extract::State(state.clone()),
            install_request(&state, ""),
        )
        .await;
        let status = response.status();
        let started = wait_for_path(&pip_started).await;
        let settled = wait_for_install(&state).await;
        if !settled {
            let _ = state.install.shutdown(std::time::Duration::from_secs(3));
        }
        let args = std::fs::read_to_string(dir.0.join("pip-args.json"))
            .ok()
            .and_then(|contents| serde_json::from_str::<Vec<String>>(&contents).ok());

        assert_eq!(status, StatusCode::OK);
        assert!(started, "accepted install should invoke pip");
        assert!(args.is_some());
        assert!(!args.unwrap().iter().any(|arg| arg == "--index-url"));
        assert!(settled, "pip test process should finish");
    }

    #[test]
    fn version_index_uses_the_selected_mirror_and_rejects_invalid_values() {
        assert_eq!(
            version_index_url("").unwrap().as_str(),
            "https://pypi.org/simple/qwenpaw/"
        );
        assert_eq!(
            version_index_url("https://pypi.tuna.tsinghua.edu.cn/simple")
                .unwrap()
                .as_str(),
            "https://pypi.tuna.tsinghua.edu.cn/simple/qwenpaw/"
        );
        assert_eq!(
            version_index_url("https://mirrors.aliyun.com/pypi/simple/")
                .unwrap()
                .as_str(),
            "https://mirrors.aliyun.com/pypi/simple/qwenpaw/"
        );
        assert!(version_index_url("https://mirrors.aliyun.com/pypi/simple/;bad").is_err());
    }

    #[tokio::test]
    async fn kernel_versions_queries_a_local_simple_index() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /simple/qwenpaw/ HTTP/1.1"));
            let body = "<a href='qwenpaw-2.0.0-py3-none-any.whl'>stable</a><a href='qwenpaw-2.1.0b1.zip'>beta</a>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let dir = TestDir::new("local-version-index");
        let state = dir.state(crate::api::test_support::free_port());
        std::fs::write(
            std::path::Path::new(&state.var_dir).join("pip-mirror"),
            format!("http://{address}/simple\n"),
        )
        .unwrap();
        let request = Request::builder()
            .method("GET")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::empty())
            .unwrap();

        let response = kernel_versions(axum::extract::State(state), request).await;
        server.join().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["latest_stable"], "2.0.0");
        assert_eq!(value["latest_beta"], "2.1.0b1");
    }

    #[tokio::test]
    async fn invalid_persisted_version_mirror_returns_bad_gateway_without_fallback() {
        let dir = TestDir::new("invalid-version-mirror");
        let state = dir.state(crate::api::test_support::free_port());
        std::fs::write(
            std::path::Path::new(&state.var_dir).join("pip-mirror"),
            "https://mirror.example/simple;bad\n",
        )
        .unwrap();
        let request = Request::builder()
            .method("GET")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::empty())
            .unwrap();

        let response = kernel_versions(axum::extract::State(state), request).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value["message"].as_str().unwrap().contains("镜像地址无效"));
    }

    #[test]
    fn kernel_uninstall_removes_only_runtime_and_pin() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-uninstall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        let var = dir.join("var");
        let share = dir.join("share");
        std::fs::create_dir_all(home.join("venv/bin")).unwrap();
        std::fs::create_dir_all(&var).unwrap();
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(home.join("venv/bin/python3"), "runtime").unwrap();
        std::fs::write(var.join("qwenpaw-version"), "2.3.0b2\n").unwrap();
        std::fs::write(share.join("config.json"), "keep").unwrap();
        let warnings = remove_kernel_files(&home, &var).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!home.join("venv").exists());
        assert!(!var.join("qwenpaw-version").exists());
        assert_eq!(
            std::fs::read_to_string(share.join("config.json")).unwrap(),
            "keep"
        );
        assert!(remove_kernel_files(&home, &var).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn uninstall_request(state: &AppState) -> Request<axum::body::Body> {
        Request::builder()
            .method("POST")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn kernel_uninstall_does_not_touch_files_after_unclean_stop() {
        let dir = TestDir::new("uninstall-unclean-stop");
        let state = dir.state(crate::api::test_support::free_port());
        let home = std::path::Path::new(&state.home_dir);
        let var = std::path::Path::new(&state.var_dir);
        std::fs::create_dir_all(home.join("venv")).unwrap();
        std::fs::write(home.join("venv/runtime"), "keep").unwrap();
        std::fs::write(var.join("qwenpaw-version"), "2.3.0\n").unwrap();
        std::fs::create_dir(var.join("kernel.pid")).unwrap();

        let response = kernel_uninstall(State(state.clone()), uninstall_request(&state)).await;
        let status = response.status();
        let value = response_json(response).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(value["success"], false);
        assert_eq!(
            std::fs::read_to_string(home.join("venv/runtime")).unwrap(),
            "keep"
        );
        assert_eq!(
            std::fs::read_to_string(var.join("qwenpaw-version")).unwrap(),
            "2.3.0\n"
        );
    }

    #[tokio::test]
    async fn kernel_uninstall_reports_runtime_removal_residue_without_claiming_rollback() {
        let dir = TestDir::new("uninstall-runtime-residue");
        let state = dir.state(crate::api::test_support::free_port());
        let home = std::path::Path::new(&state.home_dir);
        let var = std::path::Path::new(&state.var_dir);
        std::fs::write(home.join("venv"), "not-a-directory").unwrap();
        std::fs::write(var.join("qwenpaw-version"), "2.3.0\n").unwrap();
        std::fs::write(&state.ui_config_file, r#"{".url":{}}"#).unwrap();

        let response = kernel_uninstall(State(state.clone()), uninstall_request(&state)).await;
        let status = response.status();
        let value = response_json(response).await;
        let message = value["message"].as_str().unwrap_or_default();
        let residue_exists = std::fs::read_dir(home)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".venv.uninstall.")
            });

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(value["success"], false);
        assert!(!home.join("venv").exists());
        assert!(
            residue_exists,
            "failed cleanup must leave a reportable residue"
        );
        assert!(!var.join("qwenpaw-version").exists());
        assert!(message.contains("残留"), "{message}");
        assert!(message.contains("不可回滚"), "{message}");
    }

    #[tokio::test]
    async fn kernel_uninstall_reports_desktop_sync_failure_after_removal() {
        let dir = TestDir::new("uninstall-desktop-sync");
        let state = dir.state(crate::api::test_support::free_port());
        let home = std::path::Path::new(&state.home_dir);
        let var = std::path::Path::new(&state.var_dir);
        std::fs::create_dir_all(home.join("venv/bin")).unwrap();
        std::fs::write(home.join("venv/bin/python3"), "runtime").unwrap();
        std::fs::write(var.join("qwenpaw-version"), "2.3.0\n").unwrap();

        let response = kernel_uninstall(State(state.clone()), uninstall_request(&state)).await;
        let status = response.status();
        let value = response_json(response).await;
        let message = value["message"].as_str().unwrap_or_default();

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(value["success"], false);
        assert!(!home.join("venv").exists());
        assert!(!var.join("qwenpaw-version").exists());
        assert!(message.contains("桌面入口更新失败"), "{message}");
        assert!(message.contains("不可回滚"), "{message}");
    }
}
