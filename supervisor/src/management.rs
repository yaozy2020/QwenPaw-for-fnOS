use std::{fmt, net::TcpStream, path::Path, sync::Arc, time::Duration};

use crate::{
    events::Events,
    install::{InstallChannel, InstallSnapshot, InstallState},
    kernel::KernelManager,
    runtime::RuntimeAdapter,
};

pub type OperationGuard = tokio::sync::OwnedMutexGuard<()>;

#[derive(Debug, PartialEq, Eq)]
pub enum ManagementError {
    OperationInProgress,
    RecoveryPending(String),
    InstallBusy,
    InvalidSettings(String),
    OperationFailed(String),
}

impl fmt::Display for ManagementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OperationInProgress => write!(formatter, "另一个运维操作正在进行中,请稍后重试"),
            Self::RecoveryPending(reason) => write!(formatter, "恢复尚未完成: {reason}"),
            Self::InstallBusy => write!(formatter, "内核安装正在进行中,当前操作已被拒绝"),
            Self::InvalidSettings(message) | Self::OperationFailed(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for ManagementError {}

#[derive(Clone)]
pub struct ManagementCore {
    pub kernel: Arc<KernelManager>,
    pub install: InstallState,
    pub events: Events,
    pub var_dir: String,
    pub home_dir: String,
    pub share_dir: String,
    pub etc_dir: String,
    pub ui_config_file: String,
    pub kernel_log: String,
    pub operations: Arc<tokio::sync::Mutex<()>>,
    pub supervisor_port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallAcceptance {
    Started { seq: u64 },
    Busy { seq: u64 },
}

impl ManagementCore {
    pub fn try_operation(&self) -> Result<OperationGuard, ManagementError> {
        Self::try_operation_lock(&self.operations)
    }

    pub fn try_operation_lock(
        operations: &Arc<tokio::sync::Mutex<()>>,
    ) -> Result<OperationGuard, ManagementError> {
        operations
            .clone()
            .try_lock_owned()
            .map_err(|_| ManagementError::OperationInProgress)
    }

    pub fn ensure_recovery_ready(&self) -> Result<(), ManagementError> {
        if let Some(reason) = self.install.recovery_error() {
            return Err(ManagementError::RecoveryPending(reason));
        }
        Ok(())
    }

    pub fn set_mirror(&self, value: &str) -> Result<(), ManagementError> {
        let _operation = self.begin_operation()?;
        crate::mirror::write(&self.var_dir, value).map_err(|error| match error {
            crate::mirror::MirrorError::Invalid(message) => {
                ManagementError::InvalidSettings(message)
            }
            crate::mirror::MirrorError::Io(error) => {
                ManagementError::OperationFailed(format!("写入 pip-mirror 失败: {error}"))
            }
        })
    }

    pub fn install_snapshot(&self) -> InstallSnapshot {
        self.install.snapshot()
    }

    pub fn resolve_install_request(
        version: &str,
        mirror: &str,
    ) -> Result<(InstallChannel, String), ManagementError> {
        let channel = InstallChannel::parse(version).ok_or_else(|| {
            ManagementError::InvalidSettings(
                "版本号非法:仅接受 latest、beta 或合法的精确版本号".into(),
            )
        })?;
        let mirror =
            crate::install::resolve_mirror(mirror).map_err(ManagementError::InvalidSettings)?;
        Ok((channel, mirror))
    }

    pub async fn submit_install(
        &self,
        version: &str,
        mirror: &str,
    ) -> Result<InstallAcceptance, ManagementError> {
        let (channel, mirror) = Self::resolve_install_request(version, mirror)?;
        let operation = self.try_operation()?;
        self.submit_install_prepared(channel, mirror, operation)
            .await
    }

    pub async fn submit_install_prepared(
        &self,
        channel: InstallChannel,
        mirror: String,
        _operation: OperationGuard,
    ) -> Result<InstallAcceptance, ManagementError> {
        self.ensure_recovery_ready()?;
        let snapshot = self.install_snapshot();
        if self.install.is_busy() {
            return Ok(InstallAcceptance::Busy { seq: snapshot.seq });
        }

        self.kernel.stop().await;
        if let Some(error) = self.kernel.stop_verification_error() {
            return Err(ManagementError::OperationFailed(error));
        }
        let started = self.install.try_start(
            self.runtime_adapter(),
            self.kernel.port(),
            channel,
            mirror,
            format!("{}/qwenpaw-upgrade.log.pip", self.var_dir),
            format!("{}/qwenpaw-version", self.var_dir),
        );
        if started {
            Ok(InstallAcceptance::Started {
                seq: self.install_snapshot().seq,
            })
        } else if self.install.is_busy() {
            Ok(InstallAcceptance::Busy {
                seq: self.install_snapshot().seq,
            })
        } else if let Some(reason) = self.install.recovery_error() {
            Err(ManagementError::RecoveryPending(reason))
        } else {
            Err(ManagementError::OperationFailed("无法受理内核安装".into()))
        }
    }

    fn runtime_adapter(&self) -> RuntimeAdapter {
        RuntimeAdapter::new(
            Path::new(&self.home_dir).join("venv"),
            &self.share_dir,
            &self.etc_dir,
            self.kernel.log_level(),
        )
    }

    pub fn runtime_health(&self) -> serde_json::Value {
        let adapter = self.runtime_adapter();
        let (version, metadata_error, metadata_repairable) = match adapter.metadata_version() {
            Ok(version) => (version, None, false),
            Err(error) => (String::new(), Some(error.message), error.repairable),
        };
        let (initialized, data_error) = match adapter.has_initial_data() {
            Ok(initialized) => (initialized, None),
            Err(error) => (false, Some(format!("检查 QwenPaw 数据目录失败: {error}"))),
        };
        let status = self.kernel.status_now();
        let reachable = status.running
            && TcpStream::connect_timeout(
                &([127, 0, 0, 1], self.kernel.port()).into(),
                Duration::from_millis(300),
            )
            .is_ok();
        let error = metadata_error.or(data_error);
        serde_json::json!({
            "installed": self.kernel.installed(),
            "version": version,
            "initialized": initialized,
            "running": status.running,
            "reachable": reachable,
            "healthy": error.is_none() && !version.is_empty() && initialized && reachable,
            "error": error,
            "metadata_repairable": metadata_repairable,
            "port": self.kernel.port(),
        })
    }

    async fn restart_kernel(&self) -> Result<(), String> {
        let kernel = Arc::clone(&self.kernel);
        tokio::task::spawn_blocking(move || kernel.spawn())
            .await
            .map_err(|error| format!("启动内核任务失败: {error}"))??;
        let kernel = Arc::clone(&self.kernel);
        let healthy =
            tokio::task::spawn_blocking(move || kernel.wait_healthy(Duration::from_secs(10)))
                .await
                .map_err(|error| format!("内核健康检查任务失败: {error}"))?;
        if healthy {
            Ok(())
        } else {
            self.kernel.stop().await;
            Err("内核启动后健康检查未通过".into())
        }
    }

    pub async fn ensure_initialized(&self) -> Result<(), ManagementError> {
        let _operation = self.begin_operation()?;
        let was_running = self.kernel.status_now().running;
        self.kernel.stop().await;
        let adapter = self.runtime_adapter();
        let install = self.install.clone();
        let pid_file = Path::new(&self.var_dir).join("install.pid");
        let initialized = match tokio::task::spawn_blocking(move || {
            adapter.ensure_initialized_with_state(&install, &pid_file)
        })
        .await
        {
            Ok(result) => result,
            Err(error) => Err(format!("QwenPaw 初始化任务失败: {error}")),
        };
        if let Err(error) = initialized {
            let restore = if was_running {
                self.restart_kernel().await.err()
            } else {
                None
            };
            let message = match restore {
                Some(restore_error) => {
                    format!("QwenPaw 初始化失败: {error}；原内核恢复失败: {restore_error}")
                }
                None => format!("QwenPaw 初始化失败: {error}"),
            };
            return Err(ManagementError::OperationFailed(message));
        }
        if was_running {
            self.restart_kernel()
                .await
                .map_err(ManagementError::OperationFailed)?;
        }
        Ok(())
    }

    pub fn begin_operation(&self) -> Result<OperationGuard, ManagementError> {
        let guard = self.try_operation()?;
        self.ensure_recovery_ready()?;
        if self.install.is_busy() {
            return Err(ManagementError::InstallBusy);
        }
        Ok(guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::KernelConfig;

    fn core() -> ManagementCore {
        let root =
            std::env::temp_dir().join(format!("qwenpaw-management-core-{}", std::process::id()));
        let port = crate::api::test_support::free_port();
        ManagementCore {
            kernel: Arc::new(KernelManager::new(
                KernelConfig {
                    venv_dir: root.join("home/venv").to_string_lossy().into_owned(),
                    pid_file: root.join("var/kernel.pid").to_string_lossy().into_owned(),
                    log_file: root.join("var/kernel.log").to_string_lossy().into_owned(),
                    share_dir: root.join("share").to_string_lossy().into_owned(),
                    etc_dir: root.join("etc").to_string_lossy().into_owned(),
                    log_level: "info".into(),
                },
                port,
            )),
            install: InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0))),
            events: Events::new(),
            var_dir: root.join("var").to_string_lossy().into_owned(),
            home_dir: root.join("home").to_string_lossy().into_owned(),
            share_dir: root.join("share").to_string_lossy().into_owned(),
            etc_dir: root.join("etc").to_string_lossy().into_owned(),
            ui_config_file: root.join("ui/config").to_string_lossy().into_owned(),
            kernel_log: root.join("var/kernel.log").to_string_lossy().into_owned(),
            operations: Arc::new(tokio::sync::Mutex::new(())),
            supervisor_port: crate::api::test_support::free_port_other_than(port),
        }
    }

    #[test]
    fn operation_gate_returns_transport_neutral_structured_errors() {
        let core = core();
        let guard = core.begin_operation().unwrap();
        assert_eq!(
            core.begin_operation().err(),
            Some(ManagementError::OperationInProgress)
        );
        drop(guard);

        core.install.block_recovery("unfinished transaction".into());
        assert_eq!(
            core.begin_operation().err(),
            Some(ManagementError::RecoveryPending(
                "unfinished transaction".into()
            ))
        );
    }
}
