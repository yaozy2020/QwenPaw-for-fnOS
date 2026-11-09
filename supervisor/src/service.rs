use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{cli::ServiceAction, config::Config};

const LOCK_WAIT: Duration = Duration::from_secs(10);
const START_WAIT: Duration = Duration::from_secs(30);
/// External stop budget: 15s for in-flight work plus about 11.5s for kernel TERM/cleanup, with 3.5s margin.
/// The actual fnOS outer grace may be shorter and remains subject to on-device measurement.
const STOP_GRACE: Duration = Duration::from_secs(30);
const KILL_WAIT: Duration = Duration::from_secs(2);
const HTTP_TIMEOUT: Duration = Duration::from_millis(500);

pub struct LifecycleLock {
    _file: File,
}

#[derive(Debug, PartialEq, Eq)]
enum IdentityState {
    Match,
    Gone,
    Mismatch,
    Unverifiable(String),
}

#[derive(Debug)]
enum ManagerStatus {
    Ready { pid: i32, starttime: u64 },
    NotRunning(String),
    Abnormal(String),
}

impl ManagerStatus {
    fn state_name(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::NotRunning(_) => "not_running",
            Self::Abnormal(_) => "abnormal",
        }
    }

    fn exit_code(&self) -> i32 {
        match self {
            Self::Ready { .. } => 0,
            Self::NotRunning(_) => 3,
            Self::Abnormal(_) => 1,
        }
    }
}

pub fn parse_starttime(stat: &str) -> Option<u64> {
    let end = stat.rfind(')')?;
    let fields: Vec<&str> = stat.get(end + 2..)?.split_whitespace().collect();
    fields.get(19)?.parse().ok()
}

fn parse_process_state(stat: &str) -> Option<char> {
    let end = stat.rfind(')')?;
    stat.get(end + 2..)?.chars().next()
}

fn canonical_executable(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    let value = value.strip_suffix(" (deleted)").unwrap_or(&value);
    let path = Path::new(value);
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn executable_matches(expected: &Path, actual: &Path) -> bool {
    let actual = actual.to_string_lossy();
    let actual = actual.strip_suffix(" (deleted)").unwrap_or(&actual);
    canonical_executable(expected) == Path::new(actual)
}

fn check_identity(pid: i32, starttime: u64, expected_exe: &Path) -> IdentityState {
    if pid <= 1 {
        return IdentityState::Mismatch;
    }
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return IdentityState::Gone,
        Err(error) => return IdentityState::Unverifiable(format!("read process stat: {error}")),
    };
    let Some(actual_starttime) = parse_starttime(&stat) else {
        return IdentityState::Unverifiable("process stat has no valid starttime".into());
    };
    if actual_starttime != starttime
        || !matches!(
            parse_process_state(&stat),
            Some('R' | 'S' | 'D' | 'T' | 't')
        )
    {
        return IdentityState::Mismatch;
    }
    let exe = match fs::read_link(format!("/proc/{pid}/exe")) {
        Ok(exe) => exe,
        Err(error) => {
            return IdentityState::Unverifiable(format!("read process executable: {error}"));
        }
    };
    if executable_matches(expected_exe, &exe) {
        IdentityState::Match
    } else {
        IdentityState::Mismatch
    }
}

