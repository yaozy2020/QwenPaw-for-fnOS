use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{api::AppState, runtime_settings::SettingsPatch};

const MAX_REQUEST_BYTES: usize = 4 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(85);
const MAX_CONNECTIONS: usize = 32;

pub fn socket_path(var_dir: &str) -> PathBuf {
    Path::new(var_dir).join("supervisor-control.sock")
}

struct UmaskGuard(libc::mode_t);

impl UmaskGuard {
    fn restrictive() -> Self {
        Self(unsafe { libc::umask(0o077) })
    }
}

impl Drop for UmaskGuard {
    fn drop(&mut self) {
        unsafe { libc::umask(self.0) };
    }
}

fn permission_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn verify_owned_socket(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "private control path is not a Unix socket",
        ));
    }
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(permission_error(
            "private control socket has an unknown owner",
        ));
    }
    if metadata.permissions().mode() & 0o7777 != 0o600 {
        return Err(permission_error("private control socket mode is not 0600"));
    }
    if path.as_os_str().as_bytes().contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private control socket path contains NUL",
        ));
    }
    Ok(())
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    verify_owned_socket(path, &metadata)?;

    match UnixStream::connect(path) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "private control socket is active",
            ));
        }
        Err(error) if error.raw_os_error() == Some(libc::ECONNREFUSED) => {}
        Err(error) => return Err(error),
    }

    let current = fs::symlink_metadata(path)?;
    verify_owned_socket(path, &current)?;
    if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
        return Err(permission_error(
            "private control socket changed during stale-socket check",
        ));
    }
    fs::remove_file(path)
}

pub fn bind_private_socket(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    remove_stale_socket(path)?;

    let listener = {
        let _umask = UmaskGuard::restrictive();
        UnixListener::bind(path)?
    };
    let metadata = fs::symlink_metadata(path)?;
    let identity = (metadata.dev(), metadata.ino());
    let setup = (|| {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(path)?;
        verify_owned_socket(path, &metadata)?;
        Ok(())
    })();
    if let Err(error) = setup {
        drop(listener);
        if let Ok(current) = fs::symlink_metadata(path) {
            if current.file_type().is_socket()
                && current.uid() == unsafe { libc::geteuid() }
                && (current.dev(), current.ino()) == identity
            {
                let _ = fs::remove_file(path);
            }
        }
        return Err(error);
    }
    Ok(listener)
}

pub fn serve(listener: UnixListener, state: AppState, runtime: tokio::runtime::Handle) {
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let (stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => {
                eprintln!("[Supervisor] private control accept failed: {error}");
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let count = active.fetch_add(1, Ordering::AcqRel);
        if count >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::AcqRel);
            let mut stream = stream;
            let _ = write_error(&mut stream, 503, "too many control connections");
            continue;
        }
        let state = state.clone();
        let runtime = runtime.clone();
        let worker_active = Arc::clone(&active);
        let spawn = thread::Builder::new()
            .name("supervisor-control-client".into())
            .spawn(move || {
                let _guard = ConnectionGuard(worker_active);
                handle_connection(stream, state, runtime);
            });
        if let Err(error) = spawn {
            active.fetch_sub(1, Ordering::AcqRel);
            eprintln!("[Supervisor] private control worker could not start: {error}");
        }
    }
}

struct ConnectionGuard(Arc<AtomicUsize>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut libc::ucred as *mut libc::c_void,
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid private control peer credentials",
        ));
    }
    Ok(credentials.uid)
}

fn read_request(stream: &mut UnixStream) -> Result<Vec<u8>, (u16, &'static str)> {
    read_request_with_timeout(stream, READ_TIMEOUT)
}

