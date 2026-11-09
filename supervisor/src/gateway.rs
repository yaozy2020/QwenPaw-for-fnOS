use std::{
    collections::HashSet,
    fs, io,
    net::Ipv6Addr,
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{to_bytes, Body},
    extract::{Extension, State},
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json, Router,
};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use ring::hmac;
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize,
};
use serde_json::{json, Map, Value};
use tokio::{
    io::AsyncWriteExt,
    sync::{watch, Mutex, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tower::ServiceExt;

use crate::{
    api::{self, AppState},
    config::Config,
    control,
};

const PREFIX: &str = "/app/qwenpaw";
const READY_PATH: &str = "/__qwenpaw_gateway_ready";
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 32;
const MAX_SSE_CONNECTIONS: usize = 8;
const OVERLOAD_MESSAGE: &str = "Gateway is busy; retry shortly";
const CSRF_COOKIE: &str = "qwenpaw_csrf";
const HOST_CSRF_COOKIE: &str = "__Host-qwenpaw_csrf";
const CSRF_LIFETIME: u64 = 2 * 60 * 60;

fn trusted_gateway_peer(uid: u32, executable: Option<&Path>, test_exe: Option<&Path>) -> bool {
    // SO_PEERCRED UID was sampled at accept; root is the production gateway boundary.
    if uid == 0 {
        return true;
    }
    let Some((executable, test_exe)) = executable.zip(test_exe) else {
        return false;
    };
    executable == test_exe
}

// Explicit test fallback for non-root local fixtures; never enable it in production. Root peers return above without /proc reads.
fn test_peer_override() -> Option<PathBuf> {
    if std::env::var_os("QWENPAW_ENABLE_CRASH_POINT").as_deref() != Some(std::ffi::OsStr::new("1"))
    {
        return None;
    }
    std::env::var_os("QWENPAW_TEST_GATEWAY_PEER_EXE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

pub(crate) fn socket_path_from_config(cfg: &Config) -> Result<Option<PathBuf>, String> {
    let Some(path) = cfg.gateway_socket.as_deref() else {
        return Ok(None);
    };
    validate_socket_path(path, &control::socket_path(&cfg.var_dir))?;
    Ok(Some(path.to_path_buf()))
}

fn validate_socket_path(path: &Path, private: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err("QWENPAW_GATEWAY_SOCKET must be an absolute Unix socket path".into());
    }
    if path.as_os_str().as_encoded_bytes().len() >= 108 {
        return Err("QWENPAW_GATEWAY_SOCKET exceeds the Unix socket path limit".into());
    }
    let same_parent = path
        .parent()
        .zip(private.parent())
        .is_some_and(|(left, right)| {
            left == right
                || fs::canonicalize(left)
                    .ok()
                    .zip(fs::canonicalize(right).ok())
                    .is_some_and(|(left, right)| left == right)
        });
    if path == private || (same_parent && path.file_name() == private.file_name()) {
        return Err("gateway socket must be distinct from supervisor-control.sock".into());
    }
    if path
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("gateway socket path must not contain parent traversal".into());
    }
    Ok(())
}

pub(crate) struct RunningGateway {
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl RunningGateway {
    pub(crate) async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

pub(crate) struct BoundGateway {
    listener: UnixListener,
    cleanup: SocketCleanup,
}

pub(crate) fn bind(path: &Path) -> io::Result<BoundGateway> {
    let listener = control::bind_private_socket(path)?;
    listener.set_nonblocking(true)?;
    let cleanup = SocketCleanup {
        path: path.to_path_buf(),
        identity: SocketIdentity::from_path(path)?,
    };
    if !cleanup.identity.matches_path(&cleanup.path)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "gateway socket changed during bind",
        ));
    }
    Ok(BoundGateway { listener, cleanup })
}

pub(crate) fn start(
    bound: BoundGateway,
    state: AppState,
    supervisor_executable: &Path,
    helper_executable: &Path,
    persistent_helper_executable: &Path,
) -> io::Result<RunningGateway> {
    if !bound.cleanup.identity.matches_path(&bound.cleanup.path)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "gateway socket changed after bind",
        ));
    }
    let BoundGateway { listener, cleanup } = bound;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    if !cleanup.identity.matches_path(&cleanup.path)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "gateway socket changed before start",
        ));
    }
    let app_dest = std::env::var_os("TRIM_APPDEST")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/apps/qwenpaw/target"));
    let router = build_router_with_supervisors(
        state,
        app_dest.join("www"),
        test_peer_override(),
        vec![
            supervisor_executable.to_path_buf(),
            helper_executable.to_path_buf(),
            persistent_helper_executable.to_path_buf(),
        ],
    );
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(serve(listener, cleanup, router, shutdown_rx));
    Ok(RunningGateway { shutdown, task })
}