pub fn acquire_lifecycle_lock(path: &Path, timeout: Duration) -> io::Result<LifecycleLock> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    let fd = file.as_raw_fd();
    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if fd_flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }

    let deadline = Instant::now() + timeout;
    loop {
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(LifecycleLock { _file: file });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock && error.raw_os_error() != Some(libc::EINTR) {
            return Err(error);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for service lifecycle lock",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn read_markers(cfg: &Config) -> Result<Option<(i32, u64)>, String> {
    let pid_path = Path::new(&cfg.supervisor_pid_file);
    let pid_contents = match fs::read_to_string(pid_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read supervisor pid: {error}")),
    };
    let pid = pid_contents
        .trim()
        .parse::<i32>()
        .map_err(|_| "supervisor PID file is malformed".to_string())?;
    if pid <= 1 {
        return Err("supervisor PID is outside the valid range".into());
    }
    let starttime_path = format!("{}.starttime", cfg.supervisor_pid_file);
    let starttime = fs::read_to_string(&starttime_path)
        .map_err(|error| format!("read supervisor starttime: {error}"))?
        .trim()
        .parse::<u64>()
        .map_err(|_| "supervisor starttime file is malformed".to_string())?;
    Ok(Some((pid, starttime)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ShutdownPhase {
    Running,
    Clean,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ShutdownGeneration {
    schema: u32,
    pid: i32,
    starttime: u64,
    state: ShutdownPhase,
}

fn shutdown_generation_path(cfg: &Config) -> PathBuf {
    Path::new(&cfg.var_dir).join("shutdown-generation.json")
}

pub(crate) fn mark_shutdown_generation(
    cfg: &Config,
    pid: i32,
    starttime: u64,
    clean: bool,
) -> io::Result<()> {
    if pid <= 1 || starttime == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid shutdown generation identity",
        ));
    }
    let generation = ShutdownGeneration {
        schema: 1,
        pid,
        starttime,
        state: if clean {
            ShutdownPhase::Clean
        } else {
            ShutdownPhase::Running
        },
    };
    let encoded = serde_json::to_string(&generation)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    crate::install::atomic_write_0600(
        &shutdown_generation_path(cfg).to_string_lossy(),
        &format!("{encoded}\n"),
    )
}

fn read_shutdown_generation(
    cfg: &Config,
    pid: i32,
    starttime: u64,
) -> Result<Option<ShutdownPhase>, String> {
    let path = shutdown_generation_path(cfg);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read shutdown generation: {error}")),
    };
    let generation: ShutdownGeneration = serde_json::from_str(&contents)
        .map_err(|error| format!("shutdown generation is malformed: {error}"))?;
    if generation.schema != 1
        || generation.pid != pid
        || generation.starttime != starttime
        || generation.pid <= 1
        || generation.starttime == 0
    {
        return Err("shutdown generation identity or schema does not match supervisor".into());
    }
    Ok(Some(generation.state))
}

#[derive(serde::Serialize)]
struct GatewayStatus {
    state: &'static str,
    error: Option<String>,
}

impl GatewayStatus {
    fn ready(&self) -> bool {
        matches!(self.state, "ready" | "disabled")
    }
}

fn gateway_status(cfg: &Config, pid: Option<i32>) -> GatewayStatus {
    let result = match crate::gateway::socket_path_from_config(cfg) {
        Ok(None) => {
            return GatewayStatus {
                state: "disabled",
                error: None,
            }
        }
        Ok(Some(path)) => match pid {
            Some(pid) => crate::gateway::readiness(&path, &cfg.token_file, pid),
            None => Ok(false),
        },
        Err(error) => Err(error),
    };
    match result {
        Ok(true) => GatewayStatus {
            state: "ready",
            error: None,
        },
        Ok(false) => GatewayStatus {
            state: "not_ready",
            error: Some("gateway did not return a verified HTTP 200".into()),
        },
        Err(error) => GatewayStatus {
            state: "abnormal",
            error: Some(error),
        },
    }
}

fn inspect(cfg: &Config, expected_exe: &Path) -> ManagerStatus {
    let manager = inspect_with_probe(cfg, expected_exe, http_status_ready);
    if let ManagerStatus::Ready { pid, .. } = &manager {
        let gateway = gateway_status(cfg, Some(*pid));
        if !gateway.ready() {
            return ManagerStatus::Abnormal(format!(
                "gateway {}: {}",
                gateway.state,
                gateway.error.unwrap_or_default()
            ));
        }
    }
    manager
}

fn inspect_readonly(cfg: &Config, expected_exe: &Path) -> ManagerStatus {
    inspect_with_probe(cfg, expected_exe, listener_ready)
}

fn inspect_with_probe(
    cfg: &Config,
    expected_exe: &Path,
    probe: fn(&Config) -> Result<bool, String>,
) -> ManagerStatus {
    let (pid, starttime) = match read_markers(cfg) {
        Ok(Some(markers)) => markers,
        Ok(None) => return ManagerStatus::NotRunning("supervisor PID is not registered".into()),
        Err(error) => return ManagerStatus::Abnormal(error),
    };
    match check_identity(pid, starttime, expected_exe) {
        IdentityState::Gone | IdentityState::Mismatch => {
            return ManagerStatus::NotRunning(
                "registered process identity is no longer present".into(),
            );
        }
        IdentityState::Unverifiable(error) => return ManagerStatus::Abnormal(error),
        IdentityState::Match => {}
    }
    match probe(cfg) {
        Ok(true) => ManagerStatus::Ready { pid, starttime },
        Ok(false) => ManagerStatus::Abnormal("supervisor HTTP listener is not ready".into()),
        Err(error) => ManagerStatus::Abnormal(error),
    }
}

fn listener_ready(cfg: &Config) -> Result<bool, String> {
    let address = SocketAddr::from(([127, 0, 0, 1], cfg.listen_port));
    let mut stream = match TcpStream::connect_timeout(&address, HTTP_TIMEOUT) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(format!("connect to supervisor listener: {error}")),
    };
    stream
        .set_read_timeout(Some(HTTP_TIMEOUT))
        .map_err(|error| format!("set listener read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(HTTP_TIMEOUT))
        .map_err(|error| format!("set listener write timeout: {error}"))?;
    write!(
        stream,
        "GET /status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        cfg.listen_port
    )
    .map_err(|error| format!("probe supervisor listener: {error}"))?;
    let mut response = Vec::new();
    stream
        .take(4096)
        .read_to_end(&mut response)
        .map_err(|error| format!("read supervisor listener probe: {error}"))?;
    let response = String::from_utf8_lossy(&response);
    Ok(response
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 401 ")))
}

fn http_status_ready(cfg: &Config) -> Result<bool, String> {
    let token = fs::read_to_string(&cfg.token_file)
        .map_err(|error| format!("read supervisor token: {error}"))?;
    let token = token.trim();
    if token.len() < 32 || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("supervisor token is missing or invalid".into());
    }
    let address = SocketAddr::from(([127, 0, 0, 1], cfg.listen_port));
    let mut stream = match TcpStream::connect_timeout(&address, HTTP_TIMEOUT) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(format!("connect to supervisor /status: {error}")),
    };
    stream
        .set_read_timeout(Some(HTTP_TIMEOUT))
        .map_err(|error| format!("set /status read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(HTTP_TIMEOUT))
        .map_err(|error| format!("set /status write timeout: {error}"))?;
    write!(
        stream,
        "GET /status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Supervisor-Token: {}\r\nConnection: close\r\n\r\n",
        cfg.listen_port, token
    )
    .map_err(|error| format!("request supervisor /status: {error}"))?;
    let mut response = Vec::new();
    stream
        .take(64 * 1024)
        .read_to_end(&mut response)
        .map_err(|error| format!("read supervisor /status: {error}"))?;
    let response = String::from_utf8_lossy(&response);
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return Ok(false);
    };
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        return Ok(false);
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return Ok(false);
    };
    Ok(json.get("success").and_then(serde_json::Value::as_bool) == Some(true))
}

fn lifecycle_lock(cfg: &Config) -> Result<LifecycleLock, String> {
    fs::create_dir_all(&cfg.var_dir)
        .map_err(|error| format!("create supervisor var directory: {error}"))?;
    let path = Path::new(&cfg.var_dir).join(".qwenpaw-lifecycle.lock");
    acquire_lifecycle_lock(&path, LOCK_WAIT)
        .map_err(|error| format!("acquire lifecycle lock: {error}"))
}

pub(crate) fn with_lifecycle_lock<T>(
    cfg: &Config,
    operation: impl FnOnce(&Path) -> T,
) -> Result<T, String> {
    let _lock = lifecycle_lock(cfg)?;
    Ok(operation(&cfg.supervisor_binary))
}

pub(crate) fn stop_for_uninstall(cfg: &Config, expected_exe: &Path) -> Result<(), String> {
    stop_locked(cfg, expected_exe)
}

pub fn run(action: ServiceAction, cfg: &Config) -> i32 {
    match action {
        ServiceAction::Status => run_status(cfg),
        ServiceAction::Start => run_mutation(cfg, start_locked),
        ServiceAction::Stop => run_mutation(cfg, stop_locked),
        ServiceAction::Restart => run_mutation(cfg, restart_locked),
    }
}

fn run_status(cfg: &Config) -> i32 {
    let status = inspect_readonly(cfg, &cfg.supervisor_binary);
    match &status {
        ManagerStatus::Ready { pid, .. } => println!("management service ready (pid {pid})"),
        ManagerStatus::NotRunning(reason) => println!("management service not running: {reason}"),
        ManagerStatus::Abnormal(reason) => eprintln!("management service abnormal: {reason}"),
    }
    let pid = match &status {
        ManagerStatus::Ready { pid, .. } => Some(*pid),
        _ => None,
    };
    let gateway = gateway_status(cfg, pid);
    println!(
        "gateway {}{}",
        gateway.state,
        gateway
            .error
            .as_ref()
            .map(|error| format!(": {error}"))
            .unwrap_or_default()
    );
    if !gateway.ready() && status.exit_code() == 0 {
        1
    } else {
        status.exit_code()
    }
}

fn run_mutation(cfg: &Config, operation: fn(&Config, &Path) -> Result<(), String>) -> i32 {
    let _lock = match lifecycle_lock(cfg) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    match operation(cfg, &cfg.supervisor_binary) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn clear_stale_registration(cfg: &Config, expected_exe: &Path) -> Result<(), String> {
    let Some((pid, starttime)) = read_markers(cfg)? else {
        return Ok(());
    };
    match check_identity(pid, starttime, expected_exe) {
        IdentityState::Gone => remove_matching_markers(cfg, pid, starttime),
        IdentityState::Mismatch => {
            Err("stale supervisor identity mismatches; preserving registration".into())
        }
        IdentityState::Match => {
            Err("supervisor became active before stale registration cleanup".into())
        }
        IdentityState::Unverifiable(error) => Err(error),
    }
}

fn ensure_target_executable(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "target supervisor binary is unavailable {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!(
            "target supervisor binary is not executable: {}",
            path.display()
        ));
    }
    Ok(())
}

