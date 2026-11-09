use super::{atomic_write_0600, InstallState};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExecutableIdentity {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) starttime: u64,
    pub(crate) pgid: i32,
    pub(crate) session: i32,
    pub(crate) executable: Option<ExecutableIdentity>,
}

#[cfg(test)]
pub(super) fn wait_for_version(port: u16, target_version: &str, attempts: usize) -> bool {
    wait_for_version_cancellable(port, target_version, attempts, &|| false)
}

pub(super) fn wait_for_version_cancellable(
    port: u16,
    target_version: &str,
    attempts: usize,
    cancelled: &dyn Fn() -> bool,
) -> bool {
    use std::io::{Read, Write};
    for _ in 0..attempts {
        for _ in 0..10 {
            if cancelled() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if cancelled() {
            return false;
        }
        let Ok(mut stream) = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(500),
        ) else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
        let request = format!(
            "GET /api/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        if stream.write_all(request.as_bytes()).is_err() {
            continue;
        }
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        let mut buf = Vec::new();
        let mut chunk = [0; 4096];
        let mut complete = false;
        while std::time::Instant::now() < deadline && buf.len() <= 64 * 1024 {
            if cancelled() {
                return false;
            }
            match stream.read(&mut chunk) {
                Ok(0) => {
                    complete = true;
                    break;
                }
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
        if !complete || cancelled() {
            continue;
        }
        let response = String::from_utf8_lossy(&buf);
        if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
            continue;
        }
        let Some(body_idx) = response.find("\r\n\r\n") else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(&response[body_idx + 4..])
        else {
            continue;
        };
        if payload.get("version").and_then(|v| v.as_str()) == Some(target_version) {
            return true;
        }
    }
    false
}

/// 在独立进程组中运行短命命令；超时后必须回收整个进程组并 wait 子进程。
pub(crate) fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    label: &str,
) -> std::io::Result<Output> {
    use std::os::unix::process::CommandExt;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn()?;
    let pid = child.id() as i32;
    let mut identity = capture_process_identity(child.id(), pid).ok();
    let started = std::time::Instant::now();

    loop {
        if identity.is_none() {
            identity = capture_process_identity(child.id(), pid).ok();
        }
        match child.try_wait()? {
            Some(_) => {
                terminate_process_group(&mut child, identity)?;
                return child.wait_with_output();
            }
            None if started.elapsed() >= timeout => {
                terminate_process_group(&mut child, identity)?;
                child.wait_with_output()?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{label} 超时({}s),已终止", timeout.as_secs_f64()),
                ));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

pub(crate) fn install_pid_file(pip_log: &Path) -> std::io::Result<PathBuf> {
    pip_log
        .parent()
        .map(|parent| parent.join("install.pid"))
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "pip log has no parent")
        })
}

fn identity_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

fn current_process_identity(pid: u32) -> std::io::Result<Option<crate::procfs::ProcessIdentity>> {
    crate::procfs::process_identity(pid as i32).map_err(identity_error)
}

fn executable_identity(pid: u32) -> std::io::Result<ExecutableIdentity> {
    let metadata = std::fs::metadata(format!("/proc/{pid}/exe"))?;
    if !metadata.is_file() {
        return Err(identity_error("process executable is not a regular file"));
    }
    Ok(ExecutableIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
    })
}

fn process_is_alive(observed: crate::procfs::ProcessIdentity) -> std::io::Result<bool> {
    match crate::procfs::identity_status(observed) {
        crate::procfs::IdentityStatus::Alive => Ok(true),
        crate::procfs::IdentityStatus::Gone => Ok(false),
        crate::procfs::IdentityStatus::Reused => Err(identity_error(
            "process PID was reused during identity check",
        )),
        crate::procfs::IdentityStatus::Unknown(error) => Err(identity_error(error)),
    }
}

fn capture_process_identity(pid: u32, pgid: i32) -> std::io::Result<ProcessIdentity> {
    if pgid <= 1 {
        return Err(identity_error("invalid process group identity"));
    }
    let first = current_process_identity(pid)?.ok_or_else(|| {
        identity_error(format!("process {pid} exited before identity registration"))
    })?;
    if first.pgrp != pgid || first.starttime == 0 || first.session <= 1 {
        return Err(identity_error(
            "spawned process group identity could not be verified",
        ));
    }
    let first_alive = process_is_alive(first)?;
    let executable = first_alive.then(|| executable_identity(pid).ok()).flatten();
    let Some(second) = current_process_identity(pid)? else {
        return Ok(ProcessIdentity {
            pid,
            starttime: first.starttime,
            pgid,
            session: first.session,
            executable,
        });
    };
    if second.starttime != first.starttime || second.pgrp != pgid || second.session != first.session
    {
        return Err(identity_error(
            "spawned process identity changed during registration",
        ));
    }
    let second_alive = process_is_alive(second)?;
    if first_alive
        && second_alive
        && executable.is_some()
        && executable_identity(pid).ok() != executable
    {
        return Err(identity_error(
            "process executable changed during identity registration",
        ));
    }
    Ok(ProcessIdentity {
        pid,
        starttime: first.starttime,
        pgid,
        session: first.session,
        executable,
    })
}

