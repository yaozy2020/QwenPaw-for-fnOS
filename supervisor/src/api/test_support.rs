use std::sync::Arc;

use super::{AppState, AuthCache};
use crate::{events::Events, install::InstallState, kernel::{KernelConfig, KernelManager}};

pub(crate) struct TestDir(pub(crate) std::path::PathBuf);

impl TestDir {
    pub(crate) fn new(label: &str) -> Self {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "qwenpaw-api-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(crate) fn state(&self, port: u16) -> AppState {
        let home = self.0.join("home");
        let var = self.0.join("var");
        let share = self.0.join("share");
        for path in [&home, &var, &share] {
            std::fs::create_dir_all(path).unwrap();
        }
        AppState {
            kernel: Arc::new(KernelManager::new(KernelConfig {
                venv_dir: home.join("venv").to_string_lossy().into_owned(),
                pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
                log_file: var.join("kernel.log").to_string_lossy().into_owned(),
                share_dir: share.to_string_lossy().into_owned(),
                etc_dir: self.0.to_string_lossy().into_owned(),
                log_level: "info".into(),
            }, port)),
            install: InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0))),
            events: Events::new(),
            token: "test-token".into(),
            var_dir: var.to_string_lossy().into_owned(),
            home_dir: home.to_string_lossy().into_owned(),
            share_dir: share.to_string_lossy().into_owned(),
            ui_config_file: self.0.join("ui-config").to_string_lossy().into_owned(),
            kernel_log: var.join("kernel.log").to_string_lossy().into_owned(),
            operations: Arc::new(tokio::sync::Mutex::new(())),
            auth_cache: Arc::new(std::sync::Mutex::new(AuthCache::default())),
            supervisor_port: 19090,
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn install_python(dir: &TestDir, state: &AppState, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = std::path::Path::new(&state.home_dir).join("venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let python = bin.join("python3");
    std::fs::write(&python, format!(
        "#!/usr/bin/python3\nimport pathlib, sys\nroot = pathlib.Path({})\nif '-c' in sys.argv:\n    print('2.3.0')\n    sys.exit(0)\n{body}",
        serde_json::to_string(&dir.0.to_string_lossy()).unwrap()
    )).unwrap();
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
    let python_template = serde_json::to_string(&python.to_string_lossy()).unwrap();
    let base_python = dir.0.join("base-python");
    std::fs::write(
        &base_python,
        format!(
            "#!/usr/bin/python3\nimport pathlib, shutil, sys\nif len(sys.argv) < 4 or sys.argv[1:3] != ['-m', 'venv']:\n    sys.exit(2)\nbin_dir = pathlib.Path(sys.argv[3]) / 'bin'\nbin_dir.mkdir(parents=True)\nshutil.copy2({python_template}, bin_dir / 'python3')\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&base_python, std::fs::Permissions::from_mode(0o755)).unwrap();
    state.install.set_base_python(base_python);
    python
}

pub(crate) struct ChildGuard(pub(crate) std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) struct KernelGuard(pub(crate) Arc<KernelManager>);

impl Drop for KernelGuard {
    fn drop(&mut self) {
        self.0.stop_sync();
    }
}