fn start_locked(cfg: &Config, expected_exe: &Path) -> Result<(), String> {
    ensure_target_executable(expected_exe)?;
    match inspect(cfg, expected_exe) {
        ManagerStatus::Ready { pid, .. } => {
            println!("management service already ready (pid {pid})");
            return Ok(());
        }
        ManagerStatus::Abnormal(error) => return Err(format!("refusing to start: {error}")),
        ManagerStatus::NotRunning(_) => {
            if crate::gateway::socket_path_from_config(cfg)?.is_some() {
                clear_stale_registration(cfg, expected_exe)?;
            }
        }
    }

    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&cfg.supervisor_log)
        .map_err(|error| format!("open supervisor log: {error}"))?;
    let stdout = log
        .try_clone()
        .map_err(|error| format!("duplicate supervisor log handle: {error}"))?;
    let mut child = Command::new(expected_exe)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("spawn supervisor serve: {error}"))?;
    let pid = i32::try_from(child.id()).map_err(|_| "supervisor PID exceeds i32".to_string())?;
    let deadline = Instant::now() + START_WAIT;
    let starttime = match wait_for_process_starttime(pid, deadline) {
        Ok(starttime) => starttime,
        Err(error) => {
            let cleanup = match child.try_wait() {
                Ok(Some(_)) => Ok(()),
                Ok(None) => child.kill().and_then(|()| child.wait().map(|_| ())),
                Err(wait_error) => Err(wait_error),
            }
            .map_err(|cleanup_error| format!("child cleanup failed: {cleanup_error}"));
            return Err(match cleanup {
                Ok(()) => error,
                Err(cleanup_error) => format!("{error}; {cleanup_error}"),
            });
        }
    };
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("check supervisor child: {error}"))?
        {
            cleanup_registration(cfg, pid, starttime)?;
            return Err(format!("supervisor exited before ready (status {status})"));
        }
        match read_markers(cfg) {
            Ok(Some((registered_pid, registered_starttime)))
                if registered_pid == pid && registered_starttime == starttime =>
            {
                match check_identity(pid, starttime, expected_exe) {
                    IdentityState::Match => {
                        if http_status_ready(cfg).unwrap_or(false)
                            && gateway_status(cfg, Some(pid)).ready()
                        {
                            println!("management service started (pid {pid})");
                            return Ok(());
                        }
                    }
                    IdentityState::Gone | IdentityState::Mismatch => {
                        return Err("supervisor process identity changed before ready".into());
                    }
                    IdentityState::Unverifiable(error) => return Err(error),
                }
            }
            Ok(Some((other_pid, _))) if other_pid != pid => {
                return Err("a different supervisor PID registered while starting".into());
            }
            Ok(None) | Ok(Some(_)) | Err(_) => {}
        }
        if Instant::now() >= deadline {
            let cleanup = stop_child(&mut child, pid, starttime, expected_exe)
                .and_then(|()| cleanup_registration(cfg, pid, starttime));
            return Err(match cleanup {
                Ok(()) => "supervisor readiness timed out".into(),
                Err(error) => format!("supervisor readiness timed out; cleanup failed: {error}"),
            });
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_process_starttime(pid: i32, deadline: Instant) -> Result<u64, String> {
    loop {
        if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
            if let Some(starttime) = parse_starttime(&stat) {
                return Ok(starttime);
            }
        }
        if Instant::now() >= deadline {
            return Err("could not read spawned supervisor starttime".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn cleanup_registration(cfg: &Config, pid: i32, starttime: u64) -> Result<(), String> {
    match read_markers(cfg)? {
        Some((registered_pid, registered_starttime))
            if registered_pid == pid && registered_starttime == starttime =>
        {
            remove_matching_markers(cfg, pid, starttime)
        }
        Some(_) => Ok(()),
        None => {
            let path = format!("{}.starttime", cfg.supervisor_pid_file);
            if fs::read_to_string(&path)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                == Some(starttime)
            {
                remove_file_if_exists(path)?;
            }
            Ok(())
        }
    }
}

fn stop_locked(cfg: &Config, expected_exe: &Path) -> Result<(), String> {
    let markers = read_markers(cfg)?;
    let Some((pid, starttime)) = markers else {
        remove_file_if_exists(format!("{}.starttime", cfg.supervisor_pid_file))?;
        println!("management service is not running");
        return Ok(());
    };
    let generation = read_shutdown_generation(cfg, pid, starttime)?;
    match check_identity(pid, starttime, expected_exe) {
        IdentityState::Gone => {
            if generation.is_none() {
                return Err(
                    "shutdown generation is missing for stale supervisor registration".into(),
                );
            }
            remove_matching_markers(cfg, pid, starttime)?;
            remove_matching_shutdown_generation(cfg, pid, starttime)?;
            println!("management service is not running (stale registration cleared)");
            return Ok(());
        }
        IdentityState::Mismatch => {
            return Err("process identity mismatch; refusing to signal PID".into())
        }
        IdentityState::Unverifiable(error) => return Err(error),
        IdentityState::Match => {}
    }
    let forced_kill = terminate_identity(pid, starttime, expected_exe)?;
    let clean_generation = matches!(
        read_shutdown_generation(cfg, pid, starttime)?,
        Some(ShutdownPhase::Clean)
    );
    validate_stop_result(forced_kill, clean_generation)?;
    remove_matching_markers(cfg, pid, starttime)?;
    println!("management service stopped (pid {pid})");
    Ok(())
}

fn restart_locked(cfg: &Config, expected_exe: &Path) -> Result<(), String> {
    stop_locked(cfg, expected_exe)?;
    start_locked(cfg, expected_exe)
}

fn terminate_identity(pid: i32, starttime: u64, expected_exe: &Path) -> Result<bool, String> {
    match check_identity(pid, starttime, expected_exe) {
        IdentityState::Match => {}
        IdentityState::Gone | IdentityState::Mismatch => return Ok(false),
        IdentityState::Unverifiable(error) => return Err(error),
    }
    signal_verified(pid, starttime, expected_exe, libc::SIGTERM)?;
    if wait_for_exit(pid, starttime, expected_exe, STOP_GRACE)? {
        return Ok(false);
    }
    match check_identity(pid, starttime, expected_exe) {
        IdentityState::Match => {}
        IdentityState::Gone | IdentityState::Mismatch => return Ok(false),
        IdentityState::Unverifiable(error) => return Err(error),
    }
    signal_verified(pid, starttime, expected_exe, libc::SIGKILL)?;
    if wait_for_exit(pid, starttime, expected_exe, KILL_WAIT)? {
        Ok(true)
    } else {
        Err("supervisor did not exit after SIGKILL".into())
    }
}

fn validate_stop_result(forced_kill: bool, clean_generation: bool) -> Result<(), String> {
    if forced_kill {
        return Err("supervisor required SIGKILL; shutdown is not clean".into());
    }
    if !clean_generation {
        return Err("supervisor shutdown generation did not record a clean stop".into());
    }
    Ok(())
}

fn signal_verified(
    pid: i32,
    starttime: u64,
    expected_exe: &Path,
    signal: i32,
) -> Result<(), String> {
    match check_identity(pid, starttime, expected_exe) {
        IdentityState::Match => {}
        IdentityState::Gone | IdentityState::Mismatch => return Ok(()),
        IdentityState::Unverifiable(error) => return Err(error),
    }
    let result = unsafe { libc::kill(pid, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!(
            "send signal {signal} to verified supervisor {pid}: {error}"
        ))
    }
}

fn wait_for_exit(
    pid: i32,
    starttime: u64,
    expected_exe: &Path,
    timeout: Duration,
) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match check_identity(pid, starttime, expected_exe) {
            IdentityState::Match => {}
            IdentityState::Gone | IdentityState::Mismatch => return Ok(true),
            IdentityState::Unverifiable(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn stop_child(
    child: &mut Child,
    pid: i32,
    starttime: u64,
    expected_exe: &Path,
) -> Result<(), String> {
    let forced_kill = terminate_identity(pid, starttime, expected_exe)?;
    child
        .wait()
        .map_err(|error| format!("reap failed supervisor child: {error}"))?;
    if forced_kill {
        Err("supervisor required SIGKILL during startup cleanup".into())
    } else {
        Ok(())
    }
}

fn remove_matching_markers(cfg: &Config, pid: i32, starttime: u64) -> Result<(), String> {
    if read_markers(cfg)? != Some((pid, starttime)) {
        return Ok(());
    }
    remove_file_if_exists(Path::new(&cfg.supervisor_pid_file))?;
    remove_file_if_exists(format!("{}.starttime", cfg.supervisor_pid_file))?;
    Ok(())
}

fn remove_matching_shutdown_generation(
    cfg: &Config,
    pid: i32,
    starttime: u64,
) -> Result<(), String> {
    if read_shutdown_generation(cfg, pid, starttime)?.is_some() {
        remove_file_if_exists(shutdown_generation_path(cfg))?;
    }
    Ok(())
}

fn remove_file_if_exists(path: impl AsRef<Path>) -> Result<(), String> {
    match fs::remove_file(path.as_ref()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.as_ref().display())),
    }
}

pub fn doctor_json(cfg: &Config) -> String {
    let manager = inspect_readonly(cfg, &cfg.supervisor_binary);
    let (pid, starttime, error) = match &manager {
        ManagerStatus::Ready { pid, starttime } => (Some(*pid), Some(*starttime), None),
        ManagerStatus::NotRunning(reason) | ManagerStatus::Abnormal(reason) => {
            (None, None, Some(reason.as_str()))
        }
    };
    serde_json::json!({
        "schema": 1,
        "gateway": gateway_status(cfg, pid),
        "manager": {
            "state": manager.state_name(),
            "pid": pid,
            "starttime": starttime,
            "error": error
        },
        "paths": {
            "var_dir_exists": Path::new(&cfg.var_dir).is_dir(),
            "token_file_exists": Path::new(&cfg.token_file).is_file(),
            "supervisor_log_exists": Path::new(&cfg.supervisor_log).is_file(),
            "pid_file_exists": Path::new(&cfg.supervisor_pid_file).is_file(),
            "settings_recovery_pending": Path::new(&cfg.var_dir).join("settings-transaction.json").exists()
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(label: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "qwenpaw-service-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn service_stop_decision_rejects_forced_kill_and_unclean_generation() {
        assert!(super::validate_stop_result(false, true).is_ok());
        assert!(super::validate_stop_result(true, true).is_err());
        assert!(super::validate_stop_result(false, false).is_err());
    }

    #[test]
    fn start_and_stop_clean_matching_registration_for_crashed_supervisor() {
        let root = test_path("crashed-supervisor");
        let var = root.join("var");
        fs::create_dir_all(&var).unwrap();
        let pid = i32::MAX;
        let starttime = 123_456;
        let pid_path = var.join("supervisor.pid");
        let starttime_path = var.join("supervisor.pid.starttime");
        fs::write(&pid_path, format!("{pid}\n")).unwrap();
        fs::write(&starttime_path, format!("{starttime}\n")).unwrap();
        let cfg = Config {
            var_dir: var.to_string_lossy().into_owned(),
            etc_dir: root.join("etc").to_string_lossy().into_owned(),
            home_dir: root.join("home").to_string_lossy().into_owned(),
            share_dir: root.join("share").to_string_lossy().into_owned(),
            ui_config_file: root.join("ui/config").to_string_lossy().into_owned(),
            supervisor_binary: std::env::current_exe().unwrap(),
            kernel_venv: root.join("home/venv").to_string_lossy().into_owned(),
            kernel_pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
            kernel_log: var.join("qwenpaw.log").to_string_lossy().into_owned(),
            kernel_port: crate::api::test_support::free_port(),
            web_proxy_port: None,
            kernel_log_level: "info".into(),
            token_file: var.join("supervisor-token").to_string_lossy().into_owned(),
            supervisor_pid_file: pid_path.to_string_lossy().into_owned(),
            supervisor_log: var.join("supervisor.log").to_string_lossy().into_owned(),
            listen_port: crate::api::test_support::free_port(),
            auto_start: false,
            gateway_socket: None,
        };
        mark_shutdown_generation(&cfg, pid, starttime, false).unwrap();
        let expected_exe = std::env::current_exe().unwrap();
        clear_stale_registration(&cfg, &expected_exe).unwrap();
        assert!(
            !pid_path.exists(),
            "start should clear a confirmed gone registration"
        );
        assert!(!starttime_path.exists());
        fs::write(&pid_path, format!("{pid}\n")).unwrap();
        fs::write(&starttime_path, format!("{starttime}\n")).unwrap();

        let result = stop_locked(&cfg, &expected_exe);

        assert!(
            result.is_ok(),
            "stale crashed registration should be cleaned: {result:?}"
        );
        assert!(!pid_path.exists());
        assert!(!starttime_path.exists());
        assert!(!shutdown_generation_path(&cfg).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stop_grace_covers_internal_shutdown_budget_with_margin() {
        const WORKER_JOIN_BUDGET: Duration = Duration::from_secs(15);
        const KERNEL_TERM_AND_CLEANUP_BUDGET: Duration = Duration::from_millis(11_500);
        const SHUTDOWN_MARGIN: Duration = Duration::from_millis(3_500);

        assert_eq!(STOP_GRACE, Duration::from_secs(30));
        assert!(
            STOP_GRACE >= WORKER_JOIN_BUDGET + KERNEL_TERM_AND_CLEANUP_BUDGET + SHUTDOWN_MARGIN
        );
    }

    #[test]
    fn parses_starttime_after_process_name_with_parentheses() {
        let mut fields = vec!["0"; 19];
        fields[18] = "987654";
        let stat = format!("42 (qwen ) paw) S {}", fields.join(" "));
        assert_eq!(parse_starttime(&stat), Some(987654));
    }

    #[test]
    fn verifies_current_process_identity_and_rejects_changed_starttime_or_executable() {
        let pid = std::process::id() as i32;
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let starttime = parse_starttime(&stat).unwrap();
        let executable = std::env::current_exe().unwrap();
        assert_eq!(
            check_identity(pid, starttime, &executable),
            IdentityState::Match
        );
        assert_eq!(
            check_identity(pid, starttime + 1, &executable),
            IdentityState::Mismatch
        );
        assert_eq!(
            check_identity(pid, starttime, Path::new("/tmp/unrelated-supervisor")),
            IdentityState::Mismatch
        );
    }

    #[test]
    fn doctor_checks_listener_without_requesting_mutating_status_snapshot() {
        let root = test_path("doctor-listener");
        let var = root.join("var");
        fs::create_dir_all(&var).unwrap();
        let pid = std::process::id() as i32;
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let starttime = parse_starttime(&stat).unwrap();
        let pid_path = var.join("supervisor.pid");
        let starttime_path = var.join("supervisor.pid.starttime");
        fs::write(&pid_path, format!("{pid}\n")).unwrap();
        fs::write(&starttime_path, format!("{starttime}\n")).unwrap();
        let kernel_pid = var.join("kernel.pid");
        let token_file = var.join("supervisor-token");
        fs::write(&token_file, "x".repeat(32)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let kernel_pid_for_server = kernel_pid.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(300)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 256];
            loop {
                let size = stream.read(&mut chunk).unwrap();
                assert_ne!(size, 0, "connection closed before HTTP headers arrived");
                request.extend_from_slice(&chunk[..size]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("GET /status HTTP/1.1\r\n"));
            let has_token = request.contains("X-Supervisor-Token:");
            if has_token {
                fs::write(kernel_pid_for_server, "unexpected status side effect").unwrap();
            }
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            has_token
        });
        let cfg = Config {
            var_dir: var.to_string_lossy().into_owned(),
            etc_dir: root.join("etc").to_string_lossy().into_owned(),
            home_dir: root.join("home").to_string_lossy().into_owned(),
            share_dir: root.join("share").to_string_lossy().into_owned(),
            ui_config_file: root.join("ui/config").to_string_lossy().into_owned(),
            supervisor_binary: std::env::current_exe().unwrap(),
            kernel_venv: root.join("home/venv").to_string_lossy().into_owned(),
            kernel_pid_file: kernel_pid.to_string_lossy().into_owned(),
            kernel_log: var.join("qwenpaw.log").to_string_lossy().into_owned(),
            kernel_port: crate::api::test_support::free_port(),
            web_proxy_port: None,
            kernel_log_level: "info".into(),
            token_file: token_file.to_string_lossy().into_owned(),
            supervisor_pid_file: pid_path.to_string_lossy().into_owned(),
            supervisor_log: var.join("supervisor.log").to_string_lossy().into_owned(),
            listen_port: port,
            auto_start: false,
            gateway_socket: None,
        };
        let report: serde_json::Value = serde_json::from_str(&doctor_json(&cfg)).unwrap();
        assert!(!server.join().unwrap());
        assert_eq!(
            report["manager"]["state"], "ready",
            "doctor report: {report}"
        );
        assert_eq!(fs::read_to_string(&pid_path).unwrap(), format!("{pid}\n"));
        assert_eq!(
            fs::read_to_string(&starttime_path).unwrap(),
            format!("{starttime}\n")
        );
        assert!(!kernel_pid.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn doctor_is_read_only_when_var_directory_is_absent() {
        let root = test_path("doctor-readonly");
        assert!(!root.exists());
        let var = root.join("var");
        let cfg = Config {
            var_dir: var.to_string_lossy().into_owned(),
            etc_dir: root.join("etc").to_string_lossy().into_owned(),
            home_dir: root.join("home").to_string_lossy().into_owned(),
            share_dir: root.join("share").to_string_lossy().into_owned(),
            ui_config_file: root.join("ui/config").to_string_lossy().into_owned(),
            supervisor_binary: std::env::current_exe().unwrap(),
            kernel_venv: root.join("home/venv").to_string_lossy().into_owned(),
            kernel_pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
            kernel_log: var.join("qwenpaw.log").to_string_lossy().into_owned(),
            kernel_port: crate::api::test_support::free_port(),
            web_proxy_port: None,
            kernel_log_level: "info".into(),
            token_file: var.join("supervisor-token").to_string_lossy().into_owned(),
            supervisor_pid_file: var.join("supervisor.pid").to_string_lossy().into_owned(),
            supervisor_log: var.join("supervisor.log").to_string_lossy().into_owned(),
            listen_port: crate::api::test_support::free_port(),
            auto_start: false,
            gateway_socket: None,
        };
        let report: serde_json::Value = serde_json::from_str(&doctor_json(&cfg)).unwrap();
        assert_eq!(report["manager"]["state"], "not_running");
        assert!(!root.exists());
    }

    #[test]
    fn accepts_exact_and_deleted_executable_but_rejects_same_name_elsewhere() {
        let expected = std::env::current_exe().unwrap();
        assert!(executable_matches(&expected, &expected));
        let deleted = PathBuf::from(format!("{} (deleted)", expected.display()));
        assert!(executable_matches(&expected, &deleted));
        let unrelated = PathBuf::from("/tmp").join(expected.file_name().unwrap());
        assert!(!executable_matches(&expected, &unrelated));
    }

    #[test]
    fn lifecycle_lock_is_exclusive_bounded_and_cloexec() {
        let dir = test_path("lock");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config-restart.lock");
        let lock = acquire_lifecycle_lock(&path, Duration::from_millis(100)).unwrap();
        let flags = unsafe { libc::fcntl(lock._file.as_raw_fd(), libc::F_GETFD) };
        assert!(flags & libc::FD_CLOEXEC != 0);
        let started = Instant::now();
        assert!(acquire_lifecycle_lock(&path, Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(lock);
        assert!(acquire_lifecycle_lock(&path, Duration::from_millis(100)).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
}