fn verify_process_identity(identity: ProcessIdentity) -> std::io::Result<bool> {
    let Some(first) = current_process_identity(identity.pid)? else {
        return Ok(false);
    };
    if first.starttime != identity.starttime {
        return Err(identity_error(format!(
            "process {} starttime changed",
            identity.pid
        )));
    }
    if first.pgrp != identity.pgid || first.session != identity.session {
        return Err(identity_error(format!(
            "process {} process group/session changed",
            identity.pid
        )));
    }
    match crate::procfs::identity_status(first) {
        crate::procfs::IdentityStatus::Gone => return Ok(false),
        crate::procfs::IdentityStatus::Reused => {
            return Err(identity_error(format!(
                "process {} PID was reused",
                identity.pid
            )));
        }
        crate::procfs::IdentityStatus::Unknown(error) => return Err(identity_error(error)),
        crate::procfs::IdentityStatus::Alive => {}
    }
    let executable = identity
        .executable
        .ok_or_else(|| identity_error("registered executable identity is unavailable"))?;
    if executable_identity(identity.pid)? != executable {
        return Err(identity_error(format!(
            "process {} executable changed",
            identity.pid
        )));
    }
    let Some(second) = current_process_identity(identity.pid)? else {
        return Ok(false);
    };
    if second.starttime != identity.starttime
        || second.pgrp != identity.pgid
        || second.session != identity.session
    {
        return Err(identity_error(format!(
            "process {} identity changed during verification",
            identity.pid
        )));
    }
    match crate::procfs::identity_status(second) {
        crate::procfs::IdentityStatus::Alive => {}
        crate::procfs::IdentityStatus::Gone => return Ok(false),
        crate::procfs::IdentityStatus::Reused => {
            return Err(identity_error(format!(
                "process {} PID was reused",
                identity.pid
            )));
        }
        crate::procfs::IdentityStatus::Unknown(error) => return Err(identity_error(error)),
    }
    if executable_identity(identity.pid)? != executable {
        return Err(identity_error(format!(
            "process {} executable changed",
            identity.pid
        )));
    }
    Ok(true)
}

fn process_group_empty(pgid: i32, session: i32) -> std::io::Result<bool> {
    crate::procfs::processes_in_group(pgid, session)
        .map(|members| members.is_empty())
        .map_err(identity_error)
}

impl InstallState {
    pub(crate) fn register_process(
        &self,
        pid: u32,
        pgid: i32,
        pid_file: &Path,
    ) -> std::io::Result<bool> {
        let identity = match capture_process_identity(pid, pgid) {
            Ok(identity) => identity,
            Err(error) => {
                {
                    let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                    state.pid = pid;
                    state.pgid = pgid;
                    state.session = 0;
                    state.starttime = 0;
                    state.executable = None;
                }
                atomic_write_0600(&pid_file.to_string_lossy(), &format!("{pid}\n0\n{pgid}\n"))?;
                return Err(error);
            }
        };
        let cancelled = {
            let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            state.pid = pid;
            state.pgid = pgid;
            state.starttime = identity.starttime;
            state.session = identity.session;
            state.executable = identity.executable;
            state.cancelled || state.shutdown
        };
        let record = format!("{pid}\n{}\n{pgid}\n", identity.starttime);
        atomic_write_0600(&pid_file.to_string_lossy(), &record)?;
        if identity.executable.is_none() {
            let still_alive = match current_process_identity(pid)? {
                Some(current)
                    if current.starttime == identity.starttime
                        && current.pgrp == identity.pgid
                        && current.session == identity.session =>
                {
                    process_is_alive(current)?
                }
                Some(_) => {
                    return Err(identity_error(
                        "spawned process identity changed during registration",
                    ))
                }
                None => false,
            };
            if still_alive {
                return Err(identity_error(
                    "spawned process executable identity could not be verified",
                ));
            }
        }
        Ok(cancelled)
    }

    pub(crate) fn registered_identity(&self, pid: u32) -> Option<ProcessIdentity> {
        let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if state.pid != pid {
            return None;
        }
        Some(ProcessIdentity {
            pid,
            starttime: state.starttime,
            pgid: state.pgid,
            session: state.session,
            executable: state.executable,
        })
    }

