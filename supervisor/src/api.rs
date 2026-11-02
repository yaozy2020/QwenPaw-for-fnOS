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

use crate::{
    events::Events,
    install::{phase_wire, sanitize_mirror, truncate_utf8, InstallChannel, InstallState},
    kernel::KernelManager,
};

#[derive(Default)]
pub struct AuthCache {
    at: Option<std::time::Instant>,
    status: String,
}

#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<KernelManager>,
    pub install: InstallState,
    pub events: Events,
    pub token: String,
    pub var_dir: String,
    pub home_dir: String,
    pub share_dir: String,
    pub ui_config_file: String,
    pub kernel_log: String,
    pub operations: Arc<tokio::sync::Mutex<()>>,
    pub auth_cache: Arc<std::sync::Mutex<AuthCache>>,
    /// 监督者自身监听端口:改端口时必须拒绝内核占用它,否则桥接失联
    pub supervisor_port: u16,
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
        .route("/kernel/uninstall", post(kernel_uninstall))
        .route("/kernel/install-status", get(install_status))
        .route("/kernel/install-log", get(install_log))
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

type OperationResult = Result<tokio::sync::OwnedMutexGuard<()>, Box<axum::response::Response>>;

fn try_operation(lock: &Arc<tokio::sync::Mutex<()>>) -> OperationResult {
    lock.clone().try_lock_owned().map_err(|_| {
        Box::new(err_json(
            StatusCode::CONFLICT,
            "另一个运维操作正在进行中,请稍后重试",
        ))
    })
}

fn begin_operation(state: &AppState) -> OperationResult {
    let guard = try_operation(&state.operations)?;
    if state.install.snapshot().running {
        return Err(Box::new(err_json(
            StatusCode::CONFLICT,
            "内核安装正在进行中,当前操作已被拒绝",
        )));
    }
    Ok(guard)
}

