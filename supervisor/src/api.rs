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
    install::{sanitize_mirror, InstallChannel, InstallState},
    kernel::KernelManager,
};

#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<KernelManager>,
    pub install: InstallState,
    pub events: Events,
    pub token: String,
    pub var_dir: String,
    pub home_dir: String,
    pub share_dir: String,
    pub kernel_log: String,
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
        // 内核由 /proc 重新收养(监督者曾被 SIGKILL)时为 true,排障用
        "adopted": st.adopted,
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
        .map(|l| {
            // 窗口起点可能切在多字节字符中间:丢弃非法段由 lossy 兜底,
            // 单行超长截断
            if l.len() > LINE_CAP {
                let mut s = l.to_string();
                s.truncate(LINE_CAP);
                s.push('…');
                s
            } else {
                l.to_string()
            }
        })
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
        "phase": match s.phase { crate::install::Phase::Idle => "idle", crate::install::Phase::Downloading => "downloading", crate::install::Phase::Installing => "installing", crate::install::Phase::Done => "done", crate::install::Phase::Failed => "failed" },
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
        // 重启成功才真正删除备份
        let _ = std::fs::remove_file(&backup);
        state.events.publish(render_snapshot(&state));
        Json(json!({"ok": true, "success": true, "message": "管理员密码已重置,服务已重启"}))
            .into_response()
    } else {
        // 恢复备份,尽力拉回旧配置的内核;错误如实上报
        let _ = std::fs::rename(&backup, &auth);
        let _ = state.kernel.spawn();
        state.events.publish(render_snapshot(&state));
        let detail = match spawn_result {
            Err(e) => format!("服务重启失败: {e}"),
            Ok(_) => "服务重启后健康检查未通过".to_string(),
        };
        err_json(StatusCode::INTERNAL_SERVER_ERROR, &format!("{detail},已恢复原认证文件"))
    }
}

/// 阻塞式健康检查(10s 上限);spawn_blocking 包裹避免占死 IO 线程
async fn wait_healthy_blocking(state: &AppState) -> bool {
    let kernel = Arc::clone(&state.kernel);
    tokio::task::spawn_blocking(move || kernel.wait_healthy(std::time::Duration::from_secs(10)))
        .await
        .unwrap_or(false)
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
    if healthy {
        state.events.publish(render_snapshot(&state));
        Json(json!({
            "ok": true, "success": true,
            "message": format!("端口已更改为 {},服务已重启", req.port),
            "port": req.port,
        }))
        .into_response()
    } else {
        // 3) 回滚:恢复旧端口配置并重启旧实例
        let _ = std::fs::write(&port_file, format!("{}\n", old_port));
        state.kernel.set_port(old_port);
        let rollback = state.kernel.spawn();
        state.events.publish(render_snapshot(&state));
        let detail = match spawn_result {
            Err(e) => format!("新端口启动失败: {e}"),
            Ok(_) => "新端口启动后健康检查未通过(端口冲突或内核异常)".to_string(),
        };
        let roll_msg = match rollback {
            Ok(_) => "已回滚至原端口",
            Err(e) => {
                return err_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("{detail};回滚失败: {e},请手动启动服务"),
                )
            }
        };
        err_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{detail};{roll_msg}",),
        )
    }
}