    pub(crate) fn clear_process(&self, pid: u32, pid_file: &Path) -> std::io::Result<()> {
        let expected = {
            let state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            (state.pid == pid).then_some((state.starttime, state.pgid))
        };
        match std::fs::read_to_string(pid_file) {
            Ok(record) => {
                let fields: Vec<_> = record.lines().collect();
                let actual = fields
                    .first()
                    .and_then(|value| value.parse::<u32>().ok())
                    .zip(fields.get(1).and_then(|value| value.parse::<u64>().ok()))
                    .zip(fields.get(2).and_then(|value| value.parse::<i32>().ok()))
                    .map(|((pid, starttime), pgid)| (pid, starttime, pgid));
                let expected = expected.map(|(starttime, pgid)| (pid, starttime, pgid));
                if actual != expected {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "install.pid no longer belongs to the completed process",
                    ));
                }
                std::fs::remove_file(pid_file)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if state.pid == pid {
            state.pid = 0;
            state.pgid = 0;
            state.starttime = 0;
            state.session = 0;
            state.executable = None;
        }
        drop(state);
        self.settled.notify_all();
        Ok(())
    }

    pub(crate) fn run_cancellable_command(
        &self,
        cmd: Command,
        timeout: Duration,
        label: &str,
        pid_file: &Path,
    ) -> std::io::Result<Output> {
        self.run_cancellable_command_with_input(cmd, timeout, label, pid_file, None)
    }