fn read_request_with_timeout(
    stream: &mut UnixStream,
    timeout: Duration,
) -> Result<Vec<u8>, (u16, &'static str)> {
    let deadline = Instant::now() + timeout;
    let mut body = Vec::with_capacity(128);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err((408, "control request timed out"));
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| (500, "could not set control read timeout"))?;
        let mut byte = [0u8; 1];
        match stream.read(&mut byte) {
            Ok(0) => return Err((400, "request must end with newline")),
            Ok(_) if byte[0] == b'\n' => return Ok(body),
            Ok(_) if body.len() == MAX_REQUEST_BYTES => {
                return Err((413, "control request exceeds 4 KiB"));
            }
            Ok(_) => body.push(byte[0]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err((408, "control request timed out"));
            }
            Err(_) => return Err((400, "could not read control request")),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorSetRequest {
    operation: String,
    value: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallRequest {
    operation: String,
    version: String,
    #[serde(default)]
    mirror: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationRequest {
    operation: String,
}

enum ControlRequest {
    Settings(SettingsPatch),
    MirrorSet(String),
    Install { version: String, mirror: String },
    InstallStatus,
    RuntimeHealth,
    EnsureInitialized,
}

fn parse_control_request(body: &[u8]) -> Result<ControlRequest, ()> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| ())?;
    let operation = match value.get("operation").and_then(serde_json::Value::as_str) {
        Some(operation) => operation,
        None if value.get("operation").is_none() => {
            return serde_json::from_value(value)
                .map(ControlRequest::Settings)
                .map_err(|_| ());
        }
        None => return Err(()),
    };
    match operation {
        "mirror-set" => {
            let request: MirrorSetRequest = serde_json::from_value(value).map_err(|_| ())?;
            if request.operation != "mirror-set" {
                return Err(());
            }
            Ok(ControlRequest::MirrorSet(request.value))
        }
        "install" => {
            let request: InstallRequest = serde_json::from_value(value).map_err(|_| ())?;
            if request.operation != "install" {
                return Err(());
            }
            Ok(ControlRequest::Install {
                version: request.version,
                mirror: request.mirror,
            })
        }
        "install-status" | "runtime-health" | "ensure-initialized" => {
            let request: OperationRequest = serde_json::from_value(value).map_err(|_| ())?;
            match request.operation.as_str() {
                "install-status" => Ok(ControlRequest::InstallStatus),
                "runtime-health" => Ok(ControlRequest::RuntimeHealth),
                "ensure-initialized" => Ok(ControlRequest::EnsureInitialized),
                _ => Err(()),
            }
        }
        _ => Err(()),
    }
}

fn handle_connection(mut stream: UnixStream, state: AppState, runtime: tokio::runtime::Handle) {
    if stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err() {
        return;
    }
    match peer_uid(&stream) {
        Ok(uid) if uid == unsafe { libc::geteuid() } => {}
        Ok(_) => {
            let _ = write_error(&mut stream, 403, "private control peer uid is not allowed");
            return;
        }
        Err(_) => return,
    }

    let body = match read_request(&mut stream) {
        Ok(body) => body,
        Err((status, message)) => {
            let _ = write_error(&mut stream, status, message);
            return;
        }
    };
    let request = match parse_control_request(&body) {
        Ok(request) => request,
        Err(()) => {
            let _ = write_error(&mut stream, 400, "invalid control request JSON");
            return;
        }
    };

    let result = match request {
        ControlRequest::MirrorSet(value) => state.core.set_mirror(&value).map(|()| {
            serde_json::json!({
                "status": 200,
                "ok": true,
                "success": true,
                "message": "下载源已保存"
            })
        }),
        ControlRequest::Install { version, mirror } => runtime
            .block_on(state.core.submit_install(&version, &mirror))
            .map(|acceptance| match acceptance {
                crate::management::InstallAcceptance::Started { seq } => serde_json::json!({
                    "status": 200,
                    "ok": true,
                    "success": true,
                    "message": "安装已开始",
                    "seq": seq,
                    "instance": crate::api::instance_id(),
                    "running": true,
                }),
                crate::management::InstallAcceptance::Busy { seq } => serde_json::json!({
                    "status": 200,
                    "ok": true,
                    "success": false,
                    "message": "安装正在进行中,请等待完成后再试",
                    "seq": seq,
                    "instance": crate::api::instance_id(),
                    "running": true,
                }),
            }),
        ControlRequest::InstallStatus => {
            let snapshot = state.core.install_snapshot();
            Ok(serde_json::json!({
                "status": 200,
                "ok": true,
                "success": true,
                "upgrading": snapshot.running,
                "phase": crate::install::phase_wire(&snapshot.phase),
                "seq": snapshot.seq,
                "instance": crate::api::instance_id(),
                "recovery_required": snapshot.recovery_required,
                "recovery_error": snapshot.recovery_error,
                "fetched": snapshot.fetched,
                "message": snapshot.message,
                "error": snapshot.error,
            }))
        }
        ControlRequest::RuntimeHealth => Ok(serde_json::json!({
            "status": 200,
            "ok": true,
            "success": true,
            "runtime": state.core.runtime_health(),
        })),
        ControlRequest::EnsureInitialized => {
            runtime.block_on(state.core.ensure_initialized()).map(|()| {
                serde_json::json!({
                    "status": 200,
                    "ok": true,
                    "success": true,
                    "message": "QwenPaw 运行时已初始化",
                })
            })
        }
        ControlRequest::Settings(patch) => {
            let _operation = match state.core.begin_operation() {
                Ok(guard) => guard,
                Err(error) => {
                    let status = crate::api::management_error_status(&error).as_u16();
                    let message = error.to_string();
                    let _ = write_error(&mut stream, status, &message);
                    return;
                }
            };
            runtime.block_on(async {
                let outcome = crate::runtime_settings::apply(&state.core, patch).await;
                if outcome.notify_observers {
                    *state
                        .auth_cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) =
                        crate::api::AuthCache::default();
                    let snapshot_state = state.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        snapshot_state
                            .events
                            .publish(crate::api::render_snapshot(&snapshot_state));
                    })
                    .await;
                }
                outcome.result.map(|message| {
                    serde_json::json!({
                        "status": 200,
                        "ok": true,
                        "success": true,
                        "message": message,
                        "port": state.kernel.port(),
                        "log_level": state.kernel.log_level()
                    })
                })
            })
        }
    };
    match result {
        Ok(response) => {
            let _ = write_response(&mut stream, &response);
        }
        Err(error) => {
            let status = crate::api::management_error_status(&error).as_u16();
            let message = error.to_string();
            let _ = write_error(&mut stream, status, &message);
        }
    }
}

