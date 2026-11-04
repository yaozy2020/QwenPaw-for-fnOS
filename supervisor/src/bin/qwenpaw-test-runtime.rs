use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{self, Command},
    thread,
    time::Duration,
};

const DEFAULT_INSTALL_VERSION: &str = "2.0.0";
const MAX_REQUEST_BYTES: usize = 16 * 1024;

fn main() {
    if let Err(error) = run() {
        eprintln!("qwenpaw-test-runtime: {error}");
        process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == OsStr::new("--test-worker"))
    {
        return run_test_worker(false);
    }
    if args
        .first()
        .is_some_and(|arg| arg == OsStr::new("--test-grandchild"))
    {
        return run_test_worker(true);
    }
    if args.len() >= 3 && args[0] == OsStr::new("-m") && args[1] == OsStr::new("venv") {
        return create_venv(Path::new(&args[2]));
    }
    if args.len() >= 2 && args[0] == OsStr::new("-c") {
        let control = env::var_os("QWENPAW_TEST_CONTROL").map(PathBuf::from);
        return run_code_probe(&args[1], control.as_deref());
    }
    if args.len() >= 2 && args[0] == OsStr::new("-m") && args[1] == OsStr::new("pip") {
        return install_package(&args[2..]);
    }
    if args.len() == 4
        && args[0] == OsStr::new("-m")
        && args[1] == OsStr::new("qwenpaw")
        && args[2] == OsStr::new("init")
        && args[3] == OsStr::new("--defaults")
    {
        return run_init(&args);
    }
    if args.len() >= 3
        && args[0] == OsStr::new("-m")
        && args[1] == OsStr::new("qwenpaw")
        && args[2] == OsStr::new("app")
    {
        return serve_kernel(&args[3..]);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unsupported runtime argv: {}", display_args(&args)),
    ))
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn run_code_probe(code: &OsStr, control: Option<&Path>) -> io::Result<()> {
    if is_qwenpaw_import_probe(code) {
        if let Some(control) = control {
            qwenpaw_import_probe(control)?;
        }
        if code
            .to_str()
            .is_some_and(|code| code.trim() == "import qwenpaw")
        {
            return Ok(());
        }
    }
    print_installed_version()
}

fn is_qwenpaw_import_probe(code: &OsStr) -> bool {
    code.to_str().is_some_and(|code| {
        code.split([';', '\n'])
            .any(|statement| statement.trim() == "import qwenpaw")
    })
}

fn qwenpaw_import_probe(control: &Path) -> io::Result<()> {
    if control.join("bad-import").exists() || control.join("fail-import").exists() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "test runtime forced qwenpaw import failure",
        ));
    }
    Ok(())
}

fn run_init(args: &[OsString]) -> io::Result<()> {
    let control = test_control_dir()?;
    let working_dir = env::var_os("QWENPAW_WORKING_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "QWENPAW_WORKING_DIR is not set"))?;
    let environment = env::vars_os().collect::<Vec<_>>();
    let mut input = io::stdin().lock();
    initialize_qwenpaw(&control, &working_dir, args, &environment, &mut input)
}

fn initialize_qwenpaw(
    control: &Path,
    working_dir: &Path,
    args: &[OsString],
    environment: &[(OsString, OsString)],
    input: &mut impl Read,
) -> io::Result<()> {
    let count = record_init_call(control, args, environment)?;
    fs::write(control.join("init-started"), format!("{count}\n"))?;
    if slow_init_requested(control) {
        thread::sleep(Duration::from_secs(60));
    }

    let mut confirmation = String::new();
    input.read_to_string(&mut confirmation)?;
    if confirmation.trim() != "Yes" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "qwenpaw init requires Yes confirmation",
        ));
    }

    ensure_workspace_uninitialized(working_dir)?;
    if control.join("fail-init").exists() {
        return Err(io::Error::other("test runtime forced qwenpaw init failure"));
    }
    create_minimal_config(working_dir)
}