struct SocketIdentity {
    dev: u64,
    ino: u64,
    _inode: fs::File,
}

impl SocketIdentity {
    fn from_path(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let inode = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = inode.metadata()?;
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "gateway socket identity mismatch",
            ));
        }
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            _inode: inode,
        })
    }

    fn matches_path(&self, path: &Path) -> io::Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        Ok(metadata.file_type().is_socket()
            && metadata.uid() == unsafe { libc::geteuid() }
            && (metadata.dev(), metadata.ino()) == (self.dev, self.ino))
    }

    fn remove_if_owned(&self, path: &Path) {
        if fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.uid() == unsafe { libc::geteuid() }
                && (metadata.dev(), metadata.ino()) == (self.dev, self.ino)
        }) {
            let _ = fs::remove_file(path);
        }
    }
}

struct SocketCleanup {
    path: PathBuf,
    identity: SocketIdentity,
}
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        self.identity.remove_if_owned(&self.path);
    }
}

#[derive(Clone, Copy)]
struct Peer {
    uid: u32,
    pid: i32,
    starttime: u64,
}

impl Peer {
    fn from_stream(stream: &tokio::net::UnixStream) -> io::Result<Self> {
        let credentials = stream.peer_cred()?;
        let pid = credentials.pid().filter(|pid| *pid > 0).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "missing gateway peer PID")
        })?;
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let starttime = crate::service::parse_starttime(&stat).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid gateway peer stat")
        })?;
        Ok(Self {
            uid: credentials.uid(),
            pid,
            starttime,
        })
    }

    fn executable(self) -> io::Result<PathBuf> {
        let stat = fs::read_to_string(format!("/proc/{}/stat", self.pid))?;
        if crate::service::parse_starttime(&stat) != Some(self.starttime) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "gateway peer PID was reused",
            ));
        }
        fs::read_link(format!("/proc/{}/exe", self.pid))
    }

    fn is_supervisor(self, expected_executables: &[PathBuf]) -> bool {
        self.uid == unsafe { libc::geteuid() }
            && self.executable().ok().is_some_and(|executable| {
                expected_executables
                    .iter()
                    .any(|expected| crate::service::executable_matches(expected, &executable))
            })
    }
}

async fn serve(
    listener: tokio::net::UnixListener,
    _cleanup: SocketCleanup,
    router: Router,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
            joined = connections.join_next(), if !connections.is_empty() => { let _ = joined; }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(pair) => pair,
                    Err(error) => {
                        eprintln!("[Supervisor] gateway accept failed: {error}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                if connections.len() >= MAX_CONNECTIONS {
                    reject_overloaded(stream).await;
                    continue;
                }
                let peer = Peer::from_stream(&stream).ok();
                let slot = SseConnectionSlot::default();
                let service = TowerToHyperService::new(
                    router.clone().layer(Extension(peer)).layer(Extension(slot)),
                );
                connections.spawn(async move {
                    let mut builder = http1::Builder::new();
                    builder.keep_alive(false).max_buf_size(32 * 1024)
                        .timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5));
                    let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                });
            }
        }
    }
    drop(listener);
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}