fn write_error(stream: &mut UnixStream, status: u16, message: &str) -> io::Result<()> {
    write_response(
        stream,
        &serde_json::json!({
            "status": status,
            "ok": false,
            "success": false,
            "error": message,
            "message": message
        }),
    )
}

fn write_response(stream: &mut UnixStream, response: &serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(response).map_err(io::Error::other)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)
}

#[cfg(test)]
fn read_response(stream: &mut UnixStream) -> Result<serde_json::Value, String> {
    read_response_with_timeout(stream, CLIENT_TIMEOUT)
}

fn read_response_with_timeout(
    stream: &mut UnixStream,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::with_capacity(128);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("control response timed out".into());
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|error| format!("set private control read timeout: {error}"))?;
        let mut byte = [0u8; 1];
        match stream.read(&mut byte) {
            Ok(0) => return Err("control service closed without a response".into()),
            Ok(_) if byte[0] == b'\n' => {
                return serde_json::from_slice(&line)
                    .map_err(|_| "control service returned invalid JSON".into());
            }
            Ok(_) if line.len() == MAX_RESPONSE_BYTES => {
                return Err("control response exceeds 16 KiB".into());
            }
            Ok(_) => line.push(byte[0]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err("control response timed out".into());
            }
            Err(error) => return Err(format!("read control response: {error}")),
        }
    }
}

pub fn submit_settings(path: &Path, patch: &SettingsPatch) -> Result<(), String> {
    let body = serde_json::to_vec(patch).map_err(|error| error.to_string())?;
    submit_request(path, body, "settings")
}

pub fn submit_mirror(path: &Path, value: &str) -> Result<(), String> {
    let body = serde_json::to_vec(&serde_json::json!({
        "operation": "mirror-set",
        "value": value
    }))
    .map_err(|error| error.to_string())?;
    submit_request(path, body, "mirror")
}

const INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const INSTALL_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
enum InstallProgress {
    Pending,
    Done,
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
struct InstallIdentity {
    seq: u64,
    instance: String,
}

pub fn install_and_wait(path: &Path, version: &str, mirror: &str) -> Result<(), String> {
    install_and_wait_with_timeout(
        path,
        version,
        mirror,
        INSTALL_TIMEOUT,
        INSTALL_POLL_INTERVAL,
    )
}

fn install_and_wait_with_timeout(
    path: &Path,
    version: &str,
    mirror: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(), String> {
    if timeout.is_zero() {
        return Err("安装任务等待超时".into());
    }
    let deadline = Instant::now() + timeout;
    let request_timeout = timeout.min(CLIENT_TIMEOUT);
    let body = serde_json::to_vec(&serde_json::json!({
        "operation": "install",
        "version": version,
        "mirror": mirror,
    }))
    .map_err(|error| error.to_string())?;
    let accepted = request_value_with_timeout(path, body, "install", request_timeout)?;
    let identity = parse_install_acceptance(&accepted)?;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("安装任务 {} 等待终态超时", identity.seq));
        }
        let status = install_status_with_timeout(path, remaining.min(CLIENT_TIMEOUT))?;
        match parse_install_progress(&status, &identity)? {
            InstallProgress::Pending => {
                let pause = poll_interval.min(deadline.saturating_duration_since(Instant::now()));
                if !pause.is_zero() {
                    thread::sleep(pause);
                }
            }
            InstallProgress::Done => return Ok(()),
            InstallProgress::Failed(message) => {
                return Err(format!("安装任务 {} 失败: {message}", identity.seq));
            }
        }
    }
}

fn parse_install_acceptance(response: &serde_json::Value) -> Result<InstallIdentity, String> {
    if response.get("status").and_then(serde_json::Value::as_u64) != Some(200)
        || response.get("success").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Err(response_error(response, "install"));
    }
    let seq = response
        .get("seq")
        .and_then(serde_json::Value::as_u64)
        .filter(|seq| *seq > 0)
        .ok_or_else(|| "install acceptance did not include a valid sequence".to_string())?;
    let instance = response
        .get("instance")
        .and_then(serde_json::Value::as_str)
        .filter(|instance| !instance.trim().is_empty())
        .ok_or_else(|| "install acceptance did not include a valid instance".to_string())?
        .to_string();
    if response.get("running").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("install acceptance did not start a running task".into());
    }
    Ok(InstallIdentity { seq, instance })
}