fn record_init_call(
    control: &Path,
    args: &[OsString],
    environment: &[(OsString, OsString)],
) -> io::Result<u64> {
    fs::create_dir_all(control)?;
    let count_path = control.join("init-count");
    let previous = match fs::read_to_string(&count_path) {
        Ok(count) => count.trim().parse::<u64>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "init-count is not a valid number",
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let count = previous
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "init-count overflow"))?;
    let argv = format!(
        "[{}]",
        args.iter()
            .map(|arg| json_string(&arg.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut environment = environment.to_vec();
    environment.sort_by(|left, right| left.0.cmp(&right.0));
    let serialized_environment = format!(
        "{{{}}}",
        environment
            .iter()
            .map(|(key, value)| format!(
                "{}:{}",
                json_string(&key.to_string_lossy()),
                json_string(&value.to_string_lossy())
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut invocations = OpenOptions::new()
        .create(true)
        .append(true)
        .open(control.join("init-invocations.jsonl"))?;
    writeln!(
        invocations,
        "{{\"count\":{count},\"argv\":{argv},\"env\":{serialized_environment}}}"
    )?;
    invocations.sync_data()?;
    fs::write(count_path, format!("{count}\n"))?;
    fs::write(control.join("init-argv"), format!("{argv}\n"))?;
    fs::write(
        control.join("init-env"),
        format!("{serialized_environment}\n"),
    )?;
    Ok(count)
}

fn slow_init_requested(control: &Path) -> bool {
    control.join("slow-init").exists()
}

fn ensure_workspace_uninitialized(working_dir: &Path) -> io::Result<()> {
    let config = working_dir.join("config.json");
    let config_exists = match fs::symlink_metadata(&config) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let workspaces = working_dir.join("workspaces");
    let workspaces_exists = match fs::metadata(&workspaces) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if config_exists || workspaces_exists {
        let mut existing = Vec::new();
        if config_exists {
            existing.push("config.json");
        }
        if workspaces_exists {
            existing.push("workspaces");
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "qwenpaw init refused existing QwenPaw data ({}) under {}",
                existing.join(", "),
                working_dir.display()
            ),
        ));
    }
    Ok(())
}

fn create_minimal_config(working_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(working_dir)?;
    ensure_workspace_uninitialized(working_dir)?;
    let config = working_dir.join("config.json");
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "qwenpaw init refused existing QwenPaw data (config.json) under {}",
                    working_dir.display()
                ),
            ));
        }
        Err(error) => return Err(error),
    };
    file.write_all(b"{}\n")?;
    file.sync_all()
}

fn test_control_dir() -> io::Result<PathBuf> {
    env::var_os("QWENPAW_TEST_CONTROL")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "QWENPAW_TEST_CONTROL is not set"))
}

fn current_venv_dir() -> io::Result<PathBuf> {
    let executable = env::current_exe()?;
    executable
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime is not inside venv/bin",
            )
        })
}

fn create_venv(destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination.join("bin"))?;
    fs::copy(env::current_exe()?, destination.join("bin/python3"))?;
    Ok(())
}

fn installed_version(venv: &Path) -> io::Result<String> {
    let version = match fs::read_to_string(venv.join("installed")) {
        Ok(version) => version,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("1.0.0".to_string()),
        Err(error) => return Err(error),
    };
    let version = version.trim();
    if version.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "installed version is empty",
        ));
    }
    Ok(version.to_string())
}

fn print_installed_version() -> io::Result<()> {
    println!("{}", installed_version(&current_venv_dir()?)?);
    Ok(())
}

fn install_package(args: &[OsString]) -> io::Result<()> {
    let control = test_control_dir()?;
    fs::write(control.join("pip-started"), "yes")?;
    if control.join("fail-pip").exists() {
        let mut attempts = OpenOptions::new()
            .create(true)
            .append(true)
            .open(control.join("pip-attempts"))?;
        writeln!(attempts, "failed")?;
        return Err(io::Error::other("test runtime forced pip failure"));
    }
    if control.join("slow-pip").exists() {
        thread::sleep(Duration::from_secs(60));
    }
    let requested = args
        .iter()
        .find_map(|arg| arg.to_str().and_then(|arg| arg.strip_prefix("qwenpaw==")));
    let version = requested
        .map(str::to_owned)
        .or_else(|| env::var("QWENPAW_TEST_RUNTIME_INSTALL_VERSION").ok())
        .unwrap_or_else(|| DEFAULT_INSTALL_VERSION.to_string());
    if version.is_empty() || version.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pip target version is invalid",
        ));
    }
    fs::write(
        current_venv_dir()?.join("installed"),
        format!("{version}\n"),
    )?;
    println!("Successfully installed qwenpaw-{version}");
    Ok(())
}

