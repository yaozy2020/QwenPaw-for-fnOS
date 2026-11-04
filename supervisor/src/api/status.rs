use std::convert::Infallible;

use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use serde_json::json;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};

use super::{check_token, err_json, AppState};
use crate::install::phase_wire;

#[derive(Default)]
pub struct AuthCache {
    at: Option<std::time::Instant>,
    status: String,
}

fn auth_status(state: &AppState, running: bool, port: u16) -> String {
    {
        let cache = state.auth_cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache
            .at
            .map(|at| at.elapsed() < std::time::Duration::from_secs(5))
            .unwrap_or(false)
        {
            return cache.status.clone();
        }
    }
    let mut status = "unknown";
    if running && port > 0 {
        use std::io::{Read, Write};
        if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            std::time::Duration::from_millis(300),
        ) {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(300)));
            let req = format!("GET /api/auth/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = Vec::new();
                if stream.read_to_end(&mut buf).is_ok() {
                    let response = String::from_utf8_lossy(&buf);
                    if let Some(body_idx) = response.find("\r\n\r\n") {
                        if let Ok(value) =
                            serde_json::from_str::<serde_json::Value>(&response[body_idx + 4..])
                        {
                            let enabled = value
                                .get("enabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let has_users = value
                                .get("has_users")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            status = match (enabled, has_users) {
                                (true, false) => "uninitialized",
                                (true, true) => "protected",
                                _ => "disabled",
                            };
                        }
                    }
                }
            }
        }
    }
    let mut cache = state.auth_cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.at = Some(std::time::Instant::now());
    cache.status = status.into();
    cache.status.clone()
}

/// 全量快照由 HTTP 处理器放入 spawn_blocking;/proc 和探测不占用 Tokio 工作线程。
/// 形状与旧 api.cgi 兼容:控制台零改动。
pub fn render_snapshot(state: &AppState) -> String {
    let st = state.kernel.status_now();
    let version = state.kernel.kernel_version();
    let inst = state.install.snapshot();
    let auth_state = auth_status(state, st.running, st.port);
    let auth = auth_state == "protected";
    let start_epoch = st
        .started_at
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        })
        .unwrap_or(0);
    json!({
        "success": true,
        "running": st.running,
        "pid": st.pid.to_string(),
        "startAt": start_epoch,
        "version": version,
        "installed": !version.is_empty(),
        // 内核由 /proc 重新收养(监督者曾被 SIGKILL)时为 true,排障用
        "adopted": st.adopted,
        "authEnabled": auth,
        "authStatus": auth_state,
        "port": st.port,
        "cpu": st.cpu_percent,
        "memKB": st.rss_kb,
        "install": {
            // phase_wire:取消(Cancelled)在线上归入 failed,控制台协议不扩新终态
            "phase": phase_wire(&inst.phase),
            "fetched": inst.fetched,
            "message": inst.message,
            "error": inst.error,
            "seq": inst.seq,
            "instance": super::instance_id(),
            "running": inst.running,
            "recovery_required": inst.recovery_required,
            "recovery_error": inst.recovery_error,
        },
    })
    .to_string()
}

pub(super) async fn status(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let snapshot = tokio::task::spawn_blocking(move || render_snapshot(&state))
        .await
        .unwrap_or_else(|_| json!({"ok": false}).to_string());
    // 快照已是合法 JSON 字符串:直返,不再反序列化转一遍(纯浪费)
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        snapshot,
    )
        .into_response()
}

/// SSE:首帧全量快照 + 状态变更推送;KeepAlive 由 axum 承载(dsh S1 配方)。
pub(super) async fn events(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        // 鉴权失败必须返回 401,不能返 200 + 一条错误事件:
        // 200 会让调用方(含扫描器、探测脚本)认为端点无需鉴权,
        // 且 EventSource 层面无法区分"连上了但被拒"与正常流。
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let snapshot_state = state.clone();
    let first = tokio::task::spawn_blocking(move || render_snapshot(&snapshot_state))
        .await
        .unwrap_or_else(|_| json!({"ok": false}).to_string());
    let rx = state.events.subscribe();
    let snap_stream = BroadcastStream::new(rx).filter_map(|m| {
        m.ok()
            .map(|json| Ok(Event::default().event("snapshot").data(json)))
    });
    let first_stream = tokio_stream::once(Ok(Event::default().event("snapshot").data(first)));
    let merged: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(tokio_stream::StreamExt::chain(first_stream, snap_stream));
    Sse::new(merged)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;

    #[test]
    fn render_snapshot_does_not_modify_ui_config() {
        let dir = TestDir::new("snapshot");
        let state = dir.state(crate::api::test_support::free_port());
        let original = r#"{".url":{"qwenpaw.console":{"type":"iframe"},"qwenpaw.main":{"port":"18088","noDisplay":false}},"custom":"keep"}"#;
        std::fs::write(&state.ui_config_file, original).unwrap();
        let modified = std::fs::metadata(&state.ui_config_file)
            .unwrap()
            .modified()
            .unwrap();

        let snapshot: serde_json::Value = serde_json::from_str(&render_snapshot(&state)).unwrap();

        assert_eq!(snapshot["success"], true);
        assert_eq!(snapshot["installed"], false);
        assert_eq!(snapshot["port"], state.kernel.port());
        assert_eq!(
            std::fs::read_to_string(&state.ui_config_file).unwrap(),
            original
        );
        assert_eq!(
            std::fs::metadata(&state.ui_config_file)
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
    }
}