fn auth_status(state: &AppState, running: bool, port: u16) -> String {
    {
        let cache = state.auth_cache.lock().unwrap();
        if cache.at.map(|at| at.elapsed() < std::time::Duration::from_secs(5)).unwrap_or(false) {
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
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&response[body_idx + 4..]) {
                            let enabled = value.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                            let has_users = value.get("has_users").and_then(|v| v.as_bool()).unwrap_or(false);
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
    let mut cache = state.auth_cache.lock().unwrap();
    cache.at = Some(std::time::Instant::now());
    cache.status = status.into();
    cache.status.clone()
}

/// 全量快照由 HTTP 处理器放入 spawn_blocking;/proc 和探测不占用 Tokio 工作线程。
/// 形状与旧 api.cgi 兼容:控制台零改动。
pub fn render_snapshot(state: &AppState) -> String {
    let st = state.kernel.status_now();
    let version = state.kernel.kernel_version();
    let _ = sync_main_entry(
        std::path::Path::new(&state.ui_config_file),
        st.port,
        !version.is_empty(),
    );
    let inst = state.install.snapshot();
    let auth_state = auth_status(state, st.running, st.port);
    let auth = auth_state == "protected";
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
    let snapshot = tokio::task::spawn_blocking(move || render_snapshot(&state))
        .await
        .unwrap_or_else(|_| json!({"ok": false}).to_string());
    Json(serde_json::from_str(&snapshot).unwrap_or(json!({"ok": false}))).into_response()
}

async fn start(State(state): State<AppState>, req: Request<axum::body::Body>) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let _operation = match begin_operation(&state) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
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
    let _operation = match begin_operation(&state) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
    state.kernel.stop().await;
    state.events.publish(render_snapshot(&state));
    Json(json!({"ok": true, "success": true, "message": "QwenPaw 已停止"})).into_response()
}

async fn restart(State(state): State<AppState>, req: Request<axum::body::Body>) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let _operation = match begin_operation(&state) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
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
    let snapshot_state = state.clone();
    let first = tokio::task::spawn_blocking(move || render_snapshot(&snapshot_state))
        .await
        .unwrap_or_else(|_| json!({"ok": false}).to_string());
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
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| truncate_utf8(l, LINE_CAP))
        .collect();
    // 起点切在行中间时,首行是残行;窗口未覆盖全文件才可能发生
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let tail: Vec<String> = lines.into_iter().rev().take(max).collect();
    tail.into_iter().rev().collect()
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
    let logs = tail_lines(std::path::Path::new(&state.kernel_log), tail, 256 * 1024);
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
    let _operation = match try_operation(&state.operations) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
    let body = match axum::body::to_bytes(req.into_body(), 64 * 1024).await {
        Ok(b) => b,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let req: InstallReq = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    // 版本 → 渠道枚举;非法值(Shell 元字符/requirement/URL/路径)直接 400,
    // 绝不进入 pip 命令。latest/beta 之外只接受 PEP 440 字符白名单的精确版本。
    let channel = match InstallChannel::parse(&req.version) {
        Some(c) => c,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                "版本号非法:仅接受 latest、beta 或合法的精确版本号",
            )
        }
    };
    // 桥送来的是完整 URL(pip-mirror 落盘值);短名映射仅兼容旧调用方。
    // 此前完整 URL 落进 _ => "" 被静默变成官方源 —— 用户选源失效(实测发现)。
    // 字符白名单兜底直连调用方;合法镜像作为独立 --index-url argv 传递,不经 Shell。
    let mirror = if !req.mirror.is_empty()
        && !(req.mirror.starts_with("http://") || req.mirror.starts_with("https://"))
    {
        match req.mirror.as_str() {
            "tsinghua" => "https://pypi.tuna.tsinghua.edu.cn/simple".to_string(),
            "aliyun" => "https://mirrors.aliyun.com/pypi/simple/".to_string(),
            "ustc" => "https://pypi.mirrors.ustc.edu.cn/simple/".to_string(),
            _ => String::new(),
        }
    } else {
        sanitize_mirror(&req.mirror)
    };
    // 忙检查必须在停内核之前:重入时只拒绝,绝不停掉正在陪跑 pip 的内核
    // (-7 回归:stop 级联修真后,忙路径曾先杀内核再被 try_start 拒绝)。
    // busy 响应携带当前 seq:前端据此识别"这次点击没接到单",不再开转圈弹窗
    if state.install.snapshot().running {
        let cur = state.install.snapshot();
        return Json(json!({
            "ok": true, "success": false,
            "message": "安装正在进行中,请等待完成后再试",
            "seq": cur.seq, "running": true,
        }))
        .into_response();
    }
    // 安装期间先停内核(pip 换包需停服);装完由安装线程自动拉起(on_success)
    state.kernel.stop().await;
    let started = state.install.try_start(
        format!("{}/venv", state.home_dir),
        state.kernel.port(),
        channel,
        mirror,
        // 与 app/ui/api.cgi 的 upgrade_logs 读取路径一致 —— 三方文件名曾各不相同,
        // 升级弹窗日志恒为空(独立审计 major,勿再漂移)
        format!("{}/qwenpaw-upgrade.log.pip", state.var_dir),
        // 版本钉:安装线程核实元数据后原子写入实际版本
        format!("{}/qwenpaw-version", state.var_dir),
    );
    if started {
        let seq = state.install.snapshot().seq;
        state.events.publish(render_snapshot(&state));
        // seq 随受理响应返回:前端由此确认"后端已接单",替代 sendBeacon 的盲发
        Json(json!({"ok": true, "success": true, "message": "安装已开始", "seq": seq}))
            .into_response()
    } else {
        err_json(StatusCode::CONFLICT, "安装正在进行中")
    }
}

fn version_index_url(mirror: &str) -> String {
    let mirror = sanitize_mirror(mirror);
    if mirror.is_empty() {
        "https://pypi.org/simple/qwenpaw/".into()
    } else {
        format!("{}/qwenpaw/", mirror.trim_end_matches('/'))
    }
}

