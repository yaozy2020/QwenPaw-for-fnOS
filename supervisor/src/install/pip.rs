use super::{truncate_utf8, uv_args, uv_bootstrap_args, InstallChannel, InstallState, Phase};
use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub(super) trait PipSpawner: Send + Sync {
    fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child>;
}

pub(super) struct RealSpawner;

impl PipSpawner for RealSpawner {
    fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child> {
        cmd.spawn()
    }
}

const LOG_LINE_CAP: usize = 4096;
const LOG_TOTAL_CAP: u64 = 1024 * 1024;
const NO_OUTPUT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy)]
enum InstallerTool {
    PipBootstrap,
    Uv,
}

fn append_line(log: &mut Option<std::fs::File>, total: &Arc<Mutex<u64>>, raw: &str) {
    if let Some(f) = log.as_mut() {
        let mut t = total.lock().unwrap_or_else(|e| e.into_inner());
        if *t >= LOG_TOTAL_CAP {
            return;
        }
        let owned = truncate_utf8(raw, LOG_LINE_CAP);
        if f.write_all(owned.as_bytes()).is_ok() && f.write_all(b"\n").is_ok() {
            *t += owned.len() as u64 + 1;
        }
    }
}

fn touch_activity(last: &AtomicU64, start: &Instant) {
    last.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
}