async fn reject_overloaded(mut stream: tokio::net::UnixStream) {
    let body = format!(r#"{{"success":false,"message":"{OVERLOAD_MESSAGE}"}}"#);
    let response = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 1\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

#[derive(Clone, Default)]
struct SseConnectionSlot(Arc<Mutex<Option<OwnedSemaphorePermit>>>);

#[derive(Clone)]
struct GatewayState {
    app: AppState,
    api: Router,
    www_root: PathBuf,
    test_exe: Option<PathBuf>,
    supervisor_executables: Vec<PathBuf>,
    sse_connections: Arc<Semaphore>,
    web_client: hyper_util::client::legacy::Client<
        hyper_util::client::legacy::connect::HttpConnector,
        Body,
    >,
}

#[cfg(test)]
fn build_router(app: AppState, www_root: PathBuf, test_exe: Option<PathBuf>) -> Router {
    build_router_with_supervisors(
        app,
        www_root,
        test_exe,
        std::env::current_exe().ok().into_iter().collect(),
    )
}

fn build_router_with_supervisors(
    app: AppState,
    www_root: PathBuf,
    test_exe: Option<PathBuf>,
    supervisor_executables: Vec<PathBuf>,
) -> Router {
    let mut web_connector = hyper_util::client::legacy::connect::HttpConnector::new();
    web_connector.set_connect_timeout(Some(Duration::from_secs(3)));
    let web_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(web_connector);
    let state = GatewayState {
        api: api::build_router(app.clone()),
        app,
        www_root,
        test_exe,
        supervisor_executables,
        sse_connections: Arc::new(Semaphore::new(MAX_SSE_CONNECTIONS)),
        web_client,
    };
    Router::new().fallback(dispatch).with_state(state)
}

pub fn web_tcp_router(app: AppState) -> Router {
    let mut web_connector = hyper_util::client::legacy::connect::HttpConnector::new();
    web_connector.set_connect_timeout(Some(Duration::from_secs(3)));
    let web_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(web_connector);
    let state = GatewayState {
        api: api::build_router(app.clone()),
        app,
        www_root: PathBuf::new(),
        test_exe: None,
        supervisor_executables: Vec::new(),
        sse_connections: Arc::new(Semaphore::new(MAX_SSE_CONNECTIONS)),
        web_client,
    };
    Router::new()
        .fallback(web_tcp_dispatch)
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(state)
}

async fn web_tcp_dispatch(State(state): State<GatewayState>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_owned();
    if let Err(status) = web_proxy::validate_path(&path) {
        return status.into_response();
    }
    if web_proxy::upgrade_requested(request.headers()) {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "WebSocket upgrades are not supported by this web proxy",
        )
            .into_response();
    }
    web_proxy::forward_root(&state, request, &path).await
}

async fn dispatch(
    State(state): State<GatewayState>,
    peer: Option<Extension<Option<Peer>>>,
    slot: Option<Extension<SseConnectionSlot>>,
    mut request: Request<Body>,
) -> Response {
    let Some(Extension(Some(peer))) = peer else {
        return forbidden("peer extension missing");
    };
    // The private readiness probe never creates a platform identity or enters management handlers.
    if request.uri().path() == READY_PATH {
        if request.method() != Method::GET
            || !peer.is_supervisor(&state.supervisor_executables)
            || !unique_header(request.headers(), "x-supervisor-token")
                .ok()
                .flatten()
                .is_some_and(|token| constant_time_eq(token, &state.app.token))
            || request.headers().contains_key("x-trim-userid")
            || request.headers().contains_key("x-trim-isadmin")
            || request.headers().contains_key("x-trim-username")
        {
            return forbidden("invalid readiness probe");
        }
        return (
            StatusCode::OK,
            Json(json!({"gateway":"ready", "pid":std::process::id()})),
        )
            .into_response();
    }
    let executable = (peer.uid != 0 && state.test_exe.is_some())
        .then(|| peer.executable().ok())
        .flatten();
    if !trusted_gateway_peer(peer.uid, executable.as_deref(), state.test_exe.as_deref()) {
        return forbidden("untrusted peer");
    }
    let Some(identity) = platform_identity(request.headers()) else {
        return forbidden("invalid platform identity");
    };
    if !identity.admin {
        return forbidden("administrator required");
    }
    let secure = match unique_header(request.headers(), "x-forwarded-proto") {
        Ok(None | Some("http")) => false,
        Ok(Some("https")) => true,
        _ => return forbidden("invalid forwarded protocol"),
    };
    let path = match normalize_path(request.uri().path()) {
        Ok(path) => path,
        Err(status) => return status.into_response(),
    };
    let cookie = match selected_cookie(request.headers(), secure) {
        Ok(cookie) => cookie,
        Err(()) => return forbidden("invalid cookie"),
    };
    let csrf = Csrf {
        secret: &state.app.token,
        identity: &identity,
        secure,
    };
    let method = request.method().clone();
    let web_path = match web_proxy::mount_path(&path) {
        Ok(path) => path,
        Err(status) => return status.into_response(),
    };
    if let Some(web_path) = web_path {
        if !matches!(method, Method::GET | Method::HEAD | Method::POST) {
            let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD, POST"));
            return response;
        }
        if method == Method::POST {
            match unique_header(request.headers(), "sec-fetch-site") {
                Ok(Some(site)) if site.eq_ignore_ascii_case("cross-site") => {
                    return forbidden("cross-site");
                }
                Err(()) => return forbidden("invalid fetch metadata"),
                _ => {}
            }
            if !origin_allowed(request.headers()) {
                return forbidden("invalid origin");
            }
        }
        if web_proxy::upgrade_requested(request.headers()) {
            return (
                StatusCode::NOT_IMPLEMENTED,
                "WebSocket upgrades are not supported by this gateway",
            )
                .into_response();
        }
        let mut response = web_proxy::forward(&state, request, web_path, secure).await;
        if response.status() == StatusCode::OK
            && response
                .headers()
                .get(header::CONTENT_TYPE)
                .is_some_and(|value| value.as_bytes().starts_with(b"text/html"))
            && !cookie
                .as_deref()
                .is_some_and(|cookie| csrf.valid(cookie, now()))
        {
            let token = match csrf.issue(now()) {
                Ok(token) => token,
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            let cookie = format!(
                "{}={token}; Path=/; SameSite=Lax{}",
                cookie_name(secure),
                if secure { "; Secure" } else { "" }
            );
            let Ok(cookie) = HeaderValue::from_str(&cookie) else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            response.headers_mut().append(header::SET_COOKIE, cookie);
        }
        return response;
    }
    if let Some(api_path) = path.strip_prefix("/api/") {
        let (mut parts, body) = request.into_parts();
        let mutation = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
        if mutation
            && unique_header(&parts.headers, "sec-fetch-site")
                .ok()
                .flatten()
                == Some("cross-site")
        {
            return forbidden("cross-site");
        }
        if mutation && !origin_allowed(&parts.headers) {
            return forbidden("invalid origin");
        }
        let body =
            match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, MAX_REQUEST_BYTES))
                .await
            {
                Ok(Ok(body)) => body,
                Ok(Err(_)) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
                Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
            };
        let mut body = body.to_vec();
        if mutation {
            let supplied = match csrf_input(&parts.headers, parts.uri.query(), &mut body) {
                Ok(value) => value,
                Err(()) => return forbidden("invalid csrf"),
            };
            if !cookie
                .as_deref()
                .zip(supplied.as_deref())
                .is_some_and(|(cookie, supplied)| {
                    constant_time_eq(cookie, supplied) && csrf.valid(cookie, now())
                })
            {
                return forbidden("invalid csrf");
            }
        }
        let uri = match parts.uri.query() {
            Some(query) => format!("/{api_path}?{query}"),
            None => format!("/{api_path}"),
        };
        parts.uri = match uri.parse() {
            Ok(uri) => uri,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        parts.headers.remove("x-supervisor-token");
        let Ok(token) = HeaderValue::from_str(&state.app.token) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        parts.headers.insert("x-supervisor-token", token);
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.remove(header::TRANSFER_ENCODING);
        parts.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if api_path == "events" && method == Method::GET {
            let permit = match state.sse_connections.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => return retryable_overload(),
            };
            let Some(Extension(slot)) = slot else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            *slot.0.lock().await = Some(permit);
        }
        let request = Request::from_parts(parts, Body::from(body));
        return match tokio::time::timeout(Duration::from_secs(90), state.api.oneshot(request)).await
        {
            Ok(Ok(mut response)) => {
                response.headers_mut().remove("x-supervisor-token");
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            }
            Ok(Err(never)) => match never {},
            Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
        };
    }
    request.headers_mut().remove("x-supervisor-token");
    let accept_encoding = match unique_header(request.headers(), "accept-encoding") {
        Ok(value) => value.unwrap_or_default().to_owned(),
        Err(()) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let root = state.www_root;
    let response = tokio::task::spawn_blocking(move || {
        crate::web_static::serve(&root, &path, method.as_str(), Some(&accept_encoding))
    })
    .await;
    let response = match response {
        Ok(response) => response,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut builder = Response::builder().status(response.status);
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    let mut response = builder
        .body(Body::from(response.body))
        .expect("static response headers");
    if response.status() == StatusCode::OK
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes().starts_with(b"text/html"))
        && !cookie
            .as_deref()
            .is_some_and(|cookie| csrf.valid(cookie, now()))
    {
        let token = match csrf.issue(now()) {
            Ok(token) => token,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let cookie = format!(
            "{}={token}; Path=/; SameSite=Lax{}",
            cookie_name(secure),
            if secure { "; Secure" } else { "" }
        );
        let Ok(cookie) = HeaderValue::from_str(&cookie) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

fn unique_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|value| std::str::from_utf8(value.as_bytes()).map_err(|_| ()))
        .transpose()?;
    if values.next().is_some() {
        return Err(());
    }
    Ok(value)
}

struct Identity {
    userid: String,
    username: String,
    admin: bool,
}
fn platform_identity(headers: &HeaderMap) -> Option<Identity> {
    let userid = unique_header(headers, "x-trim-userid").ok()??;
    let username = unique_header(headers, "x-trim-username").ok()??;
    let role = unique_header(headers, "x-trim-isadmin").ok()??;
    if userid.is_empty()
        || !userid.bytes().all(|byte| byte.is_ascii_digit())
        || userid.parse::<u64>().is_err()
        || username.is_empty()
        || username.len() > 256
        || username.chars().any(char::is_control)
        || !matches!(role, "true" | "false")
    {
        return None;
    }
    Some(Identity {
        userid: userid.into(),
        username: username.into(),
        admin: role == "true",
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OriginScheme {
    Http,
    Https,
}

#[derive(PartialEq, Eq)]
struct HttpOrigin {
    scheme: OriginScheme,
    host: String,
    port: u16,
}

fn origin_allowed(headers: &HeaderMap) -> bool {
    let origin = match unique_header(headers, "origin") {
        Ok(Some(value)) => match parse_origin(value) {
            Some(origin) => origin,
            None => return false,
        },
        Ok(None) => return true,
        Err(()) => return false,
    };
    let referer = match unique_header(headers, "referer") {
        Ok(referer) => referer,
        Err(()) => return false,
    };
    match unique_header(headers, "x-forwarded-host") {
        Ok(Some(host)) => {
            let Some(proto) = unique_header(headers, "x-forwarded-proto").ok().flatten() else {
                return false;
            };
            parse_forwarded_origin(proto, host).is_some_and(|expected| expected == origin)
        }
        Ok(None) => referer
            .and_then(parse_referer_origin)
            .is_some_and(|expected| expected == origin),
        Err(()) => false,
    }
}

fn parse_origin(value: &str) -> Option<HttpOrigin> {
    if value.ends_with('/') {
        return None;
    }
    let uri = value.parse::<Uri>().ok()?;
    if uri
        .path_and_query()
        .is_some_and(|path_and_query| path_and_query.as_str() != "/")
    {
        return None;
    }
    parse_uri_origin(&uri)
}

fn parse_referer_origin(value: &str) -> Option<HttpOrigin> {
    let uri = value.parse::<Uri>().ok()?;
    parse_uri_origin(&uri)
}

fn parse_uri_origin(uri: &Uri) -> Option<HttpOrigin> {
    let scheme = parse_origin_scheme(uri.scheme_str()?)?;
    parse_authority(scheme, uri.authority()?.as_str())
}

fn parse_forwarded_origin(proto: &str, host: &str) -> Option<HttpOrigin> {
    parse_authority(parse_origin_scheme(proto)?, host)
}

fn parse_origin_scheme(value: &str) -> Option<OriginScheme> {
    if value.eq_ignore_ascii_case("http") {
        Some(OriginScheme::Http)
    } else if value.eq_ignore_ascii_case("https") {
        Some(OriginScheme::Https)
    } else {
        None
    }
}

fn parse_authority(scheme: OriginScheme, authority: &str) -> Option<HttpOrigin> {
    if authority.is_empty()
        || authority != authority.trim()
        || authority.contains('@')
        || authority.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
    {
        return None;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let ipv6 = rest[..end].parse::<Ipv6Addr>().ok()?;
        let suffix = &rest[end + 1..];
        let port = if suffix.is_empty() {
            None
        } else {
            Some(suffix.strip_prefix(':')?.parse::<u16>().ok()?)
        };
        (format!("[{ipv6}]"), port)
    } else {
        if authority.contains(['[', ']']) {
            return None;
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port.parse::<u16>().ok()?)),
            Some(_) => return None,
            None => (authority, None),
        };
        if host.is_empty() || !host.bytes().all(is_valid_host_byte) {
            return None;
        }
        (host.to_ascii_lowercase(), port)
    };
    Some(HttpOrigin {
        scheme,
        port: port.unwrap_or(match scheme {
            OriginScheme::Http => 80,
            OriginScheme::Https => 443,
        }),
        host,
    })
}

fn is_valid_host_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~!$&'()*+;=".contains(&byte)
}

fn normalize_path(path: &str) -> Result<String, StatusCode> {
    let tail = if path == PREFIX {
        "/"
    } else {
        path.strip_prefix(PREFIX)
            .filter(|tail| tail.starts_with('/'))
            .ok_or(StatusCode::NOT_FOUND)?
    };
    if tail.starts_with("/api/")
        && (path.len() > 8 * 1024
            || path.contains(['\\', '\0', '#'])
            || tail.contains('%')
            || tail[1..].contains("//")
            || tail.split('/').any(|part| matches!(part, "." | "..")))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(tail.into())
}

fn cookie_name(secure: bool) -> &'static str {
    if secure {
        HOST_CSRF_COOKIE
    } else {
        CSRF_COOKIE
    }
}

fn selected_cookie(headers: &HeaderMap, secure: bool) -> Result<Option<String>, ()> {
    let mut secure_cookie = None;
    let mut legacy_cookie = None;
    for value in headers.get_all(header::COOKIE) {
        let value = value.to_str().map_err(|_| ())?;
        for pair in value.split(';') {
            let (name, value) = pair.trim().split_once('=').unwrap_or((pair.trim(), ""));
            let slot = match name.trim() {
                HOST_CSRF_COOKIE => &mut secure_cookie,
                CSRF_COOKIE => &mut legacy_cookie,
                _ => continue,
            };
            if slot.is_some() {
                return Err(());
            }
            *slot = Some(value.trim().to_owned());
        }
    }
    Ok(if secure { secure_cookie } else { legacy_cookie })
}

struct Csrf<'a> {
    secret: &'a str,
    identity: &'a Identity,
    secure: bool,
}
impl Csrf<'_> {
    fn message(&self, payload: &str) -> Vec<u8> {
        serde_json::to_vec(&json!([
            "qwenpaw-gateway-csrf-v1",
            self.identity.userid,
            self.identity.username,
            self.secure,
            payload
        ]))
        .expect("CSRF signature payload")
    }

    fn issue(&self, timestamp: u64) -> Result<String, String> {
        let payload = format!("{timestamp}.{}", crate::random_token()?);
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.secret.as_bytes());
        let tag = hmac::sign(&key, &self.message(&payload));
        Ok(format!("{payload}.{}", hex(tag.as_ref())))
    }

    fn valid(&self, token: &str, timestamp: u64) -> bool {
        let fields: Vec<_> = token.split('.').collect();
        if fields.len() != 3
            || fields[1].len() != 64
            || fields[2].len() != 64
            || !fields[1].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return false;
        }
        let Ok(issued) = fields[0].parse::<u64>() else {
            return false;
        };
        if issued > timestamp || timestamp - issued > CSRF_LIFETIME {
            return false;
        }
        let Some(tag) = unhex(fields[2]) else {
            return false;
        };
        let payload = format!("{}.{}", fields[0], fields[1]);
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.secret.as_bytes());
        hmac::verify(&key, &self.message(&payload), &tag).is_ok()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn unhex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |byte: u8| (byte as char).to_digit(16).map(|value| value as u8);
            Some(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn constant_time_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0, |diff, (left, right)| diff | (left ^ right))
            == 0
}
fn retryable_overload() -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"success":false, "message":OVERLOAD_MESSAGE})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