async fn kernel_versions(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let mirror = std::fs::read_to_string(format!("{}/pip-mirror", state.var_dir))
        .unwrap_or_default();
    let url = version_index_url(mirror.trim());
    // PEP 503 Simple Index 与 pip 使用同一镜像;通用 JSON API 并非所有镜像都提供。
    let probe = "\
import urllib.request, urllib.parse, json, sys
from html.parser import HTMLParser
class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.versions = set()
    def handle_starttag(self, tag, attrs):
        if tag != 'a':
            return
        href = dict(attrs).get('href', '')
        name = urllib.parse.unquote(urllib.parse.urlsplit(href).path.rsplit('/', 1)[-1])
        if name.lower().startswith('qwenpaw-') and name.endswith('.whl'):
            parts = name.split('-')
            if len(parts) >= 5:
                self.versions.add(parts[1])
        else:
            for suffix in ('.tar.gz', '.zip'):
                if name.lower().startswith('qwenpaw-') and name.endswith(suffix):
                    self.versions.add(name[9:-len(suffix)])
req = urllib.request.Request(sys.argv[1], headers={'User-Agent': 'QwenPaw'})
parser = Links()
parser.feed(urllib.request.urlopen(req, timeout=15).read().decode('utf-8', 'replace'))
if not parser.versions:
    raise RuntimeError('镜像未返回可用内核版本')
print(json.dumps({'releases': sorted(parser.versions), 'latest_stable': ''}))";
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("python3").arg("-c").arg(probe).arg(url).output()
    })
    .await;
    match output {
        Ok(Ok(o)) if o.status.success() => {
            let payload: serde_json::Value =
                serde_json::from_slice(&o.stdout).unwrap_or(json!({"ok": false}));
            Json(payload).into_response()
        }
        _ => err_json(StatusCode::BAD_GATEWAY, "查询所选 PyPI 镜像失败"),
    }
}

/// 安装快照(桥的 upgrade_status 透传,替代每 800ms 一次的 python3 解析)
async fn install_status(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let s = state.install.snapshot();
    Json(json!({
        "success": true,
        "upgrading": s.running,
        "phase": phase_wire(&s.phase),
        // seq 供弹窗按任务跟踪:触发前取基线,seq 变化 = 本次任务已被受理,
        // 终态 + seq 匹配才算本次结果(修复"首拍已是终态"的漏判)
        "seq": s.seq,
        "fetched": s.fetched,
        "message": s.message,
        "error": s.error,
    }))
    .into_response()
}

/// pip 日志尾部(Rust 侧 JSON 转义,替代桥侧 python3;含引号/反斜杠安全)
async fn install_log(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let tail = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find(|p| p.starts_with("tail=")).map(|p| p[5..].parse::<usize>().ok()))
        .flatten()
        .unwrap_or(40)
        .clamp(1, 500);
    let path = format!("{}/qwenpaw-upgrade.log.pip", state.var_dir);
    let lines = tail_lines(std::path::Path::new(&path), tail, 256 * 1024);
    Json(json!({"success": true, "logs": lines})).into_response()
}

