//! 路由与处理器。鉴权:X-Supervisor-Token 头(vs VAR_DIR/supervisor-token,0600)。
//! 监督者只绑 127.0.0.1;控制台经 cgi 桥接访问(App 切后台不影响本服务)。
//! 响应形状与旧 api.cgi 兼容(控制台零改动)。

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
    pub etc_dir: String,
    pub share_dir: String,
    pub kernel_log: String,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/start", post(start))
        .route("/stop", post(stop))
        .route("/restart", post(restart))
        .route("/events", get(events))
        .route("/logs", get(logs))
        .route("/logs/clear", post(logs_clear))
        .route("/kernel/install", post(kernel_install))
        .route("/kernel/versions", get(kernel_versions))
        .route("/kernel/pin", post(kernel_pin))
        .route("/kernel/reset-auth", post(kernel_reset_auth))
        .route("/port", post(port))
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
    (code, Json(json!({"ok": false, "success": false, "error": msg, "message": msg}))).into_response()
}

/// 全量快照(同步;/proc 与版本查询均为阻塞操作,量级可接受)。
/// 形状与旧 api.cgi 兼容:控制台零改动。
pub fn render_snapshot(state: &AppState) -> String {
    let st = state.kernel.status_now();
    let version = state.kernel.kernel_version();
    let inst = state.install.snapshot();
    let auth = std::path::Path::new(&state.share_dir)
        .parent()
        .map(|p| p.join(".qwenpaw.secret/auth.json").exists())
        .unwrap_or(false);
    let start_epoch = st
        .started_at
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
        .unwrap_or(0);
    json!({
        "success": true,
        "running": st.running,
        "pid": st.pid.to_string(),
        "startAt": start_epoch,
        "version": version,
        "installed": !version.is_empty(),
        "authEnabled": auth,
        "port": st.port,
        "cpu": st.cpu_percent,
        "memKB": st.rss_kb,
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
            Json(json!({"ok": true, "success": true, "message": "QwenPaw 已启动", "pid": pid}))
                .into_response()
        }
        Err(e) if e.contains("already running") => {
            Json(json!({"ok": true, "success": true, "message": "QwenPaw 已在运行"})).into_response()
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
    Json(json!({"ok": true, "success": true, "message": "QwenPaw 已停止"})).into_response()
}

async fn restart(State(state): State<AppState>, req: Request<axum::body::Body>) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    state.kernel.stop().await;
    match state.kernel.spawn() {
        Ok(pid) => {
            state.events.publish(render_snapshot(&state));
            Json(json!({"ok": true, "success": true, "message": "QwenPaw 已重启", "pid": pid})).into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// SSE:首帧全量快照 + 状态变更推送;KeepAlive 由 axum 承载(dsh S1 配方)。
async fn events(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> Sse<Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>> {
    if !check_token(&state.token, &req) {
        let denied: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(
            tokio_stream::once(Ok(Event::default().event("snapshot").data(
                json!({"ok": false, "error": "invalid token"}).to_string(),
            ))),
        );
        return Sse::new(denied).keep_alive(KeepAlive::default());
    }
    let first = render_snapshot(&state);
    let rx = state.events.subscribe();
    let snap_stream = BroadcastStream::new(rx).filter_map(|m| {
        m.ok().map(|json| Ok(Event::default().event("snapshot").data(json)))
    });
    let first_stream = tokio_stream::once(Ok(Event::default().event("snapshot").data(first)));
    let merged: std::pin::Pin<
        Box<dyn Stream<Item = Result<Event, Infallible>> + Send>,
    > = Box::pin(tokio_stream::StreamExt::chain(first_stream, snap_stream));
    Sse::new(merged).keep_alive(KeepAlive::default())
}

/// 内核运行日志尾部(旧形状:{success, logs: [字符串数组]})。
async fn logs(
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
            q.split('&').find(|p| p.starts_with("tail=")).map(|p| p[5..].parse::<usize>().ok())
        })
        .flatten()
        .unwrap_or(80)
        .clamp(1, 2000);
    let content = std::fs::read_to_string(&state.kernel_log).unwrap_or_default();
    let recent: Vec<String> = content.lines().rev().take(tail).map(|s| s.to_string()).collect();
    let logs: Vec<String> = recent.into_iter().rev().collect();
    Json(json!({"success": true, "logs": logs})).into_response()
}

async fn logs_clear(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let _ = std::fs::write(&state.kernel_log, "");
    Json(json!({"success": true, "message": "日志已清理"})).into_response()
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
    // 桥送来的是完整 URL(pip-mirror 落盘值);短名映射仅兼容旧调用方。
    // 此前完整 URL 落进 _ => "" 被静默变成官方源 —— 用户选源失效(实测发现)。
    // 字符白名单与桥端 set_mirror 一致:mirror 会进 bash -c pip 串,防注入。
    let mirror = if req.mirror.starts_with("http://") || req.mirror.starts_with("https://") {
        if req.mirror.chars().all(|c| c.is_ascii_alphanumeric() || ":/._~-".contains(c)) {
            req.mirror.clone()
        } else {
            String::new()
        }
    } else {
        match req.mirror.as_str() {
            "tsinghua" => "https://pypi.tuna.tsinghua.edu.cn/simple".to_string(),
            "aliyun" => "https://mirrors.aliyun.com/pypi/simple/".to_string(),
            "ustc" => "https://pypi.mirrors.ustc.edu.cn/simple/".to_string(),
            _ => String::new(),
        }
    };
    // 安装期间先停内核(pip 换包需停服);装完由安装协程自动拉起(on_success)
    state.kernel.stop().await;
    let started = state.install.try_start(
        format!("{}/venv", state.home_dir),
        spec,
        mirror.to_string(),
        // 与 app/ui/api.cgi 的 upgrade_logs 读取路径一致 —— 三方文件名曾各不相同,
        // 升级弹窗日志恒为空(独立审计 major,勿再漂移)
        format!("{}/qwenpaw-upgrade.log.pip", state.var_dir),
    );
    if started {
        state.events.publish(render_snapshot(&state));
        Json(json!({"ok": true, "success": true, "message": "安装已开始"})).into_response()
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
    // 全量原始键名交桥接层排序:字典序会截丢 2.10.0 这类版本,语义排序在
    // 桥内 python(真实缩进可测)完成,监督者不做截断(独立审计 minor)。
    let probe = "\
import urllib.request, json
req = urllib.request.Request('https://pypi.org/pypi/qwenpaw/json', headers={'User-Agent': 'QwenPaw'})
data = json.loads(urllib.request.urlopen(req, timeout=5).read())
releases = list(data.get('releases', {}).keys())
print(json.dumps({'releases': releases, 'latest_stable': data['info']['version']}))";
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
        Ok(_) => {
            state.events.publish(render_snapshot(&state));
            Json(json!({"ok": true, "message": format!("版本钉已更新: {}", req.version)}))
                .into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// 重置管理员密码:移除 auth.json,下次打开 Web 端重新设定(旧 reset_auth 语义)。
async fn kernel_reset_auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let auth = std::path::Path::new(&state.share_dir)
        .parent()
        .map(|p| p.join(".qwenpaw.secret/auth.json"))
        .unwrap_or_default();
    match std::fs::remove_file(&auth) {
        Ok(_) => {
            state.kernel.stop().await;
            state.kernel.spawn().unwrap_or(0);
            state.events.publish(render_snapshot(&state));
            Json(json!({"ok": true, "success": true, "message": "管理员密码已重置,服务已重启"}))
                .into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Json(json!({"ok": true, "success": true, "message": "密码本就未设置"})).into_response()
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct PortReq {
    port: u16,
}

/// 改端口:持久化 service_port + 热重启内核到新端口(旧 change_port 语义)。
async fn port(
    State(state): State<AppState>,
    mut req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let body = match axum::body::to_bytes(std::mem::take(req.body_mut()), 4 * 1024).await {
        Ok(b) => b,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let req: PortReq = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    if !(1024..=65535).contains(&req.port) || req.port == 19091 {
        return err_json(StatusCode::BAD_REQUEST, "端口无效(1024~65535,不可用 19091)");
    }
    let _ = std::fs::write(format!("{}/service_port", state.var_dir), format!("{}\n", req.port));
    state.kernel.set_port(req.port);
    state.kernel.stop().await;
    let pid = state.kernel.spawn().unwrap_or(0);
    state.events.publish(render_snapshot(&state));
    Json(json!({
        "ok": true, "success": true,
        "message": format!("端口已更改为 {},服务已重启", req.port),
        "port": req.port,
    }))
    .into_response()
}
