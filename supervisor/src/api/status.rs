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

fn auth_probe_path(state: &AppState) -> &'static str {
    if state.kernel.base_path().is_empty() {
        "/api/auth/status"
    } else {
        "/app/qwenpaw/web/api/auth/status"
    }
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
            let req = format!(
                "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
                auth_probe_path(state)
            );
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
/// 快照形状供桌面控制台和 Gateway API 使用。
pub fn render_snapshot(state: &AppState) -> String {
    let st = state.kernel.status_now();
    let installed = state.kernel.installed();
    let version = if installed {
        let events = state.events.clone();
        state.kernel.display_version(move || events.invalidate())
    } else {
        String::new()
    };
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
        "installed": installed,
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

async fn render_snapshot_blocking(state: AppState) -> String {
    tokio::task::spawn_blocking(move || render_snapshot(&state))
        .await
        .unwrap_or_else(|_| json!({"ok": false}).to_string())
}

pub(super) async fn status(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let snapshot = render_snapshot_blocking(state).await;
    // 快照已是合法 JSON 字符串:直返,不再反序列化转一遍(纯浪费)
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        snapshot,
    )
        .into_response()
}

/// SSE events are invalidation signals: always emit a fresh full snapshot, including after lag.
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
    let rx = state.events.subscribe();
    let first = render_snapshot_blocking(state.clone()).await;
    let log_path = (req.uri().query() == Some("watch=logs")).then(|| state.kernel_log.clone());
    let metrics = (req.uri().query() == Some("watch=metrics")).then(|| state.kernel.clone());
    let updates_state = state;
    let snap_stream = BroadcastStream::new(rx).then(move |_| {
        let state = updates_state.clone();
        async move {
            let snapshot = render_snapshot_blocking(state).await;
            Ok(Event::default().event("snapshot").data(snapshot))
        }
    });
    let first_stream = tokio_stream::once(Ok(Event::default().event("snapshot").data(first)));
    let snapshots = first_stream.chain(snap_stream);
    let merged: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        if let Some(path) = log_path {
            Box::pin(snapshots.merge(log_changes(path, std::time::Duration::from_secs(2))))
        } else if let Some(kernel) = metrics {
            use std::time::Duration;
            let period = Duration::from_secs(1);
            let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let samples = tokio_stream::wrappers::IntervalStream::new(timer)
                .then(move |_| {
                    let kernel = kernel.clone();
                    async move {
                        let (pid, start, cpu, mem) =
                            tokio::task::spawn_blocking(move || kernel.metrics_now())
                                .await
                                .ok()?;
                        let start = start
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs());
                        Some(Ok(Event::default().event("metrics").data(
                            json!({"pid":pid,"startAt":start,"cpu":cpu,"memKB":mem}).to_string(),
                        )))
                    }
                })
                .filter_map(|sample| sample);
            Box::pin(snapshots.merge(samples))
        } else {
            Box::pin(snapshots)
        };
    Sse::new(merged)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// Only an opted-in management stream watches metadata. Log text stays on the bounded /logs route.
fn log_changes(
    path: String,
    period: std::time::Duration,
) -> impl Stream<Item = Result<Event, Infallible>> {
    use std::os::unix::fs::MetadataExt;
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous = None;
    tokio_stream::wrappers::IntervalStream::new(interval)
        .then(move |_| {
            let path = path.clone();
            async move {
                tokio::fs::metadata(path)
                    .await
                    .ok()
                    .map(|m| (m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
            }
        })
        .filter_map(move |next| {
            let changed = previous.replace(next).is_some_and(|old| old != next);
            changed.then(|| Ok(Event::default().event("log-change").data("1")))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;

    #[tokio::test]
    async fn subscribed_log_file_wakes_on_write_and_removal_but_not_idle_ticks() {
        use std::time::Duration;
        let dir = TestDir::new("log-changes");
        let state = dir.state(crate::api::test_support::free_port());
        let file = state.kernel_log.clone();
        let changes = log_changes(file.clone(), Duration::from_millis(10));
        tokio::pin!(changes);
        let idle = Duration::from_millis(80);
        assert!(tokio::time::timeout(idle, changes.next()).await.is_err());
        std::fs::write(&file, "sample\n").unwrap();
        let _change = tokio::time::timeout(Duration::from_secs(2), changes.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(tokio::time::timeout(idle, changes.next()).await.is_err());
        std::fs::remove_file(file).unwrap();
        let _change = tokio::time::timeout(Duration::from_secs(2), changes.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

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

    fn event_request(state: &AppState) -> Request<axum::body::Body> {
        Request::builder()
            .method("GET")
            .header("X-Supervisor-Token", &state.token)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn snapshot_from_frame(frame: &[u8]) -> serde_json::Value {
        let frame = std::str::from_utf8(frame).unwrap();
        let data = frame
            .lines()
            .find_map(|line| line.strip_prefix("data:"))
            .expect("SSE frame must contain data")
            .trim();
        serde_json::from_str(data).unwrap()
    }

    #[test]
    fn events_subscribes_before_rendering_the_initial_snapshot() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            use std::future::Future;

            let dir = TestDir::new("events-initial-subscribe");
            let state = dir.state(crate::api::test_support::free_port());
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            started_rx.await.unwrap();

            let mut response_future = Box::pin(events(State(state.clone()), event_request(&state)));
            std::future::poll_fn(|context| match response_future.as_mut().poll(context) {
                std::task::Poll::Pending => std::task::Poll::Ready(()),
                std::task::Poll::Ready(_) => panic!("snapshot should wait for the blocking worker"),
            })
            .await;

            state
                .events
                .publish(r#"{"success":false,"marker":"during-first-snapshot"}"#.into());
            release_tx.send(()).unwrap();
            let response = response_future.await;
            assert_eq!(response.status(), StatusCode::OK);
            let mut stream = response.into_body().into_data_stream();
            let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(snapshot_from_frame(&first)["success"], true);
            let update = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .expect("event published during first snapshot was lost")
                .unwrap()
                .unwrap();
            let update = snapshot_from_frame(&update);
            assert_eq!(update["success"], true);
            assert!(update.get("marker").is_none());
            blocker.await.unwrap();
        });
    }

    #[tokio::test]
    async fn events_emit_a_fresh_snapshot_after_broadcast_lag() {
        let dir = TestDir::new("events-broadcast-lag");
        let state = dir.state(crate::api::test_support::free_port());
        let response = events(State(state.clone()), event_request(&state)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(snapshot_from_frame(&first)["success"], true);

        for index in 0..17 {
            state
                .events
                .publish(serde_json::json!({"marker": index}).to_string());
        }
        let update = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("lag must cause a current full snapshot")
            .unwrap()
            .unwrap();
        let update = snapshot_from_frame(&update);
        assert_eq!(update["success"], true);
        assert!(update.get("marker").is_none());
    }

    #[test]
    fn auth_probe_uses_the_native_base_path_when_the_patch_is_active() {
        let dir = TestDir::new("auth-native-base-path");
        let state = dir.state(crate::api::test_support::free_port());
        let venv = dir.0.join("home/venv");
        std::fs::create_dir_all(&venv).unwrap();
        assert_eq!(auth_probe_path(&state), "/api/auth/status");
        std::fs::write(venv.join(".qwenpaw-base-path-patch"), "fixture").unwrap();
        assert_eq!(auth_probe_path(&state), "/app/qwenpaw/web/api/auth/status");
    }
}