fn remove_kernel_files(home: &std::path::Path, var: &std::path::Path) -> Result<Vec<String>, String> {
    let venv = home.join("venv");
    let pin = var.join("qwenpaw-version");
    if !venv.exists() {
        return match std::fs::remove_file(&pin) {
            Ok(()) => Ok(Vec::new()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("删除版本钉失败: {e}")),
        };
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let trash = home.join(format!(".venv.uninstall.{}.{unique}", std::process::id()));
    let pin_backup = var.join(format!(".qwenpaw-version.uninstall.{}.{unique}", std::process::id()));
    let had_pin = pin.exists();
    if had_pin {
        std::fs::rename(&pin, &pin_backup).map_err(|e| format!("备份版本钉失败: {e}"))?;
    }
    if let Err(e) = std::fs::rename(&venv, &trash) {
        if had_pin {
            let _ = std::fs::rename(&pin_backup, &pin);
        }
        return Err(format!("移出内核运行环境失败: {e}"));
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
    Ok(warnings)
}

async fn kernel_uninstall(
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
    let warnings = match remove_kernel_files(
        std::path::Path::new(&state.home_dir),
        std::path::Path::new(&state.var_dir),
    ) {
        Ok(warnings) => warnings,
        Err(e) => {
            let _ = state.kernel.spawn();
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, &e);
        }
    };
    state.kernel.invalidate_version();
    let entry_result = sync_main_entry(
        std::path::Path::new(&state.ui_config_file),
        state.kernel.port(),
        false,
    );
    state.events.publish(render_snapshot(&state));
    let mut message = "内核已卸载，用户配置、记忆与管理员认证已保留".to_string();
    if !warnings.is_empty() {
        message.push_str(&format!("；{}", warnings.join("；")));
    }
    if let Err(e) = entry_result {
        message.push_str(&format!("；桌面入口更新失败: {e}"));
    }
    Json(json!({"ok": true, "success": true, "message": message})).into_response()
}

async fn kernel_pin(
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
    if !matches!(InstallChannel::parse(&req.version), Some(InstallChannel::Exact(_))) {
        return err_json(StatusCode::BAD_REQUEST, "版本钉仅接受合法的精确版本号");
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
/// 事务式:先备份重命名 → 重启 → 健康检查通过才删备份;失败恢复备份并如实报错。
/// 历史版本直接删文件 + `spawn().unwrap_or(0)`,重启失败时既丢了认证又假报成功。
async fn kernel_reset_auth(
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
        return Json(json!({"ok": true, "success": true, "message": "密码本就未设置"})).into_response();
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
    let spawn_result = state.kernel.spawn();
    let mut healthy = spawn_result.is_ok();
    if spawn_result.is_ok() {
        healthy = wait_healthy_blocking(&state).await;
    }
    if healthy {
        // 重启成功才真正删除备份;删除失败必须告知(备份里留着旧口令哈希)
        let backup_removed = std::fs::remove_file(&backup);
        state.events.publish(render_snapshot(&state));
        let msg = match backup_removed {
            Ok(_) => "管理员密码已重置,服务已重启".to_string(),
            Err(e) => format!(
                "管理员密码已重置,服务已重启;但备份删除失败({e}),残留于 {}",
                backup.display()
            ),
        };
        Json(json!({"ok": true, "success": true, "message": msg})).into_response()
    } else {
        // 回滚(P1-02):先停止失败的新实例 —— 它可能仍存活只是不健康,
        // 不停则恢复后旧实例 spawn 会撞 "already running",回滚静默失效
        state.kernel.stop().await;
        // 恢复认证文件;失败必须如实上报(备份仍在,不丢认证)
        let restore = std::fs::rename(&backup, &auth);
        let respawn = state.kernel.spawn();
        // 旧实例同样要通过健康检查,才允许宣称"已恢复"
        let respawn_healthy = if respawn.is_ok() {
            wait_healthy_blocking(&state).await
        } else {
            false
        };
        state.events.publish(render_snapshot(&state));
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

fn write_ui_config(path: &std::path::Path, config: &serde_json::Value) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut bytes = serde_json::to_vec_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    bytes.push(b'\n');
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::rename(tmp, path)
}

fn read_ui_config(path: &std::path::Path) -> std::io::Result<serde_json::Value> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn sync_main_entry(path: &std::path::Path, port: u16, installed: bool) -> std::io::Result<()> {
    let mut config = read_ui_config(path)?;
    let urls = config
        .get_mut(".url")
        .and_then(|value| value.as_object_mut())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing .url"))?;
    let entry_matches = urls
        .get("qwenpaw.main")
        .map(|entry| {
            entry["port"].as_str() == Some(&port.to_string())
                && entry["noDisplay"].as_bool() == Some(!installed)
        })
        .unwrap_or(false);
    if entry_matches {
        return Ok(());
    }
    let entry = urls.entry("qwenpaw.main").or_insert_with(|| {
        json!({
            "title": "QwenPaw",
            "icon": "images/icon_{0}.png",
            "type": "url",
            "protocol": "http",
            "port": port.to_string(),
            "url": "/",
            "allUsers": false,
            "control": {"accessPerm": "editable"}
        })
    });
    entry["port"] = serde_json::Value::String(port.to_string());
    entry["noDisplay"] = serde_json::Value::Bool(!installed);
    write_ui_config(path, &config)
}

fn sync_ui_port(path: &std::path::Path, port: u16) -> std::io::Result<()> {
    let mut config = read_ui_config(path)?;
    if let Some(entry) = config.pointer_mut("/.url/qwenpaw.main") {
        entry["port"] = serde_json::Value::String(port.to_string());
        write_ui_config(path, &config)?;
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct PortReq {
    port: u16,
}

/// 改端口:事务式 —— 落盘新端口 → 重启内核 → 健康检查,任何一步失败都回滚
/// 旧端口并重启旧实例,绝不假报成功。历史版本忽略写文件/启动错误固定返回成功。
async fn port(
    State(state): State<AppState>,
    mut req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let _operation = match begin_operation(&state) {
        Ok(guard) => guard,
        Err(response) => return *response,
    };
    let body = match axum::body::to_bytes(std::mem::take(req.body_mut()), 4 * 1024).await {
        Ok(b) => b,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let req: PortReq = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    // 保留端口:监督者自身监听口(占它 = 桥接失联)与历史冲突口 19091
    if !(1024..=65535).contains(&req.port) || req.port == state.supervisor_port || req.port == 19091
    {
        return err_json(
            StatusCode::BAD_REQUEST,
            &format!(
                "端口无效(1024~65535,且不可为监督者保留端口 {}/19091)",
                state.supervisor_port
            ),
        );
    }
    if req.port == state.kernel.port() {
        if let Err(e) = sync_ui_port(std::path::Path::new(&state.ui_config_file), req.port) {
            return err_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("桌面入口端口同步失败: {e}"),
            );
        }
        return Json(json!({
            "ok": true, "success": true,
            "message": format!("端口本就是 {},无需更改", req.port),
            "port": req.port,
        }))
        .into_response();
    }
    let port_file = format!("{}/service_port", state.var_dir);
    let old_port = state.kernel.port();

    // 1) 落盘新端口(写失败即中止,此时内核未动,无回滚必要)
    if let Err(e) = std::fs::write(&port_file, format!("{}\n", req.port)) {
        return err_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("写入端口配置失败: {e}"),
        );
    }
    // 2) 重启内核到新端口
    state.kernel.set_port(req.port);
    state.kernel.stop().await;
    let spawn_result = state.kernel.spawn();
    let mut healthy = spawn_result.is_ok();
    if healthy {
        healthy = wait_healthy_blocking(&state).await;
    }
    let mut ui_error = None;
    if healthy {
        if let Err(e) = sync_ui_port(std::path::Path::new(&state.ui_config_file), req.port) {
            healthy = false;
            ui_error = Some(e);
        }
    }
    if healthy {
        state.events.publish(render_snapshot(&state));
        Json(json!({
            "ok": true, "success": true,
            "message": format!("端口已更改为 {},服务已重启", req.port),
            "port": req.port,
        }))
        .into_response()
    } else {
        // 回滚(P1-02):先停止失败的新实例 —— 它可能仍存活只是不健康,
        // 不停则旧实例 spawn 会撞 "already running",回滚静默失效
        state.kernel.stop().await;
        // 恢复旧端口配置;写回失败必须如实上报,不得宣称"已回滚"
        let restore = std::fs::write(&port_file, format!("{}\n", old_port));
        let ui_restore = sync_ui_port(std::path::Path::new(&state.ui_config_file), old_port);
        state.kernel.set_port(old_port);
        let rollback = state.kernel.spawn();
        // 旧实例同样要通过健康检查,才允许宣称"已回滚"(与正向路径同一标准)
        let rolled_healthy = if rollback.is_ok() {
            wait_healthy_blocking(&state).await
        } else {
            false
        };
        state.events.publish(render_snapshot(&state));
        let detail = match ui_error {
            Some(e) => format!("桌面入口端口同步失败: {e}"),
            None => match spawn_result {
                Err(e) => format!("新端口启动失败: {e}"),
                Ok(_) => "新端口启动后健康检查未通过(端口冲突或内核异常)".to_string(),
            },
        };
        let roll_msg = if let Err(e) = ui_restore {
            format!("旧桌面入口恢复失败: {e},请手动检查 {}", state.ui_config_file)
        } else {
            match (&restore, &rollback, rolled_healthy) {
                (Err(e), _, _) => format!(
                    "旧端口配置写回失败({e}),服务暂按原端口 {old_port} 运行,重启后将回到新端口,请修复 {} 后重试",
                    port_file
                ),
                (_, Err(e), _) => format!("回滚启动失败: {e},请手动启动服务"),
                (_, _, false) => "旧实例已启动但健康检查未通过,请检查内核日志".to_string(),
                (_, _, true) => "已回滚至原端口".to_string(),
            }
        };
        err_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{detail};{roll_msg}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_index_uses_the_selected_mirror() {
        assert_eq!(version_index_url(""), "https://pypi.org/simple/qwenpaw/");
        assert_eq!(
            version_index_url("https://pypi.tuna.tsinghua.edu.cn/simple"),
            "https://pypi.tuna.tsinghua.edu.cn/simple/qwenpaw/"
        );
        assert_eq!(
            version_index_url("https://mirrors.aliyun.com/pypi/simple/"),
            "https://mirrors.aliyun.com/pypi/simple/qwenpaw/"
        );
        assert_eq!(
            version_index_url("https://mirrors.aliyun.com/pypi/simple/;bad"),
            "https://pypi.org/simple/qwenpaw/"
        );
    }

    #[test]
    fn operation_guard_rejects_concurrent_changes() {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let _guard = try_operation(&lock).unwrap();
        let response = try_operation(&lock).unwrap_err();
        assert_eq!(response.status(), StatusCode::CONFLICT);
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
        assert_eq!(std::fs::read_to_string(share.join("config.json")).unwrap(), "keep");
        assert!(remove_kernel_files(&home, &var).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_main_entry_follows_kernel_installation() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-ui-entry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, r#"{".url":{"qwenpaw.console":{"type":"iframe"}}}"#).unwrap();
        sync_main_entry(&path, 18088, false).unwrap();
        sync_main_entry(&path, 18088, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["port"], "18088");
        assert_eq!(value[".url"]["qwenpaw.main"]["noDisplay"], false);
        assert_eq!(value[".url"]["qwenpaw.console"]["type"], "iframe");
        sync_main_entry(&path, 18088, false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["noDisplay"], true);
        sync_ui_port(&path, 18099).unwrap();
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["port"], "18099");
        assert_eq!(value[".url"]["qwenpaw.main"]["noDisplay"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_port_sync_updates_only_the_main_entry() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-ui-port-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(
            &path,
            r#"{".url":{"qwenpaw.console":{"type":"iframe"},"qwenpaw.main":{"type":"url","port":"18088"}}}"#,
        )
        .unwrap();
        sync_ui_port(&path, 18099).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["port"], "18099");
        assert_eq!(value[".url"]["qwenpaw.console"]["type"], "iframe");
        assert!(!dir.join("config.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
