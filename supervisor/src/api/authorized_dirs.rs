use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

const RESULT_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const MAX_RESULTS: usize = 200;
const MAX_STATE_LENGTH: usize = 200;
const MAX_SEARCH_LENGTH: usize = 10000;

struct PendingResult {
    search: String,
    ts: Instant,
}

fn pending_results() -> &'static Mutex<HashMap<String, PendingResult>> {
    static RESULTS: OnceLock<Mutex<HashMap<String, PendingResult>>> = OnceLock::new();
    RESULTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn publish_result(state: &str, search: String) -> Result<(), String> {
    if state.is_empty() || state.len() > MAX_STATE_LENGTH || search.is_empty() || search.len() > MAX_SEARCH_LENGTH {
        return Err("invalid authorization result".into());
    }
    let mut results = pending_results().lock().unwrap_or_else(|error| error.into_inner());
    results.retain(|_, result| result.ts.elapsed() <= RESULT_TTL);
    if results.len() >= MAX_RESULTS {
        let oldest = results
            .iter()
            .min_by_key(|(_, result)| result.ts)
            .map(|(state, _)| state.clone());
        if let Some(oldest) = oldest {
            results.remove(&oldest);
        }
    }
    results.insert(state.into(), PendingResult { search, ts: Instant::now() });
    Ok(())
}

fn take_result(state: &str) -> Option<String> {
    let mut results = pending_results().lock().unwrap_or_else(|error| error.into_inner());
    results.retain(|_, result| result.ts.elapsed() <= RESULT_TTL);
    results.remove(state).map(|result| result.search)
}

use super::{check_token, err_json, AppState};

#[derive(Debug, PartialEq, Eq)]
struct DirectoryHealth {
    path: PathBuf,
    readable: bool,
    writable: bool,
}

fn directory_health(path: &Path) -> Result<DirectoryHealth, String> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::fs::canonicalize(path).map_err(|error| format!("目录不存在或无法解析: {error}"))?;
    let metadata = std::fs::metadata(&path).map_err(|error| format!("无法读取目录状态: {error}"))?;
    if !metadata.is_dir() {
        return Err("授权目标不是目录".into());
    }
    let native = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "目录路径包含 NUL".to_string())?;
    let readable = unsafe { libc::access(native.as_ptr(), libc::R_OK) } == 0;
    let writable = unsafe { libc::access(native.as_ptr(), libc::W_OK) } == 0;
    Ok(DirectoryHealth {
        path,
        readable,
        writable,
    })
}

#[derive(Deserialize)]
struct CheckRequest {
    path: String,
}

pub(super) async fn check(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let body = match axum::body::to_bytes(req.into_body(), 8 * 1024).await {
        Ok(body) => body,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let request: CheckRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    if request.path.is_empty() || request.path.len() > 4096 {
        return err_json(StatusCode::BAD_REQUEST, "目录路径为空或过长");
    }
    let result = tokio::task::spawn_blocking(move || directory_health(Path::new(&request.path))).await;
    match result {
        Ok(Ok(health)) => Json(json!({
            "success": true,
            "path": health.path,
            "readable": health.readable,
            "writable": health.writable,
        }))
        .into_response(),
        Ok(Err(error)) => err_json(StatusCode::UNPROCESSABLE_ENTITY, &error),
        Err(error) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

#[derive(Deserialize)]
struct PublishResultRequest {
    state: String,
    search: String,
}

pub(super) async fn publish_result_http(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let body = match axum::body::to_bytes(req.into_body(), 16 * 1024).await {
        Ok(body) => body,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let request: PublishResultRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    match publish_result(&request.state, request.search) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(error) => err_json(StatusCode::BAD_REQUEST, &error),
    }
}

pub(super) async fn take_result_http(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let query = match axum::extract::Query::<HashMap<String, String>>::try_from_uri(req.uri()) {
        Ok(query) => query,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let state = query.get("state").map(String::as_str).unwrap_or("");
    match take_result(state) {
        Some(search) => Json(json!({"ok": true, "state": state, "search": search})).into_response(),
        None => err_json(StatusCode::NOT_FOUND, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_real_directory_and_reports_access() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-dir-health-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let health = directory_health(&dir).unwrap();
        assert_eq!(health.path, dir);
        assert!(health.readable);
        assert!(health.writable);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn authorization_result_is_read_exactly_once() {
        let state = format!("test-{}", std::process::id());
        publish_result(&state, "?status=success".into()).unwrap();
        assert_eq!(take_result(&state), Some("?status=success".into()));
        assert_eq!(take_result(&state), None);
    }

    #[test]
    fn rejects_missing_paths_and_regular_files() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-dir-health-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        assert!(directory_health(&dir.join("missing")).is_err());
        let file = dir.join("file");
        std::fs::write(&file, b"not a directory").unwrap();
        assert!(directory_health(&file).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