    pub(crate) fn run_cancellable_command_with_input(
        &self,
        mut cmd: Command,
        timeout: Duration,
        label: &str,
        pid_file: &Path,
        input: Option<&[u8]>,
    ) -> std::io::Result<Output> {
        use std::io::Write;
        use std::os::unix::process::CommandExt;
        if self.cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                format!("{label} 已取消"),
            ));
        }
        cmd.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
        let mut child = cmd.spawn()?;
        let pid = child.id();
        let pgid = pid as i32;
        let registration = self.register_process(pid, pgid, pid_file);
        let registered_cancel = match registration {
            Ok(cancelled) => cancelled,
            Err(error) => {
                let cleanup = terminate_process_group(&mut child, self.registered_identity(pid));
                let recovery_reason = match &cleanup {
                    Ok(()) => format!("无法核实 {label} 子进程身份: {error}"),
                    Err(cleanup_error) => {
                        format!("登记 {label} 进程失败: {error}; 回收失败: {cleanup_error}")
                    }
                };
                self.block_recovery(recovery_reason);
                return Err(match cleanup {
                    Ok(()) => error,
                    Err(cleanup_error) => std::io::Error::other(format!(
                        "register {label} process failed: {error}; cleanup failed: {cleanup_error}"
                    )),
                });
            }
        };
        if registered_cancel {
            self.stop_registered_child(child, pid, pid_file, label, None)?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                format!("{label} 已取消"),
            ));
        }
        if let Some(input) = input {
            let write_result = match child.stdin.take() {
                Some(mut stdin) => stdin.write_all(input),
                None => Err(std::io::Error::other("child stdin was not piped")),
            };
            if let Err(error) = write_result {
                return self.stop_registered_child(child, pid, pid_file, label, Some(error));
            }
        }

        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let cleanup =
                        terminate_process_group(&mut child, self.registered_identity(pid));
                    match cleanup {
                        Ok(()) => {
                            let output = child.wait_with_output()?;
                            self.clear_or_block(pid, pid_file, label)?;
                            return Ok(output);
                        }
                        Err(error) => {
                            self.block_recovery(format!("{label} process cleanup failed: {error}"));
                            return Err(error);
                        }
                    }
                }
                Ok(None) if self.cancelled() => {
                    self.stop_registered_child(child, pid, pid_file, label, None)?;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        format!("{label} 已取消"),
                    ));
                }
                Ok(None) if started.elapsed() >= timeout => {
                    self.stop_registered_child(child, pid, pid_file, label, None)?;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("{label} 超时({}s),已终止", timeout.as_secs_f64()),
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => {
                    return self.stop_registered_child(child, pid, pid_file, label, Some(error));
                }
            }
        }
    }

    fn stop_registered_child(
        &self,
        mut child: Child,
        pid: u32,
        pid_file: &Path,
        label: &str,
        original_error: Option<std::io::Error>,
    ) -> std::io::Result<Output> {
        let cleanup = terminate_process_group(&mut child, self.registered_identity(pid));
        match cleanup {
            Ok(()) => {
                let output = child.wait_with_output()?;
                self.clear_or_block(pid, pid_file, label)?;
                match original_error {
                    Some(error) => Err(error),
                    None => Ok(output),
                }
            }
            Err(error) => {
                self.block_recovery(format!("{label} process cleanup failed: {error}"));
                Err(error)
            }
        }
    }

    fn clear_or_block(&self, pid: u32, pid_file: &Path, label: &str) -> std::io::Result<()> {
        if let Err(error) = self.clear_process(pid, pid_file) {
            self.block_recovery(format!("清理 {label} 进程登记失败: {error}"));
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) fn signal_process_group(identity: ProcessIdentity, signal: i32) -> std::io::Result<()> {
    if !matches!(signal, libc::SIGTERM | libc::SIGKILL) {
        return Err(identity_error("unsupported process group signal"));
    }
    if !verify_process_identity(identity)? {
        return if process_group_empty(identity.pgid, identity.session)? {
            Ok(())
        } else {
            Err(identity_error(format!(
                "process group {} remains after its registered leader exited",
                identity.pgid
            )))
        };
    }
    if process_group_empty(identity.pgid, identity.session)? {
        return Ok(());
    }
    if unsafe { libc::kill(-identity.pgid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH)
        && process_group_empty(identity.pgid, identity.session)?
    {
        Ok(())
    } else {
        Err(error)
    }
}

pub(crate) fn terminate_process_group(
    child: &mut Child,
    identity: Option<ProcessIdentity>,
) -> std::io::Result<()> {
    let identity = identity.ok_or_else(|| {
        identity_error("process group cleanup requires a captured leader identity")
    })?;
    let pgid = identity.pgid;
    let session = identity.session;
    if pgid <= 1 || session <= 1 {
        return Err(identity_error("invalid process group/session for cleanup"));
    }
    if !verify_process_identity(identity)? {
        if process_group_empty(pgid, session)? {
            child.wait()?;
            return Ok(());
        }
        return Err(identity_error(format!(
            "process group {pgid} remains after its registered leader exited"
        )));
    }
    if process_group_empty(pgid, session)? {
        child.wait()?;
        return Ok(());
    }
    signal_process_group(identity, libc::SIGTERM)?;

    let term_deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if process_group_empty(pgid, session)? {
            child.wait()?;
            return Ok(());
        }
        if std::time::Instant::now() >= term_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    signal_process_group(identity, libc::SIGKILL)?;
    child.wait()?;
    let kill_deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if process_group_empty(pgid, session)? {
            return Ok(());
        }
        if std::time::Instant::now() >= kill_deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("process group {pgid} survived SIGKILL"),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// 用 venv 内 python 读已安装内核版本(importlib.metadata)
pub(super) fn metadata_version(python: &Path) -> String {
    let mut cmd = std::process::Command::new(python);
    cmd.args([
        "-c",
        "import importlib.metadata as m; print(m.version('qwenpaw'))",
    ]);
    run_with_timeout(cmd, std::time::Duration::from_secs(30), "读取内核版本")
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// 实际版本 vs 请求版本:PEP 440 允许前导 v 且大小写不敏感,其余按字面比较。
/// 不相等即失败 —— 宁可报错也不静默装错版本。
pub(super) fn version_matches(actual: &str, want: &str) -> bool {
    let norm = |s: &str| s.trim().trim_start_matches(['v', 'V']).to_ascii_lowercase();
    norm(actual) == norm(want)
}

#[cfg(test)]
mod identity_registration_tests {
    use super::*;
    use std::{
        os::unix::process::CommandExt,
        sync::atomic::{AtomicU64, Ordering},
    };

    #[test]
    fn short_lived_child_gone_before_executable_check_is_registered() {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let pid_file = std::env::temp_dir().join(format!(
            "qwenpaw-short-lived-process-{}-{}.pid",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut child = Command::new("/bin/true").process_group(0).spawn().unwrap();
        let pid = child.id();
        let pgid = pid as i32;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let current = current_process_identity(pid).unwrap();
            if current.is_some_and(|identity| !process_is_alive(identity).unwrap()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "short-lived child did not exit before identity registration"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(current_process_identity(pid).unwrap().is_some());
        assert!(
            executable_identity(pid).is_err(),
            "exited child's /proc executable must be unavailable during registration"
        );

        let state = InstallState::new(std::sync::Arc::new(|_| {}), std::sync::Arc::new(|| Ok(0)));
        let registration = state.register_process(pid, pgid, &pid_file);
        let identity = state.registered_identity(pid);
        let record = std::fs::read_to_string(&pid_file).ok();
        child.wait().unwrap();
        let _ = std::fs::remove_file(&pid_file);

        assert!(
            registration.is_ok(),
            "a child that exited before identity registration must not block registration: {registration:?}"
        );
        let identity = identity.expect("the exited child identity must remain registered");
        assert_eq!(identity.executable, None);
        assert!(!verify_process_identity(identity).unwrap());
        assert!(process_group_empty(identity.pgid, identity.session).unwrap());
        assert!(record.is_some_and(|record| record.starts_with(&format!("{pid}\n"))));
    }
}