fn set_nonblocking<R: AsRawFd>(reader: &R) -> io::Result<()> {
    let fd = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_pipe_lines<R, F>(mut reader: R, stop: &AtomicBool, mut on_line: F)
where
    R: Read + AsRawFd,
    F: FnMut(&[u8]),
{
    if set_nonblocking(&reader).is_err() {
        return;
    }
    let mut pending = Vec::new();
    let mut chunk = [0u8; 8192];
    while !stop.load(Ordering::Acquire) {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(size) => {
                pending.extend_from_slice(&chunk[..size]);
                while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                    let line = pending.drain(..=index).collect::<Vec<_>>();
                    on_line(&line[..line.len() - 1]);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if !pending.is_empty() {
        on_line(&pending);
    }
}

impl InstallState {
    pub(super) fn run_uv(
        &self,
        python: &Path,
        channel: &InstallChannel,
        mirror: &str,
        pip_log: &str,
    ) -> Result<(), String> {
        self.run_installer(
            python,
            channel,
            mirror,
            pip_log,
            InstallerTool::PipBootstrap,
            false,
        )?;
        self.run_installer(python, channel, mirror, pip_log, InstallerTool::Uv, true)
    }

    fn run_installer(
        &self,
        python: &Path,
        channel: &InstallChannel,
        mirror: &str,
        pip_log: &str,
        tool: InstallerTool,
        append_log: bool,
    ) -> Result<(), String> {
        use super::process::{install_pid_file, terminate_process_group};
        use std::os::unix::process::CommandExt;
        let cache_dir = Path::new(pip_log)
            .parent()
            .ok_or_else(|| "安装日志路径缺少父目录".to_string())?
            .join(match tool {
                InstallerTool::PipBootstrap => "pip-cache",
                InstallerTool::Uv => "uv-cache",
            });
        std::fs::create_dir_all(&cache_dir)
            .map_err(|error| format!("创建安装缓存目录失败: {error}"))?;
        let mut cmd = Command::new(python);
        match tool {
            InstallerTool::PipBootstrap => {
                cmd.arg("-m").arg("pip").args(uv_bootstrap_args());
                if !mirror.is_empty() {
                    cmd.env("PIP_INDEX_URL", mirror);
                }
            }
            InstallerTool::Uv => {
                let uv = python
                    .parent()
                    .ok_or_else(|| "venv Python 路径缺少 bin 目录".to_string())?
                    .join("uv");
                cmd = Command::new(uv);
                cmd.args(uv_args(channel))
                    .arg("--python")
                    .arg(python)
                    .env("UV_DEFAULT_INDEX", mirror);
            }
        }
        cmd.env(
            match tool {
                InstallerTool::PipBootstrap => "PIP_CACHE_DIR",
                InstallerTool::Uv => "UV_CACHE_DIR",
            },
            cache_dir,
        );
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(append_log)
            .truncate(!append_log)
            .open(pip_log)
            .ok();
        let total = Arc::new(Mutex::new(0u64));
        let pid_file = install_pid_file(Path::new(pip_log)).map_err(|error| error.to_string())?;
        self.check_cancelled()?;
        let mut child = self.spawner.spawn(&mut cmd).map_err(|e| e.to_string())?;
        let pid = child.id();
        let pgid = pid as i32;
        let registered_cancel = match self.register_process(pid, pgid, &pid_file) {
            Ok(cancelled) => cancelled,
            Err(error) => {
                let cleanup = terminate_process_group(&mut child, self.registered_identity(pid));
                let recovery_reason = match &cleanup {
                    Ok(()) => format!("无法核实 pip 子进程身份: {error}"),
                    Err(cleanup_error) => {
                        format!("登记 pip 进程失败: {error}; 回收失败: {cleanup_error}")
                    }
                };
                self.block_recovery(recovery_reason);
                return Err(match cleanup {
                    Ok(()) => error.to_string(),
                    Err(cleanup_error) => {
                        format!("登记 pip 进程失败: {error}; 回收失败: {cleanup_error}")
                    }
                });
            }
        };
        if registered_cancel {
            if let Err(error) = terminate_process_group(&mut child, self.registered_identity(pid)) {
                self.block_recovery(format!("回收已取消的 pip 进程失败: {error}"));
                return Err(format!("安装已被停止；进程回收失败: {error}"));
            }
            self.clear_process(pid, &pid_file).map_err(|error| {
                self.block_recovery(format!("清理已取消 pip 进程登记失败: {error}"));
                error.to_string()
            })?;
            return Err("安装已被停止".into());
        }
        let started = Arc::new(Instant::now());
        let last_activity = Arc::new(AtomicU64::new(0));
        let reader_stop = Arc::new(AtomicBool::new(false));
        let mut readers = Vec::new();
        if let Some(err) = child.stderr.take() {
            let stop = Arc::clone(&reader_stop);
            let mut log = log.as_ref().and_then(|f| f.try_clone().ok());
            let total = Arc::clone(&total);
            let activity = Arc::clone(&last_activity);
            let start = Arc::clone(&started);
            readers.push(std::thread::spawn(move || {
                read_pipe_lines(err, &stop, |bytes| {
                    touch_activity(&activity, &start);
                    append_line(
                        &mut log,
                        &total,
                        String::from_utf8_lossy(bytes).trim_end_matches(['\r']),
                    );
                });
            }));
        }
        if let Some(out) = child.stdout.take() {
            let stop = Arc::clone(&reader_stop);
            let this = self.clone();
            let mut log = log.as_ref().and_then(|f| f.try_clone().ok());
            let total = Arc::clone(&total);
            let activity = Arc::clone(&last_activity);
            let start = Arc::clone(&started);
            readers.push(std::thread::spawn(move || {
                read_pipe_lines(out, &stop, |bytes| {
                    touch_activity(&activity, &start);
                    let owned = String::from_utf8_lossy(bytes);
                    let line = owned.trim_end();
                    append_line(&mut log, &total, line);
                    if let Some(package) = line.strip_prefix("Collecting ") {
                        let package = package.split(' ').next().unwrap_or("");
                        this.set(|g| {
                            g.fetched += 1;
                            g.message = format!("获取 {package}");
                        });
                    } else if line.starts_with("Downloading ") {
                        this.set(|g| g.message = "下载中".into());
                    } else if line.starts_with("Successfully installed ") {
                        this.set(|g| {
                            g.phase = Phase::Installing;
                            g.message = "安装依赖包".into();
                        });
                    }
                });
            }));
        }
        let mut exit_ok = false;
        let mut wait_err = None;
        let mut timeout_reason = String::new();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_ok = status.success();
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    wait_err = Some(e.to_string());
                    break;
                }
            }
            if self.cancelled() {
                break;
            }
            let elapsed = started.elapsed();
            let idle_ms =
                (elapsed.as_millis() as u64).saturating_sub(last_activity.load(Ordering::Relaxed));
            if idle_ms > NO_OUTPUT_TIMEOUT.as_millis() as u64 {
                timeout_reason = format!(
                    "安装超时: 无输出超过 {} 分钟,已终止(下载源可能失联,请换源重试)",
                    NO_OUTPUT_TIMEOUT.as_secs() / 60
                );
                break;
            }
            if elapsed > TOTAL_TIMEOUT {
                timeout_reason = format!(
                    "安装超时: 总时长超过 {} 分钟,已终止",
                    TOTAL_TIMEOUT.as_secs() / 60
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let cleanup = terminate_process_group(&mut child, self.registered_identity(pid));
        reader_stop.store(true, Ordering::Release);
        for reader in readers {
            let _ = reader.join();
        }
        if let Err(error) = cleanup {
            self.block_recovery(format!("pip 进程组回收失败: {error}"));
            return Err(format!("pip 进程组回收失败: {error}"));
        }
        self.clear_process(pid, &pid_file).map_err(|error| {
            self.block_recovery(format!("清理 pip 进程登记失败: {error}"));
            format!("清理 pip 进程登记失败: {error}")
        })?;
        self.check_cancelled()?;
        if !timeout_reason.is_empty() {
            return Err(timeout_reason);
        }
        if let Some(e) = wait_err {
            return Err(format!("pip 进程等待异常: {e}"));
        }
        if !exit_ok {
            let mut reason = "pip 安装失败,详见日志".to_string();
            if let Ok(content) = std::fs::read_to_string(pip_log) {
                if let Some(line) = content.lines().rev().find(|line| {
                    line.contains("ERROR:")
                        || line.contains("403 Forbidden")
                        || line.contains("from versions: none")
                }) {
                    reason = format!("pip 安装失败: {}", line.trim());
                }
            }
            return Err(reason);
        }
        Ok(())
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;
    use std::{fs::File, os::fd::FromRawFd, sync::mpsc};

    #[test]
    fn reader_stops_while_a_writer_keeps_the_pipe_open() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let reader = unsafe { File::from_raw_fd(fds[0]) };
        let mut writer = unsafe { File::from_raw_fd(fds[1]) };
        let stop = Arc::new(AtomicBool::new(false));
        let reader_stop = Arc::clone(&stop);
        let (line_tx, line_rx) = mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            read_pipe_lines(reader, &reader_stop, |line| {
                let _ = line_tx.send(String::from_utf8_lossy(line).into_owned());
            });
        });

        writer.write_all(b"pip output\n").unwrap();
        assert_eq!(
            line_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "pip output"
        );
        let stopped_at = Instant::now();
        stop.store(true, Ordering::Release);
        reader_thread.join().unwrap();
        assert!(stopped_at.elapsed() < Duration::from_secs(1));
    }
}