fn parse_install_progress(
    response: &serde_json::Value,
    expected: &InstallIdentity,
) -> Result<InstallProgress, String> {
    if response.get("status").and_then(serde_json::Value::as_u64) != Some(200)
        || response.get("success").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Err(response_error(response, "install status"));
    }
    let seq = response
        .get("seq")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "install status did not include a valid sequence".to_string())?;
    if seq != expected.seq {
        return Err(format!(
            "install status sequence mismatch: expected {}, received {seq}",
            expected.seq
        ));
    }
    let instance = response
        .get("instance")
        .and_then(serde_json::Value::as_str)
        .filter(|instance| !instance.trim().is_empty())
        .ok_or_else(|| "install status did not include a valid instance".to_string())?;
    if instance != expected.instance {
        return Err(format!(
            "install status instance mismatch: expected {}, received {instance}",
            expected.instance
        ));
    }
    let phase = response
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "install status did not include a phase".to_string())?;
    match phase {
        "downloading" | "installing" | "starting" => Ok(InstallProgress::Pending),
        "done" => Ok(InstallProgress::Done),
        "failed" => {
            let message = response
                .get("error")
                .or_else(|| response.get("message"))
                .and_then(serde_json::Value::as_str)
                .filter(|message| !message.trim().is_empty())
                .unwrap_or("installer reported failure")
                .to_string();
            Ok(InstallProgress::Failed(message))
        }
        _ => Err(format!("install status returned unknown phase: {phase}")),
    }
}

fn install_status_with_timeout(
    path: &Path,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let body = br#"{"operation":"install-status"}"#.to_vec();
    let response = request_value_with_timeout(path, body, "install status", timeout)?;
    if response.get("status").and_then(serde_json::Value::as_u64) != Some(200)
        || response.get("success").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Err(response_error(&response, "install status"));
    }
    Ok(response)
}

pub fn runtime_health(path: &Path) -> Result<serde_json::Value, String> {
    let body = br#"{"operation":"runtime-health"}"#.to_vec();
    successful_request(path, body, "runtime health")
}

pub fn ensure_initialized(path: &Path) -> Result<(), String> {
    let body = br#"{"operation":"ensure-initialized"}"#.to_vec();
    submit_request(path, body, "ensure initialized")
}

fn successful_request(
    path: &Path,
    body: Vec<u8>,
    operation: &str,
) -> Result<serde_json::Value, String> {
    let response = request_value(path, body, operation)?;
    if response.get("status").and_then(serde_json::Value::as_u64) != Some(200)
        || response.get("success").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Err(response_error(&response, operation));
    }
    Ok(response)
}

fn submit_request(path: &Path, body: Vec<u8>, operation: &str) -> Result<(), String> {
    successful_request(path, body, operation).map(|_| ())
}

fn response_error(response: &serde_json::Value, operation: &str) -> String {
    response
        .get("error")
        .or_else(|| response.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("private control rejected {operation}"))
}

fn request_value(path: &Path, body: Vec<u8>, operation: &str) -> Result<serde_json::Value, String> {
    request_value_with_timeout(path, body, operation, CLIENT_TIMEOUT)
}

