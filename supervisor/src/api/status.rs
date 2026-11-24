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
use tokio_stream::{wrappers::WatchStream, Stream, StreamExt};

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

fn parse_auth_response(response: &[u8]) -> Option<&'static str> {
    let response = std::str::from_utf8(response).ok()?;
    let (headers, body) = response.split_once("\r\n\r\n")?;
    let mut status_line = headers.lines().next()?.split_whitespace();
    if !matches!(status_line.next()?, "HTTP/1.0" | "HTTP/1.1") {
        return None;
    }
    if !StatusCode::from_bytes(status_line.next()?.as_bytes())
        .ok()?
        .is_success()
    {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let enabled = value.get("enabled")?.as_bool()?;
    let has_users = value.get("has_users")?.as_bool()?;
    Some(match (enabled, has_users) {
        (true, false) => "uninitialized",
        (true, true) => "protected",
        (false, _) => "disabled",
    })
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
                    if let Some(probed) = parse_auth_response(&buf) {
                        status = probed;
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

/// SSE sends a fresh full snapshot for pending invalidations, not a history of changes.
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
    // Subscribe before the first read so a change during rendering stays pending.
    let rx = state.events.subscribe();
    let first = render_snapshot_blocking(state.clone()).await;
    let log_path = (req.uri().query() == Some("watch=logs")).then(|| state.kernel_log.clone());
    let metrics = (req.uri().query() == Some("watch=metrics")).then(|| {
        let kernel = state.kernel.clone();
        state.metrics.subscribe(move || kernel.metrics_now())
    });
    let updates_state = state;
    let snap_stream = WatchStream::from_changes(rx).then(move |()| {
        let state = updates_state.clone();
        async move {
            let snapshot = render_snapshot_blocking(state).await;
            Ok(Event::default().event("snapshot").data(snapshot))
        }
    });
    let first_stream = tokio_stream::once(Ok(Event::default().event("snapshot").data(first)));
    let updates: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        if let Some(path) = log_path {
            Box::pin(snap_stream.merge(log_changes(path, std::time::Duration::from_secs(2))))
        } else if let Some(metrics) = metrics {
            let samples = metrics.map(|(pid, start, cpu, mem)| {
                let start = start
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());
                Ok(Event::default()
                    .event("metrics")
                    .data(json!({"pid":pid,"startAt":start,"cpu":cpu,"memKB":mem}).to_string()))
            });
            Box::pin(snap_stream.merge(samples))
        } else {
            Box::pin(snap_stream)
        };
    Sse::new(first_stream.chain(updates))
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
    async fn settings_handler_refreshes_auth_display_and_events_only_when_needed() {
        let dir = TestDir::new("settings-observers");
        let state = dir.state(crate::api::test_support::free_port());
        let ui = json!({".url": {"qwenpaw.main": {"port": state.kernel.port().to_string()}}});
        std::fs::write(&state.ui_config_file, ui.to_string()).unwrap();

        for (body, expected_status, refresh) in [
            ("{}", StatusCode::OK, false),
            (r#"{"log_level":"verbose"}"#, StatusCode::BAD_REQUEST, false),
            (r#"{"log_level":"debug"}"#, StatusCode::OK, true),
            (r#"{"log_level":"debug"}"#, StatusCode::OK, true),
            (
                r#"{"log_level":"warning"}"#,
                StatusCode::INTERNAL_SERVER_ERROR,
                true,
            ),
        ] {
            if expected_status == StatusCode::INTERNAL_SERVER_ERROR {
                std::fs::write(&state.ui_config_file, "invalid json").unwrap();
            }
            let now = std::time::Instant::now();
            *state.auth_cache.lock().unwrap() = AuthCache {
                at: Some(now),
                status: "protected".into(),
            };
            let events = state.events.subscribe();
            let request = Request::post("/settings")
                .header("X-Supervisor-Token", &state.token)
                .body(axum::body::Body::from(body))
                .unwrap();

            let response = crate::api::settings::settings(State(state.clone()), request).await;

            assert_eq!(response.status(), expected_status, "{body}");
            assert_eq!(events.has_changed().unwrap(), refresh, "{body}");
            let cache = state.auth_cache.lock().unwrap();
            assert_eq!(cache.at, (!refresh).then_some(now), "{body}");
            assert_eq!(cache.status, if refresh { "" } else { "protected" });
        }
        assert_eq!(
            std::fs::read_to_string(dir.0.join("var/qwenpaw-log-level")).unwrap(),
            "debug\n"
        );
    }

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
        // Publish one metadata transition; create + write can be sampled separately.
        let ready = dir.0.join("sample.log");
        std::fs::write(&ready, "sample\n").unwrap();
        std::fs::rename(ready, &file).unwrap();
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

            state.install.block_recovery("during first snapshot".into());
            state.events.invalidate();
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
            assert_eq!(
                snapshot_from_frame(&update)["install"]["recovery_error"],
                "during first snapshot"
            );
            blocker.await.unwrap();
        });
    }

    #[tokio::test]
    async fn events_coalesce_pending_invalidations_into_one_fresh_snapshot() {
        use std::time::Duration;

        let dir = TestDir::new("events-coalesce");
        let state = dir.state(crate::api::test_support::free_port());
        let response = events(State(state.clone()), event_request(&state)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let deadline = Duration::from_secs(2);
        let first = tokio::time::timeout(deadline, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot_from_frame(&first)["install"]["recovery_required"],
            false
        );

        for index in 0..17 {
            state
                .install
                .block_recovery(format!("recovery change {index}"));
            state.events.invalidate();
        }
        let update = tokio::time::timeout(deadline, stream.next())
            .await
            .expect("pending changes must cause a current full snapshot")
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot_from_frame(&update)["install"]["recovery_error"],
            "recovery change 16"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(250), stream.next())
                .await
                .is_err(),
            "pending notifications must not replay redundant snapshots"
        );

        state.install.block_recovery("later change".into());
        state.events.invalidate();
        let next = tokio::time::timeout(deadline, stream.next())
            .await
            .expect("a new change must wake the stream again")
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot_from_frame(&next)["install"]["recovery_error"],
            "later change"
        );
    }

    fn probe_response(response: String, delay: std::time::Duration) -> String {
        use std::io::{BufRead, Write};

        let dir = TestDir::new("auth-response");
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = dir.state(port);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            assert_eq!(request, "GET /api/auth/status HTTP/1.1\r\n");
            loop {
                let mut header = String::new();
                assert_ne!(reader.read_line(&mut header).unwrap(), 0);
                if header == "\r\n" {
                    break;
                }
            }
            std::thread::sleep(delay);
            let _ = stream.write_all(response.as_bytes());
        });
        let status = auth_status(&state, true, port);
        server.join().unwrap();
        status
    }

    fn probe_json(status_line: &str, body: &str) -> String {
        probe_response(
            format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
            std::time::Duration::ZERO,
        )
    }

    #[test]
    fn auth_probe_rejects_unsuccessful_http_responses() {
        for status in [
            "HTTP/1.1 301 Moved Permanently",
            "HTTP/1.1 401 Unauthorized",
            "HTTP/1.1 403 Forbidden",
            "HTTP/1.1 404 Not Found",
            "HTTP/1.1 405 Method Not Allowed",
            "HTTP/1.1 500 Internal Server Error",
            "INVALID 200 OK",
        ] {
            assert_eq!(
                probe_json(status, r#"{"enabled":false,"has_users":false}"#),
                "unknown",
                "{status}"
            );
        }
    }

    #[test]
    fn auth_probe_requires_both_boolean_fields() {
        for body in [
            "{}",
            r#"{"enabled":false}"#,
            r#"{"has_users":false}"#,
            r#"{"enabled":true}"#,
            r#"{"enabled":"false","has_users":false}"#,
            r#"{"enabled":false,"has_users":0}"#,
            r#"{"enabled":null,"has_users":false}"#,
            r#"{"enabled":true,"has_users":null}"#,
            "[]",
            "not JSON",
        ] {
            assert_eq!(probe_json("HTTP/1.1 200 OK", body), "unknown", "{body}");
        }
    }

    #[test]
    fn auth_probe_preserves_valid_auth_states() {
        for (body, expected) in [
            (r#"{"enabled":false,"has_users":false}"#, "disabled"),
            (r#"{"enabled":false,"has_users":true}"#, "disabled"),
            (r#"{"enabled":true,"has_users":false}"#, "uninitialized"),
            (r#"{"enabled":true,"has_users":true}"#, "protected"),
        ] {
            assert_eq!(probe_json("HTTP/1.1 200 OK", body), expected, "{body}");
        }
    }

    #[test]
    fn auth_probe_timeout_and_unavailable_kernel_are_unknown() {
        assert_eq!(
            probe_response(
                "HTTP/1.1 200 OK\r\n\r\n{\"enabled\":false,\"has_users\":false}".into(),
                std::time::Duration::from_millis(600),
            ),
            "unknown"
        );
        let dir = TestDir::new("auth-unavailable");
        let port = crate::api::test_support::free_port();
        let state = dir.state(port);
        assert_eq!(auth_status(&state, true, port), "unknown");
        *state.auth_cache.lock().unwrap() = AuthCache::default();
        assert_eq!(auth_status(&state, false, port), "unknown");
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