fn run_test_worker(grandchild: bool) -> io::Result<()> {
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    let _grandchild = if grandchild {
        None
    } else {
        let child = Command::new(env::current_exe()?)
            .arg("--test-grandchild")
            .process_group(0)
            .spawn()?;
        let control = test_control_dir()?;
        fs::write(
            control.join("target-descendants"),
            format!("{}\n{}\n", process::id(), child.id()),
        )?;
        Some(child)
    };
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn start_test_process_tree() -> io::Result<()> {
    let control = test_control_dir()?;
    if control.join("spawn-process-tree").exists() {
        Command::new(env::current_exe()?)
            .arg("--test-worker")
            .process_group(0)
            .spawn()?;
    }
    Ok(())
}

fn serve_kernel(args: &[OsString]) -> io::Result<()> {
    let port = args
        .windows(2)
        .find(|pair| pair[0] == OsStr::new("--port"))
        .and_then(|pair| pair[1].to_str())
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing valid --port"))?;
    let control = test_control_dir()?;
    if control.join("remove-control-socket").exists() {
        let var_dir = env::var_os("TRIM_PKGVAR")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "TRIM_PKGVAR is not set"))?;
        let socket = var_dir.join("supervisor-control.sock");
        for _ in 0..500 {
            if socket.exists() {
                fs::remove_file(&socket)?;
                fs::write(control.join("control-socket-removed"), "yes\n")?;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    start_test_process_tree()?;
    if control.join("pause-kernel").exists() {
        fs::write(control.join("kernel-paused"), "")?;
        thread::sleep(Duration::from_secs(60));
    }
    if fs::read_to_string(control.join("fail-port"))
        .map(|failed_port| failed_port.trim() == port.to_string())
        .unwrap_or(false)
    {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("test runtime refusing port {port}"),
        ));
    }
    let venv = current_venv_dir()?;
    installed_version(&venv)?;
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let pid = process::id();
    let start_time = process_start_time(pid)?;
    fs::write(
        venv.join(".qwenpaw-test-runtime.pid"),
        format!("{pid} {start_time}\n"),
    )?;

    for incoming in listener.incoming() {
        handle_incoming(incoming, &venv, pid)?;
    }
    Ok(())
}

fn process_start_time(pid: u32) -> io::Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let rest = stat
        .rsplit_once(") ")
        .map(|(_, rest)| rest)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid /proc stat"))?;
    rest.split_whitespace()
        .nth(19)
        .map(str::to_string)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing process starttime"))
}

fn handle_incoming(incoming: io::Result<TcpStream>, venv: &Path, pid: u32) -> io::Result<()> {
    let stream = incoming?;
    match handle_request(stream, venv, pid) {
        Err(error) if is_client_disconnect(&error) => Ok(()),
        result => result,
    }
}

fn is_client_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
}

fn handle_request(mut stream: TcpStream, venv: &Path, pid: u32) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    while request.len() < MAX_REQUEST_BYTES && !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let count = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(error) => return Err(error),
        };
        request.extend_from_slice(&chunk[..count]);
    }
    if request.is_empty() {
        return Ok(());
    }

    let request_line = request
        .split(|byte| *byte == b'\n')
        .next()
        .map(|line| String::from_utf8_lossy(line).trim().to_string())
        .unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    if method == "GET" && path == "/api/version" {
        let version = installed_version(venv)?;
        append_health_record(venv, pid, &version)?;
        let body = format!("{{\"version\":{}}}", json_string(&version));
        write_response(&mut stream, "200 OK", "application/json", body.as_bytes())
    } else if method == "GET" && path == "/api/auth/status" {
        write_response(
            &mut stream,
            "200 OK",
            "application/json",
            b"{\"enabled\":true,\"has_users\":true}",
        )
    } else {
        write_response(&mut stream, "404 Not Found", "application/json", b"{}")
    }
}