fn forbidden(reason: &str) -> Response {
    eprintln!("[Gateway] forbidden: {reason}");
    (
        StatusCode::FORBIDDEN,
        Json(json!({"success":false, "message":"Forbidden"})),
    )
        .into_response()
}

struct UniqueObject(Map<String, Value>);
impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = UniqueObject;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an object without duplicate fields")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if values.len() >= 128 || values.insert(key, value).is_some() {
                        return Err(de::Error::custom("too many or duplicate fields"));
                    }
                }
                Ok(UniqueObject(values))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

fn form_fields(input: &str) -> Result<Vec<(String, String)>, ()> {
    let mut result = Vec::new();
    let mut names = HashSet::new();
    for pair in input.split('&').filter(|pair| !pair.is_empty()) {
        if result.len() >= 128 {
            return Err(());
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decode = |input: &str| -> Result<String, ()> {
            let input = input.replace('+', " ");
            for (offset, byte) in input.bytes().enumerate() {
                if byte == b'%'
                    && input
                        .as_bytes()
                        .get(offset + 1..offset + 3)
                        .is_none_or(|digits| !digits.iter().all(u8::is_ascii_hexdigit))
                {
                    return Err(());
                }
            }
            let decoded = percent_encoding::percent_decode_str(&input)
                .decode_utf8()
                .map_err(|_| ())?
                .into_owned();
            if decoded.contains('\0') {
                return Err(());
            }
            Ok(decoded)
        };
        let name = decode(name)?;
        if !names.insert(name.clone()) {
            return Err(());
        }
        result.push((name, decode(value)?));
    }
    Ok(result)
}

fn csrf_input(
    headers: &HeaderMap,
    query: Option<&str>,
    body: &mut Vec<u8>,
) -> Result<Option<String>, ()> {
    let mut supplied = unique_header(headers, "x-csrf-token")?.map(str::to_owned);
    let mut take = |value: String| {
        if supplied.is_some() {
            return Err(());
        }
        supplied = Some(value);
        Ok(())
    };
    for (name, value) in form_fields(query.unwrap_or_default())? {
        if name == "_csrf" {
            take(value)?;
        }
    }
    if !body.is_empty() {
        let media_type = unique_header(headers, "content-type")?
            .unwrap_or("application/json")
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let mut object = if media_type.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
            let mut object = Map::new();
            for (name, value) in form_fields(std::str::from_utf8(body).map_err(|_| ())?)? {
                let value = if name == "port" {
                    value
                        .parse::<u16>()
                        .map(|value| json!(value))
                        .map_err(|_| ())?
                } else {
                    Value::String(value)
                };
                object.insert(name, value);
            }
            object
        } else if media_type.eq_ignore_ascii_case("application/json") {
            serde_json::from_slice::<UniqueObject>(body)
                .map_err(|_| ())?
                .0
        } else {
            return Err(());
        };
        if let Some(value) = object.remove("_csrf") {
            take(value.as_str().ok_or(())?.to_owned())?;
        }
        *body = serde_json::to_vec(&object).map_err(|_| ())?;
    }
    Ok(supplied)
}

mod probe;
mod web_proxy;
mod web_proxy_manager;
pub use web_proxy_manager::WebProxyManager;

pub(crate) fn readiness(path: &Path, token_file: &str, expected_pid: i32) -> Result<bool, String> {
    use std::io::{Read, Write};
    let token = fs::read_to_string(token_file)
        .map_err(|error| format!("read gateway probe token: {error}"))?;
    let token = token.trim();
    if token.len() < 32 || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("gateway probe token is invalid".into());
    }
    let mut stream =
        probe::connect(path, expected_pid).map_err(|error| format!("connect gateway: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| error.to_string())?;
    write!(stream, "GET {READY_PATH} HTTP/1.1\r\nHost: localhost\r\nX-Supervisor-Token: {token}\r\nConnection: close\r\n\r\n").map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .take(4096)
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return Ok(false);
    };
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return Ok(false);
    };
    Ok(headers
        .lines()
        .next()
        .is_some_and(|line| matches!(line, "HTTP/1.1 200 OK" | "HTTP/1.0 200 OK"))
        && body["gateway"] == "ready"
        && body["pid"].as_i64() == Some(i64::from(expected_pid)))
}

#[cfg(test)]
mod tests;