fn request_value_with_timeout(
    path: &Path,
    mut body: Vec<u8>,
    operation: &str,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    if timeout.is_zero() {
        return Err(format!("{operation} request timed out"));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect private control socket: {error}"))?;
    verify_owned_socket(path, &metadata).map_err(|error| error.to_string())?;
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("connect to private control socket: {error}"))?;
    let uid = peer_uid(&stream).map_err(|error| format!("verify private control peer: {error}"))?;
    if uid != unsafe { libc::geteuid() } {
        return Err("private control peer uid is not allowed".into());
    }
    let connected = fs::symlink_metadata(path)
        .map_err(|error| format!("recheck private control socket: {error}"))?;
    verify_owned_socket(path, &connected).map_err(|error| error.to_string())?;
    if connected.dev() != metadata.dev() || connected.ino() != metadata.ino() {
        return Err("private control socket changed during connect".into());
    }
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("set private control read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("set private control write timeout: {error}"))?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err(format!("{operation} request exceeds 4 KiB"));
    }
    body.push(b'\n');
    stream
        .write_all(&body)
        .map_err(|error| format!("write private control request: {error}"))?;
    read_response_with_timeout(&mut stream, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;

    fn socket_path(dir: &TestDir, label: &str) -> PathBuf {
        dir.0.join(label).join("supervisor-control.sock")
    }

    #[test]
    fn socket_is_private_and_active_socket_is_never_unlinked() {
        let dir = TestDir::new("control-permissions");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert!(bind_private_socket(&path).is_err());
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
        drop(listener);
    }

    #[test]
    fn regular_files_and_wrong_mode_sockets_are_not_removed() {
        let dir = TestDir::new("control-untrusted-path");
        let file = socket_path(&dir, "file");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "keep").unwrap();
        assert!(bind_private_socket(&file).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"keep");

        let socket = socket_path(&dir, "wrong-mode");
        fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(bind_private_socket(&socket).is_err());
        assert!(fs::symlink_metadata(&socket)
            .unwrap()
            .file_type()
            .is_socket());
        drop(listener);
    }

    #[test]
    fn client_refuses_a_socket_with_insecure_permissions() {
        let dir = TestDir::new("control-client-permissions");
        let path = socket_path(&dir, "var");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();

        let result = submit_settings(
            &path,
            &SettingsPatch {
                port: None,
                log_level: Some("debug".into()),
            },
        );
        assert!(result.unwrap_err().contains("mode is not 0600"));
        drop(listener);
    }

    #[test]
    fn stale_owned_socket_is_recovered_after_a_crash() {
        let dir = TestDir::new("control-stale-socket");
        let path = socket_path(&dir, "var");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stale = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        drop(stale);
        let listener = bind_private_socket(&path).unwrap();
        assert_eq!(
            fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(listener);
    }

    #[test]
    fn slow_drip_requests_and_responses_respect_total_timeouts() {
        let (mut request_reader, mut request_writer) = UnixStream::pair().unwrap();
        let request_sender = thread::spawn(move || {
            for byte in b"slow request" {
                if request_writer.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        let started = Instant::now();
        assert_eq!(
            read_request_with_timeout(&mut request_reader, Duration::from_millis(50))
                .unwrap_err()
                .0,
            408
        );
        assert!(started.elapsed() < Duration::from_millis(120));
        request_sender.join().unwrap();

        let (mut response_reader, mut response_writer) = UnixStream::pair().unwrap();
        let response_sender = thread::spawn(move || {
            for byte in b"slow response" {
                if response_writer.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        let started = Instant::now();
        assert!(
            read_response_with_timeout(&mut response_reader, Duration::from_millis(50))
                .unwrap_err()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_millis(120));
        response_sender.join().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bad_and_oversized_requests_are_rejected_without_mutation() {
        let dir = TestDir::new("control-bad-request");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let state = dir.state(crate::api::test_support::free_port());
        let runtime = tokio::runtime::Handle::current();
        let mut oversized = vec![b'a'; MAX_REQUEST_BYTES + 1];
        oversized.push(b'\n');
        let cases = [(b"{broken}\n".to_vec(), 400), (oversized, 413)];
        for (body, expected) in cases {
            let client = UnixStream::connect(&path).unwrap();
            let (server, _) = listener.accept().unwrap();
            let state = state.clone();
            let runtime = runtime.clone();
            let worker = thread::spawn(move || handle_connection(server, state, runtime));
            let mut client = client;
            client.write_all(&body).unwrap();
            let response = read_response(&mut client).unwrap();
            worker.join().unwrap();
            assert_eq!(response["status"], expected);
        }
        assert_eq!(state.kernel.log_level(), "info");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn requests_share_the_management_gate_events_and_transaction_rollback() {
        let dir = TestDir::new("control-transaction");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let state = dir.state(crate::api::test_support::free_port());
        let runtime = tokio::runtime::Handle::current();
        fs::write(
            Path::new(&state.var_dir).join("service_port"),
            format!("{}\n", state.kernel.port()),
        )
        .unwrap();
        fs::write(
            Path::new(&state.var_dir).join("qwenpaw-log-level"),
            "info\n",
        )
        .unwrap();
        fs::write(
            &state.ui_config_file,
            serde_json::json!({".url": {"qwenpaw.main": {"port": state.kernel.port().to_string()}}}).to_string(),
        )
        .unwrap();
        let mut events = state.events.subscribe();
        let guard = state.core.begin_operation().unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        let state_for_worker = state.clone();
        let runtime_for_worker = runtime.clone();
        let worker =
            thread::spawn(move || handle_connection(server, state_for_worker, runtime_for_worker));
        let mut client = client;
        client.write_all(b"{\"log_level\":\"debug\"}\n").unwrap();
        let response = read_response(&mut client).unwrap();
        worker.join().unwrap();
        assert_eq!(response["status"], 409);
        drop(guard);

        fs::write(&state.ui_config_file, "invalid json").unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        let state_for_worker = state.clone();
        let runtime_for_worker = runtime.clone();
        let worker =
            thread::spawn(move || handle_connection(server, state_for_worker, runtime_for_worker));
        let mut client = client;
        client.write_all(b"{\"log_level\":\"debug\"}\n").unwrap();
        let response = read_response(&mut client).unwrap();
        worker.join().unwrap();
        assert_eq!(response["status"], 500);
        assert_eq!(state.kernel.log_level(), "info");
        assert_eq!(
            fs::read_to_string(Path::new(&state.var_dir).join("qwenpaw-log-level")).unwrap(),
            "info\n"
        );
        assert!(events.try_recv().is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mirror_mutation_uses_the_management_gate_and_never_writes_on_rejection() {
        let dir = TestDir::new("control-mirror");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let state = dir.state(crate::api::test_support::free_port());
        let runtime = tokio::runtime::Handle::current();
        let mirror_file = Path::new(&state.var_dir).join("pip-mirror");
        fs::write(&mirror_file, "https://old.example/simple\n").unwrap();

        let server_state = state.clone();
        let server = thread::spawn(move || {
            for _ in 0..4 {
                let (stream, _) = listener.accept().unwrap();
                handle_connection(stream, server_state.clone(), runtime.clone());
            }
        });

        assert!(submit_mirror(&path, "https://mirror.example/simple;touch").is_err());
        assert_eq!(
            fs::read_to_string(&mirror_file).unwrap(),
            "https://old.example/simple\n"
        );

        let operation = state.core.begin_operation().unwrap();
        assert!(submit_mirror(&path, "https://mirror.example/simple")
            .unwrap_err()
            .contains("另一个运维操作"));
        drop(operation);
        assert_eq!(
            fs::read_to_string(&mirror_file).unwrap(),
            "https://old.example/simple\n"
        );

        submit_mirror(&path, "https://mirror.example/simple").unwrap();
        assert_eq!(
            fs::read_to_string(&mirror_file).unwrap(),
            "https://mirror.example/simple\n"
        );
        assert_eq!(
            fs::metadata(&mirror_file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        state.install.block_recovery("pending test recovery".into());
        assert!(submit_mirror(&path, "")
            .unwrap_err()
            .contains("恢复尚未完成"));
        assert_eq!(
            fs::read_to_string(&mirror_file).unwrap(),
            "https://mirror.example/simple\n"
        );
        server.join().unwrap();
    }

    #[test]
    fn private_control_parses_installation_and_runtime_operations() {
        for request in [
            br#"{"operation":"install","version":"latest","mirror":"aliyun"}"#.as_slice(),
            br#"{"operation":"install-status"}"#.as_slice(),
            br#"{"operation":"runtime-health"}"#.as_slice(),
            br#"{"operation":"ensure-initialized"}"#.as_slice(),
        ] {
            assert!(
                parse_control_request(request).is_ok(),
                "request was rejected: {}",
                String::from_utf8_lossy(request)
            );
        }
    }

    fn exchange_request(
        listener: &UnixListener,
        state: &AppState,
        runtime: &tokio::runtime::Handle,
        body: &[u8],
    ) -> serde_json::Value {
        let address = listener.local_addr().unwrap();
        let path = address.as_pathname().unwrap();
        let mut client = UnixStream::connect(path).unwrap();
        let (server, _) = listener.accept().unwrap();
        let state = state.clone();
        let runtime = runtime.clone();
        let worker = thread::spawn(move || handle_connection(server, state, runtime));
        let mut request = body.to_vec();
        request.push(b'\n');
        client.write_all(&request).unwrap();
        let response = read_response(&mut client).unwrap();
        worker.join().unwrap();
        response
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn installation_status_uses_snapshot_without_operation_lock() {
        let dir = TestDir::new("control-install-status");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let state = dir.state(crate::api::test_support::free_port());
        let runtime = tokio::runtime::Handle::current();
        let operation = state.core.begin_operation().unwrap();

        let response = exchange_request(
            &listener,
            &state,
            &runtime,
            br#"{"operation":"install-status"}"#,
        );

        drop(operation);
        assert_eq!(response["status"], 200);
        assert_eq!(response["success"], true);
        assert_eq!(response["phase"], "idle");
        assert_eq!(response["seq"], 0);
        assert!(response["instance"].as_str().is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runtime_health_and_ensure_initialized_use_the_shared_runtime_adapter() {
        use crate::api::test_support::install_python;
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new("control-runtime-operations");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let port_reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let state = dir.state(port_reservation.local_addr().unwrap().port());
        let runtime = tokio::runtime::Handle::current();
        let python = install_python(&dir, &state, "");

        let health = exchange_request(
            &listener,
            &state,
            &runtime,
            br#"{"operation":"runtime-health"}"#,
        );
        assert_eq!(health["status"], 200);
        assert_eq!(health["runtime"]["installed"], true);
        assert_eq!(health["runtime"]["version"], "2.3.0");
        assert_eq!(health["runtime"]["running"], false);
        assert_eq!(health["runtime"]["healthy"], false);

        let share = Path::new(&state.share_dir);
        std::fs::write(
            &python,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$@" > '{}/init-argv'
cat > '{}/init-stdin'
printf '%s\n' "$QWENPAW_WORKING_DIR" "$QWENPAW_LOG_LEVEL" "$QWENPAW_AUTH_ENABLED" "$QWENPAW_DEPENDENCY_STATE_DIR" "$PATH" > '{}/init-env'
pwd > '{}/init-cwd'
touch "$QWENPAW_WORKING_DIR/config.json"
"#,
                dir.0.display(),
                dir.0.display(),
                dir.0.display(),
                dir.0.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let initialized = exchange_request(
            &listener,
            &state,
            &runtime,
            br#"{"operation":"ensure-initialized"}"#,
        );

        assert_eq!(initialized["status"], 200);
        assert_eq!(initialized["success"], true);
        assert_eq!(
            std::fs::read_to_string(dir.0.join("init-argv")).unwrap(),
            "-m\nqwenpaw\ninit\n--defaults\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("init-stdin")).unwrap(),
            "Yes\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("init-cwd"))
                .unwrap()
                .trim(),
            share.to_string_lossy()
        );
        assert!(share.join("config.json").exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn private_install_busy_keeps_http_compatible_acceptance_shape() {
        use crate::api::test_support::install_python;

        let dir = TestDir::new("control-install-busy");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let state = dir.state(crate::api::test_support::free_port());
        let runtime = tokio::runtime::Handle::current();
        let root = serde_json::to_string(&dir.0.to_string_lossy()).unwrap();
        install_python(
            &dir,
            &state,
            &format!(
                "import pathlib, shutil, sys, time\nroot = pathlib.Path({root})\nif sys.argv[1:3] == ['-m', 'pip']:\n    (root / 'bootstrap-started').touch()\n    shutil.copy2(sys.argv[0], pathlib.Path(sys.argv[0]).with_name('uv'))\n    sys.exit(0)\nif pathlib.Path(sys.argv[0]).name == 'uv' and sys.argv[1:3] == ['pip', 'install']:\n    (root / 'uv-started').touch()\n    while not (root / 'release-uv').exists():\n        time.sleep(0.01)\n    sys.exit(1)\n"
            ),
        );
        assert!(state.install.try_start(
            format!("{}/venv", state.home_dir),
            state.kernel.port(),
            crate::install::InstallChannel::Latest,
            String::new(),
            format!("{}/pip.log", state.var_dir),
            format!("{}/qwenpaw-version", state.var_dir),
        ));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !dir.0.join("uv-started").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            dir.0.join("bootstrap-started").exists(),
            "bootstrap did not start"
        );
        assert!(dir.0.join("uv-started").exists(), "uv did not start");
        let seq = state.install.snapshot().seq;

        let response = exchange_request(
            &listener,
            &state,
            &runtime,
            br#"{"operation":"install","version":"latest","mirror":"aliyun"}"#,
        );

        assert_eq!(response["status"], 200);
        assert_eq!(response["success"], false);
        assert_eq!(response["seq"], seq);
        assert!(response["instance"].as_str().is_some());
        std::fs::write(dir.0.join("release-uv"), "release").unwrap();
        assert!(state.install.shutdown(Duration::from_secs(3)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn private_install_started_includes_its_instance_identity() {
        use crate::api::test_support::install_python;

        let dir = TestDir::new("control-install-started");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let state = dir.state(crate::api::test_support::free_port());
        let runtime = tokio::runtime::Handle::current();
        install_python(
            &dir,
            &state,
            "if sys.argv[1:3] == ['-m', 'pip']:\n    sys.exit(1)\n",
        );

        let response = exchange_request(
            &listener,
            &state,
            &runtime,
            br#"{"operation":"install","version":"latest","mirror":"aliyun"}"#,
        );
        let identity_is_valid = response["seq"].as_u64().is_some_and(|seq| seq > 0)
            && response["instance"]
                .as_str()
                .is_some_and(|instance| !instance.trim().is_empty());
        let installation_stopped = state.install.shutdown(Duration::from_secs(3));

        assert!(installation_stopped);
        assert_eq!(response["status"], 200);
        assert_eq!(response["success"], true);
        assert_eq!(response["running"], true);
        assert!(identity_is_valid);
    }

    #[test]
    fn platform_install_client_waits_for_its_matching_sequence_to_finish() {
        let dir = TestDir::new("control-platform-install");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let server = thread::spawn(move || {
            let responses = [
                serde_json::json!({"status": 200, "success": true, "seq": 41, "instance": "worker-a", "running": true}),
                serde_json::json!({"status": 200, "success": true, "seq": 41, "instance": "worker-a", "phase": "installing", "upgrading": true}),
                serde_json::json!({"status": 200, "success": true, "seq": 41, "instance": "worker-a", "phase": "done", "upgrading": false}),
            ];
            for (index, response) in responses.iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                let body = read_request(&mut stream).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                if index == 0 {
                    assert_eq!(request["operation"], "install");
                    assert_eq!(request["version"], "2.3.0");
                    assert_eq!(request["mirror"], "aliyun");
                } else {
                    assert_eq!(request["operation"], "install-status");
                }
                write_response(&mut stream, response).unwrap();
            }
        });

        install_and_wait_with_timeout(
            &path,
            "2.3.0",
            "aliyun",
            Duration::from_secs(2),
            Duration::from_millis(1),
        )
        .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn platform_install_client_rejects_busy_acceptance_without_polling() {
        let dir = TestDir::new("control-platform-busy");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let body = read_request(&mut stream).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["operation"], "install");
            write_response(
                &mut stream,
                &serde_json::json!({
                    "status": 200,
                    "success": false,
                    "seq": 40,
                    "running": true,
                    "message": "安装正在进行中"
                }),
            )
            .unwrap();
        });

        let result = install_and_wait_with_timeout(
            &path,
            "2.3.0",
            "aliyun",
            Duration::from_secs(2),
            Duration::from_millis(1),
        );
        assert!(result.unwrap_err().contains("安装正在进行中"));
        server.join().unwrap();
    }

    #[test]
    fn platform_install_progress_rejects_mismatched_sequences_and_unknown_phases() {
        let expected = InstallIdentity {
            seq: 42,
            instance: "worker-a".into(),
        };
        assert!(parse_install_acceptance(&serde_json::json!({
            "status": 200,
            "success": false,
            "seq": 42,
            "running": true
        }))
        .is_err());
        assert!(parse_install_acceptance(&serde_json::json!({
            "status": 200,
            "success": true,
            "running": true
        }))
        .is_err());
        assert!(parse_install_progress(
            &serde_json::json!({"status": 200, "success": true, "seq": 43, "instance": "worker-a", "phase": "done"}),
            &expected
        )
        .is_err());
        assert!(parse_install_progress(
            &serde_json::json!({"status": 200, "success": true, "seq": 42, "instance": "worker-a", "phase": "unknown"}),
            &expected
        )
        .is_err());
        assert!(parse_install_progress(
            &serde_json::json!({"status": 200, "success": true, "seq": 42, "instance": "worker-b", "phase": "failed", "error": "foreign failure"}),
            &expected
        )
        .is_err());
        assert!(matches!(
            parse_install_progress(
                &serde_json::json!({"status": 200, "success": true, "seq": 42, "instance": "worker-a", "phase": "downloading"}),
                &expected
            ),
            Ok(InstallProgress::Pending)
        ));
        assert!(matches!(
            parse_install_progress(
                &serde_json::json!({"status": 200, "success": true, "seq": 42, "instance": "worker-a", "phase": "done"}),
                &expected
            ),
            Ok(InstallProgress::Done)
        ));
        assert!(matches!(
            parse_install_progress(
                &serde_json::json!({"status": 200, "success": true, "seq": 42, "instance": "worker-a", "phase": "failed", "error": "pip failed"}),
                &expected
            ),
            Ok(InstallProgress::Failed(message)) if message == "pip failed"
        ));
    }

    #[test]
    fn platform_install_acceptance_requires_instance() {
        assert!(parse_install_acceptance(&serde_json::json!({
            "status": 200,
            "success": true,
            "seq": 42,
            "running": true
        }))
        .is_err());
    }

    #[test]
    fn platform_install_progress_requires_instance() {
        let expected = InstallIdentity {
            seq: 42,
            instance: "worker-a".into(),
        };
        assert!(parse_install_progress(
            &serde_json::json!({"status": 200, "success": true, "seq": 42, "phase": "done"}),
            &expected
        )
        .is_err());
    }

    #[test]
    fn platform_install_client_rejects_terminal_status_from_another_instance() {
        let dir = TestDir::new("control-platform-cross-instance");
        let path = socket_path(&dir, "var");
        let listener = bind_private_socket(&path).unwrap();
        let server = thread::spawn(move || {
            for response in [
                serde_json::json!({"status": 200, "success": true, "seq": 41, "instance": "worker-a", "running": true}),
                serde_json::json!({"status": 200, "success": true, "seq": 41, "instance": "worker-b", "phase": "done", "upgrading": false}),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = read_request(&mut stream).unwrap();
                write_response(&mut stream, &response).unwrap();
            }
        });

        let result = install_and_wait_with_timeout(
            &path,
            "2.3.0",
            "aliyun",
            Duration::from_secs(2),
            Duration::from_millis(1),
        );
        assert!(result.is_err());
        server.join().unwrap();
    }
}