fn append_health_record(venv: &Path, pid: u32, version: &str) -> io::Result<()> {
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(venv.join(".qwenpaw-test-runtime-health"))?;
    writeln!(log, "pid={pid} version={version}")?;
    log.sync_data()
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_string_escapes_control_and_quote_characters() {
        assert_eq!(json_string("v\"\\\n2"), "\"v\\\"\\\\\\n2\"");
    }

    #[test]
    fn process_start_time_reads_linux_proc_stat() {
        let start_time = process_start_time(process::id()).unwrap();
        assert!(start_time.bytes().all(|byte| byte.is_ascii_digit()));
    }

    #[test]
    fn missing_installed_file_reports_uninstalled_metadata_version() {
        let venv = env::temp_dir().join(format!(
            "qwenpaw-test-runtime-uninstalled-{}",
            process::id()
        ));
        let _ = fs::remove_dir_all(&venv);
        fs::create_dir_all(&venv).unwrap();
        assert_eq!(installed_version(&venv).unwrap(), "1.0.0");
        fs::remove_dir_all(venv).unwrap();
    }

    #[test]
    fn http_helper_exposes_version_and_auth_status() {
        fn request(venv: &Path, path: &str) -> String {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let venv = venv.to_path_buf();
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                handle_request(stream, &venv, process::id()).unwrap();
            });
            let mut stream = TcpStream::connect(address).unwrap();
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            server.join().unwrap();
            response
        }

        let venv = env::temp_dir().join(format!("qwenpaw-test-runtime-http-{}", process::id()));
        let _ = fs::remove_dir_all(&venv);
        fs::create_dir_all(&venv).unwrap();
        fs::write(venv.join("installed"), "2.3.0\n").unwrap();

        let version = request(&venv, "/api/version");
        assert!(version.starts_with("HTTP/1.1 200 OK"), "{version}");
        assert!(version.ends_with("{\"version\":\"2.3.0\"}"), "{version}");

        let auth = request(&venv, "/api/auth/status");
        assert!(auth.starts_with("HTTP/1.1 200 OK"), "{auth}");
        assert!(
            auth.ends_with("{\"enabled\":true,\"has_users\":true}"),
            "{auth}"
        );
        fs::remove_dir_all(venv).unwrap();
    }

    #[test]
    fn empty_tcp_connection_closes_silently_and_version_remains_available() {
        use std::net::Shutdown;

        let venv = env::temp_dir().join(format!(
            "qwenpaw-test-runtime-empty-probe-{}",
            process::id()
        ));
        let _ = fs::remove_dir_all(&venv);
        fs::create_dir_all(&venv).unwrap();
        fs::write(venv.join("installed"), "2.3.0\n").unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server_venv = venv.clone();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let incoming = listener.accept().map(|(stream, _)| stream);
                handle_incoming(incoming, &server_venv, process::id()).unwrap();
            }
        });

        let mut probe = TcpStream::connect(address).unwrap();
        probe.shutdown(Shutdown::Write).unwrap();
        let mut probe_response = Vec::new();
        probe.read_to_end(&mut probe_response).unwrap();

        let mut health = TcpStream::connect(address).unwrap();
        write!(
            health,
            "GET /api/version HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut health_response = String::new();
        health.read_to_string(&mut health_response).unwrap();
        server.join().unwrap();

        assert!(probe_response.is_empty(), "{probe_response:?}");
        assert!(
            health_response.starts_with("HTTP/1.1 200 OK"),
            "{health_response}"
        );
        assert!(
            health_response.ends_with("{\"version\":\"2.3.0\"}"),
            "{health_response}"
        );
        fs::remove_dir_all(venv).unwrap();
    }

    #[test]
    fn client_disconnect_errors_are_classified_narrowly() {
        for kind in [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::NotConnected,
            io::ErrorKind::UnexpectedEof,
        ] {
            assert!(is_client_disconnect(&io::Error::from(kind)), "{kind:?}");
        }
        assert!(!is_client_disconnect(&io::Error::other("internal error")));
    }

    #[test]
    fn listener_errors_are_not_suppressed() {
        let error = handle_incoming(
            Err(io::Error::other("listener failure")),
            Path::new("."),
            process::id(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "listener failure");
    }

    #[test]
    fn internal_health_errors_are_not_suppressed() {
        let venv = env::temp_dir().join(format!(
            "qwenpaw-test-runtime-invalid-venv-{}",
            process::id()
        ));
        let _ = fs::remove_file(&venv);
        fs::write(&venv, "not a directory").unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server_venv = venv.clone();
        let server = thread::spawn(move || {
            let incoming = listener.accept().map(|(stream, _)| stream);
            handle_incoming(incoming, &server_venv, process::id())
        });

        let mut client = TcpStream::connect(address).unwrap();
        write!(
            client,
            "GET /api/version HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        let error = server.join().unwrap().unwrap_err();

        assert!(!is_client_disconnect(&error), "{error}");
        assert!(response.is_empty(), "{response:?}");
        fs::remove_file(venv).unwrap();
    }

    fn init_fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = env::temp_dir().join(format!(
            "qwenpaw-test-runtime-init-{name}-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let control = root.join("control");
        let working = root.join("working");
        fs::create_dir_all(&control).unwrap();
        (root, control, working)
    }

    #[test]
    fn init_records_argv_environment_and_creates_minimal_config() {
        let (root, control, working) = init_fixture("success");
        let args = ["-m", "qwenpaw", "init", "--defaults"].map(OsString::from);
        let environment = vec![
            (
                OsString::from("QWENPAW_WORKING_DIR"),
                working.as_os_str().to_os_string(),
            ),
            (OsString::from("TEST_MARKER"), OsString::from("recorded")),
        ];
        let mut input = io::Cursor::new("Yes\n");

        initialize_qwenpaw(&control, &working, &args, &environment, &mut input).unwrap();

        assert_eq!(fs::read(working.join("config.json")).unwrap(), b"{}\n");
        assert_eq!(
            fs::read_to_string(control.join("init-count")).unwrap(),
            "1\n"
        );
        assert_eq!(
            fs::read_to_string(control.join("init-argv")).unwrap(),
            "[\"-m\",\"qwenpaw\",\"init\",\"--defaults\"]\n"
        );
        assert_eq!(
            fs::read_to_string(control.join("init-env")).unwrap(),
            format!(
                "{{\"QWENPAW_WORKING_DIR\":{},\"TEST_MARKER\":\"recorded\"}}\n",
                json_string(&working.to_string_lossy())
            )
        );
        assert!(fs::read_to_string(control.join("init-invocations.jsonl"))
            .unwrap()
            .contains("\"count\":1"));
        assert_eq!(
            fs::read_to_string(control.join("init-started")).unwrap(),
            "1\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn init_rejects_existing_config_or_workspaces_without_overwriting() {
        for existing in ["config", "workspaces"] {
            let (root, control, working) = init_fixture(existing);
            fs::create_dir_all(&working).unwrap();
            if existing == "config" {
                fs::write(working.join("config.json"), "existing config\n").unwrap();
            } else {
                fs::create_dir(working.join("workspaces")).unwrap();
            }
            let args = ["-m", "qwenpaw", "init", "--defaults"].map(OsString::from);
            let mut input = io::Cursor::new("Yes\n");

            let error = initialize_qwenpaw(&control, &working, &args, &[], &mut input).unwrap_err();

            assert!(error.to_string().contains("refused existing"), "{error}");
            if existing == "config" {
                assert_eq!(
                    fs::read_to_string(working.join("config.json")).unwrap(),
                    "existing config\n"
                );
            } else {
                assert!(working.join("workspaces").is_dir());
                assert!(!working.join("config.json").exists());
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn init_requires_yes_and_supports_controlled_failure_and_delay() {
        let (root, control, working) = init_fixture("controls");
        let args = ["-m", "qwenpaw", "init", "--defaults"].map(OsString::from);
        let mut input = io::Cursor::new("No\n");
        let error = initialize_qwenpaw(&control, &working, &args, &[], &mut input).unwrap_err();
        assert!(error.to_string().contains("Yes"), "{error}");
        assert!(!working.join("config.json").exists());

        fs::write(control.join("fail-init"), "").unwrap();
        let mut input = io::Cursor::new("Yes\n");
        let error = initialize_qwenpaw(&control, &working, &args, &[], &mut input).unwrap_err();
        assert!(error.to_string().contains("forced"), "{error}");
        assert!(!working.join("config.json").exists());

        fs::write(control.join("slow-init"), "").unwrap();
        assert!(slow_init_requested(&control));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn qwenpaw_import_probe_supports_success_and_injected_failure() {
        let (root, control, _) = init_fixture("import");
        qwenpaw_import_probe(&control).unwrap();
        fs::write(control.join("bad-import"), "").unwrap();
        let error = qwenpaw_import_probe(&control).unwrap_err();
        assert!(error.to_string().contains("import"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_probe_recognizes_only_qwenpaw_import_statements() {
        assert!(is_qwenpaw_import_probe(OsStr::new("import qwenpaw")));
        assert!(is_qwenpaw_import_probe(OsStr::new(
            "import importlib.metadata as m; m.version('qwenpaw'); import qwenpaw"
        )));
        assert!(!is_qwenpaw_import_probe(OsStr::new(
            "import importlib.metadata as m; print(m.version('qwenpaw'))"
        )));
    }
}
