use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[cfg(test)]
use std::{io::Write, process::Stdio, time::Instant};

const METADATA_TIMEOUT: Duration = Duration::from_secs(15);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const INIT_PROBE: &str =
    "import importlib.metadata as metadata; import qwenpaw; print(metadata.version('qwenpaw'))";
const NODE_PATH: &str = "/var/apps/nodejs_v24/target/bin:/usr/local/bin:/usr/bin:/bin";

#[derive(Clone)]
pub(crate) struct RuntimeAdapter {
    venv_dir: PathBuf,
    share_dir: PathBuf,
    log_level: String,
}

impl RuntimeAdapter {
    pub(crate) fn new(
        venv_dir: impl Into<PathBuf>,
        share_dir: impl Into<PathBuf>,
        log_level: impl Into<String>,
    ) -> Self {
        Self {
            venv_dir: venv_dir.into(),
            share_dir: share_dir.into(),
            log_level: log_level.into(),
        }
    }

    fn python(&self) -> PathBuf {
        self.venv_dir.join("bin/python3")
    }

    pub(crate) fn metadata_version(python: &Path) -> Result<String, String> {
        let mut command = Command::new(python);
        command.args(["-c", INIT_PROBE]);
        let output = crate::install::run_with_timeout(command, METADATA_TIMEOUT, "读取内核版本")
            .map_err(|error| format!("读取内核版本失败: {error}"))?;
        if !output.status.success() {
            return Err(format!("读取内核版本失败: {}", output.status));
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if version.is_empty() {
            return Err("内核 metadata/import 探针未返回版本".into());
        }
        Ok(version)
    }

    pub(crate) fn has_initial_data(&self) -> io::Result<bool> {
        let config = self.share_dir.join("config.json");
        let config_exists = match std::fs::symlink_metadata(&config) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        let workspaces = self.share_dir.join("workspaces");
        let workspaces_exists = match std::fs::metadata(&workspaces) {
            Ok(metadata) => metadata.is_dir(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        Ok(config_exists || workspaces_exists)
    }

    #[cfg(test)]
    pub(crate) fn ensure_initialized(&self, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        self.ensure_initialized_with_timeout(INITIALIZE_TIMEOUT, cancelled)
    }

    pub(crate) fn ensure_initialized_with_state(
        &self,
        state: &crate::install::InstallState,
        pid_file: &Path,
    ) -> Result<(), String> {
        if self
            .has_initial_data()
            .map_err(|error| format!("检查 QwenPaw 数据目录失败: {error}"))?
        {
            return Ok(());
        }
        let mut command = Command::new(self.python());
        command
            .args(["-m", "qwenpaw", "init", "--defaults"])
            .current_dir(&self.share_dir)
            .env("QWENPAW_WORKING_DIR", &self.share_dir)
            .env("QWENPAW_LOG_LEVEL", &self.log_level)
            .env("QWENPAW_AUTH_ENABLED", "true")
            .env(
                "QWENPAW_DEPENDENCY_STATE_DIR",
                self.share_dir.join("dependency-environments"),
            )
            .env("PATH", NODE_PATH);
        let output = state
            .run_cancellable_command_with_input(
                command,
                INITIALIZE_TIMEOUT,
                "QwenPaw 初始化",
                pid_file,
                Some(b"Yes\n"),
            )
            .map_err(|error| format!("QwenPaw 初始化失败: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!("QwenPaw 初始化退出失败: {}", output.status))
        }
    }

    #[cfg(test)]
    fn ensure_initialized_with_timeout(
        &self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if cancelled() {
            return Err("QwenPaw 初始化已取消".into());
        }
        if self
            .has_initial_data()
            .map_err(|error| format!("检查 QwenPaw 数据目录失败: {error}"))?
        {
            return Ok(());
        }

        let mut command = Command::new(self.python());
        command
            .args(["-m", "qwenpaw", "init", "--defaults"])
            .current_dir(&self.share_dir)
            .env("QWENPAW_WORKING_DIR", &self.share_dir)
            .env("QWENPAW_LOG_LEVEL", &self.log_level)
            .env("QWENPAW_AUTH_ENABLED", "true")
            .env(
                "QWENPAW_DEPENDENCY_STATE_DIR",
                self.share_dir.join("dependency-environments"),
            )
            .env("PATH", NODE_PATH);
        run_init_command(command, timeout, cancelled)
    }
}

#[cfg(test)]
fn run_init_command(
    mut command: Command,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动 QwenPaw 初始化失败: {error}"))?;
    let pid = child.id() as i32;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| "无法连接 QwenPaw 初始化输入".to_string())?;
    if let Err(error) = input.write_all(b"Yes\n") {
        terminate_process_group(&mut child, pid);
        return Err(format!("发送 QwenPaw 初始化确认失败: {error}"));
    }
    drop(input);

    let started = Instant::now();
    loop {
        if cancelled() {
            terminate_process_group(&mut child, pid);
            return Err("QwenPaw 初始化已取消".into());
        }
        if started.elapsed() >= timeout {
            terminate_process_group(&mut child, pid);
            return Err(format!(
                "QwenPaw 初始化超时({}s),已终止",
                timeout.as_secs_f64()
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("QwenPaw 初始化退出失败: {status}"))
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                terminate_process_group(&mut child, pid);
                return Err(format!("等待 QwenPaw 初始化失败: {error}"));
            }
        }
    }
}

#[cfg(test)]
fn terminate_process_group(child: &mut std::process::Child, pid: i32) {
    unsafe { libc::kill(-pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::RuntimeAdapter;
    use std::{
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };

    struct RuntimeFixture {
        root: PathBuf,
        adapter: RuntimeAdapter,
        python: PathBuf,
    }

    impl RuntimeFixture {
        fn new(label: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "qwenpaw-runtime-{label}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let venv = root.join("home/venv");
            let python = venv.join("bin/python3");
            let share = root.join("share");
            std::fs::create_dir_all(python.parent().unwrap()).unwrap();
            std::fs::create_dir_all(&share).unwrap();
            Self {
                adapter: RuntimeAdapter::new(&venv, &share, "debug"),
                root,
                python,
            }
        }

        fn write_python(&self, script: &str) {
            std::fs::write(&self.python, script).unwrap();
            std::fs::set_permissions(&self.python, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    impl Drop for RuntimeFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn init_uses_fixed_argv_stdin_and_kernel_environment() {
        let fixture = RuntimeFixture::new("init-contract");
        fixture.write_python(&format!(
            "#!/bin/sh\nroot='{}'\nprintf '%s\\n' \"$@\" > \"$root/argv\"\ncat > \"$root/stdin\"\nprintf '%s\\n' \"$QWENPAW_WORKING_DIR\" \"$QWENPAW_LOG_LEVEL\" \"$QWENPAW_AUTH_ENABLED\" \"$QWENPAW_DEPENDENCY_STATE_DIR\" \"$PATH\" > \"$root/env\"\npwd > \"$root/cwd\"\n",
            fixture.root.display()
        ));

        fixture.adapter.ensure_initialized(&|| false).unwrap();

        let share = fixture.root.join("share");
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("argv")).unwrap(),
            "-m\nqwenpaw\ninit\n--defaults\n"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("stdin")).unwrap(),
            "Yes\n"
        );
        let env: Vec<_> = std::fs::read_to_string(fixture.root.join("env"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(env[0], share.display().to_string());
        assert_eq!(env[1], "debug");
        assert_eq!(env[2], "true");
        assert_eq!(
            env[3],
            share.join("dependency-environments").display().to_string()
        );
        assert_eq!(
            env[4],
            "/var/apps/nodejs_v24/target/bin:/usr/local/bin:/usr/bin:/bin"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("cwd"))
                .unwrap()
                .trim(),
            share.to_string_lossy()
        );
    }

    #[test]
    fn existing_config_or_workspaces_skips_init_without_invoking_python() {
        for marker in ["config.json", "workspaces"] {
            let fixture = RuntimeFixture::new(&format!("existing-{marker}"));
            fixture.write_python(&format!(
                "#!/bin/sh\ntouch '{}/called'\n",
                fixture.root.display()
            ));
            let path = fixture.root.join("share").join(marker);
            if marker == "workspaces" {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, "existing user data").unwrap();
            }

            fixture.adapter.ensure_initialized(&|| false).unwrap();
            assert!(!fixture.root.join("called").exists(), "marker={marker}");
        }
    }

    #[test]
    fn metadata_probe_requires_both_package_metadata_and_import() {
        let fixture = RuntimeFixture::new("metadata-probe");
        fixture.write_python(&format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}/home/probe-args'\nprintf '2.3.0\\n'\n",
            fixture.root.display()
        ));

        assert_eq!(
            RuntimeAdapter::metadata_version(&fixture.python).unwrap(),
            "2.3.0"
        );
        let args = std::fs::read_to_string(fixture.root.join("home/probe-args")).unwrap();
        assert!(args.starts_with("-c\n"));
        assert!(args.contains("importlib.metadata"));
        assert!(args.contains("import qwenpaw"));
    }

    #[test]
    fn init_timeout_terminates_the_command_process_group() {
        let fixture = RuntimeFixture::new("init-timeout");
        fixture.write_python(&format!(
            "#!/bin/sh\ntouch '{}/started'\nexec /bin/sleep 30\n",
            fixture.root.display()
        ));
        let started = Instant::now();

        let result = fixture
            .adapter
            .ensure_initialized_with_timeout(Duration::from_millis(150), &|| false);

        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(fixture.root.join("started").exists());
    }

    #[test]
    fn init_cancellation_terminates_the_command_process_group() {
        let fixture = RuntimeFixture::new("init-cancel");
        fixture.write_python(&format!(
            "#!/bin/sh\ntouch '{}/started'\nexec /bin/sleep 30\n",
            fixture.root.display()
        ));
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let adapter = fixture.adapter.clone();
        let worker = std::thread::spawn(move || {
            adapter.ensure_initialized_with_timeout(Duration::from_secs(10), &|| {
                flag.load(Ordering::SeqCst)
            })
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.root.join("started").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(fixture.root.join("started").exists(), "init did not start");
        cancelled.store(true, Ordering::SeqCst);
        let started = Instant::now();
        assert!(worker.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
