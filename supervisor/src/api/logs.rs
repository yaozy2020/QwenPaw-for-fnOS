use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;

use super::{check_token, err_json, AppState};
use crate::install::truncate_utf8;

/// 日志尾部读取:只读文件末尾窗口(默认 256KB),不整文件加载 —— 日志文件可长期
/// 追加到数十 MB,整读曾让 /logs 每次轮询都拖全量 IO。
/// 单行截断到 2000 字符,防超长行撑爆响应。
fn tail_lines(path: &std::path::Path, max: usize, window: u64) -> Vec<String> {
    const LINE_CAP: usize = 2000;
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(window);
    let mut handle = f;
    use std::io::{Read, Seek, SeekFrom};
    if handle.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if handle.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(|l| truncate_utf8(l, LINE_CAP)).collect();
    // 起点切在行中间时,首行是残行;窗口未覆盖全文件才可能发生
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let tail: Vec<String> = lines.into_iter().rev().take(max).collect();
    tail.into_iter().rev().collect()
}

/// 内核运行日志尾部(旧形状:{success, logs: [字符串数组]})。
pub(super) async fn logs(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let tail = req
        .uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("tail="))
                .map(|p| p[5..].parse::<usize>().ok())
        })
        .flatten()
        .unwrap_or(80)
        .clamp(1, 2000);
    let log_path = std::path::PathBuf::from(&state.kernel_log);
    let logs = tokio::task::spawn_blocking(move || tail_lines(&log_path, tail, 256 * 1024))
        .await
        .unwrap_or_default();
    Json(json!({"success": true, "logs": logs})).into_response()
}

pub(super) async fn logs_clear(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    // 截断落盘移入 spawn_blocking:与全库 async 卫生纪律一致(审查 P2-7)
    let log_path = state.kernel_log.clone();
    let _ = tokio::task::spawn_blocking(move || std::fs::write(&log_path, "")).await;
    Json(json!({"success": true, "message": "日志已清理"})).into_response()
}

/// pip 日志尾部(Rust 侧 JSON 转义,替代桥侧 python3;含引号/反斜杠安全)
pub(super) async fn install_log(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let tail = req
        .uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("tail="))
                .map(|p| p[5..].parse::<usize>().ok())
        })
        .flatten()
        .unwrap_or(40)
        .clamp(1, 500);
    let path = std::path::PathBuf::from(format!("{}/qwenpaw-upgrade.log.pip", state.var_dir));
    let lines = tokio::task::spawn_blocking(move || tail_lines(&path, tail, 256 * 1024))
        .await
        .unwrap_or_default();
    Json(json!({"success": true, "logs": lines})).into_response()
}
