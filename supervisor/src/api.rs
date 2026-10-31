//! 路由与处理器。鉴权:X-Supervisor-Token 头(vs VAR_DIR/supervisor-token,0600)。
//! 监督者只绑 127.0.0.1;控制台经 cgi 桥接访问(App 切后台不影响本服务)。
//! SSE 依据 dsh 生产配方:X-Accel-Buffering: no(axum KeepAlive)+ 首帧全量快照 +
//! 订阅 broadcast;轮询由 /status 承担(HA 双轨)。

use std::sync::Arc;

use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::{convert::Infallible, pin::Pin};
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};

use crate::{events::Events, install::{InstallState, Phase}, kernel::KernelManager};

#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<KernelManager>,
    pub install: InstallState,
    pub events: Events,
    pub token: String,
    pub var_dir: String,
    pub home_dir: String,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/start", post(start))
        .route("/stop", post(stop))
        .route("/restart", post(restart))
        .route("/events", get(events))
        .route("/kernel/install", post(kernel_install))
        .route("/kernel/versions", get(kernel_versions))
        .route("/kernel/pin", post(kernel_pin))
        .with_state(state)
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
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn err_json(code: StatusCode, msg: &str) -> axum::response::Response {
    (code, Json(json!({"ok": false, "error": msg}))).into_response()
}

/// 全量快照(同步;/proc 与版本查询均为阻塞操作,量级可接受)。
pub fn render_snapshot(state: &AppState) -> String {
    let st = state.kernel.status_now();
    let version = state.kernel.kernel_version();
    let inst = state.install.snapshot();
    json!({
        "running": st.running,
        "pid": st.pid,
        "adopted": st.adopted,
        "version": version,
        "installed": !version.is_empty(),
        "cpu": st.cpu_percent,
        "memKB": st.rss_kb,
        "port": st.port,
        "install": {
            "phase": format!("{:?}", inst.phase).to_lowercase(),
            "fetched": inst.fetched,
            "message": inst.message,
            "error": inst.error,
            "seq": inst.seq,
            "running": inst.running,
        },
    })
    .to_string()
}

async fn status(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    Json(serde_json::from_str(&render_snapshot(&state)).unwrap_or(json!({"ok": false})))
        .into_response()
}

async fn start(State(state): State<AppState>, req: Request<axum::body::Body>) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    if !state.kernel.installed() {
        return err_json(StatusCode::CONFLICT, "QwenPaw 内核尚未安装,请在「版本更新」中选择版本安装");
    }
    match state.kernel.spawn() {
        Ok(pid) => {
            state.events.publish(render_snapshot(&state));
            Json(json!({"ok": true, "message": "QwenPaw 已启动", "pid": pid})).into_response()
        }
        Err(e) if e.contains("already running") => {
            Json(json!({"ok": true, "message": "QwenPaw 已在运行"})).into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn stop(State(state): State<AppState>, req: Request<axum::body::Body>) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    state.kernel.stop().await;
    state.events.publish(render_snapshot(&state));
    Json(json!({"ok": true, "message": "QwenPaw 已停止"})).into_response()
}

async fn restart(State(state): State<AppState>, req: Request<axum::body::Body>) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    state.kernel.stop().await;
    match state.kernel.spawn() {
        Ok(pid) => {
            state.events.publish(render_snapshot(&state));
            Json(json!({"ok": true, "message": "QwenPaw 已重启", "pid": pid})).into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn events(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> Sse<Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>> {
    if !check_token(&state.token, &req) {
        let denied: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
            Box::pin(tokio_stream::once(Ok(Event::default().event("snapshot").data(
                json!({"ok": false, "error": "invalid token"}).to_string(),
            ))));
        return Sse::new(denied).keep_alive(KeepAlive::default());
    }
    let first = render_snapshot(&state);
    let rx = state.events.subscribe();
    let snap_stream = BroadcastStream::new(rx).filter_map(|m| {
        m.ok().map(|json| Ok(Event::default().event("snapshot").data(json)))
    });
    let first_stream = tokio_stream::once(Ok(Event::default().event("snapshot").data(first)));
    let merged: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(tokio_stream::StreamExt::chain(first_stream, snap_stream));
    Sse::new(merged).keep_alive(KeepAlive::default())
}

#[derive(serde::Deserialize)]
struct InstallReq {
    version: String,
    #[serde(default)]
    mirror: String,
}

async fn kernel_install(
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
    let spec = match req.version.as_str() {
        "latest" => "qwenpaw".to_string(),
        "beta" | "pre" => "--pre qwenpaw".to_string(),
        v => format!("qwenpaw=={v}"),
    };
    let mirror = match req.mirror.as_str() {
        "tsinghua" => "https://pypi.tuna.tsinghua.edu.cn/simple",
        "aliyun" => "https://mirrors.aliyun.com/pypi/simple/",
        "ustc" => "https://pypi.mirrors.ustc.edu.cn/simple/",
        _ => "",
    };
    // 安装期间先停内核(pip 换包需停服);装完由控制台触发启动
    state.kernel.stop().await;
    let started = state.install.try_start(
        format!("{}/venv", state.home_dir),
        spec,
        mirror.to_string(),
        format!("{}/qwenpaw-upgrade", state.var_dir),
    );
    if started {
        Json(json!({"ok": true, "message": "安装已开始"})).into_response()
    } else {
        err_json(StatusCode::CONFLICT, "安装正在进行中")
    }
}

async fn kernel_versions(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let probe = "\
import urllib.request, json
req = urllib.request.Request('https://pypi.org/pypi/qwenpaw/json', headers={'User-Agent': 'QwenPaw'})
data = json.loads(urllib.request.urlopen(req, timeout=5).read())
releases = sorted(data.get('releases', {}).keys())
print(json.dumps({'releases': releases[-14:], 'latest_stable': data['info']['version']}))";
    match std::process::Command::new("python3").arg("-c").arg(probe).output() {
        Ok(o) if o.status.success() => {
            let payload: serde_json::Value =
                serde_json::from_slice(&o.stdout).unwrap_or(json!({"ok": false}));
            Json(payload).into_response()
        }
        _ => err_json(StatusCode::BAD_GATEWAY, "查询 PyPI 失败"),
    }
}

async fn kernel_pin(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
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
    let pin_path = format!("{}/qwenpaw-version", state.var_dir);
    match std::fs::write(&pin_path, format!("{}\n", req.version)) {
        Ok(_) => Json(json!({"ok": true, "message": format!("版本钉已更新: {}", req.version)}))
            .into_response(),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}
