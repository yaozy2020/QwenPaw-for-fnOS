use std::{path::Path, sync::Arc, time::Duration};

use crate::{
    management::{ManagementCore, ManagementError},
    settings_transaction::SettingsTransaction,
};

#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

impl SettingsPatch {
    pub fn validate(&self, supervisor_port: u16) -> Result<(), ManagementError> {
        if let Some(port) = self.port {
            if port < 1024 || port == supervisor_port || port == 19091 {
                return Err(ManagementError::InvalidSettings(format!(
                    "端口无效(1024~65535,且不可为监督者保留端口 {supervisor_port}/19091)"
                )));
            }
        }
        if let Some(level) = &self.log_level {
            if !matches!(
                level.as_str(),
                "critical" | "error" | "warning" | "info" | "debug" | "trace"
            ) {
                return Err(ManagementError::InvalidSettings("日志级别无效".into()));
            }
        }
        Ok(())
    }
}

pub struct ApplyOutcome {
    pub result: Result<String, ManagementError>,
    pub notify_observers: bool,
}

fn outcome(result: Result<String, ManagementError>, notify_observers: bool) -> ApplyOutcome {
    ApplyOutcome {
        result,
        notify_observers,
    }
}

fn failed(message: impl Into<String>) -> ManagementError {
    ManagementError::OperationFailed(message.into())
}

async fn spawn_healthy(core: &ManagementCore) -> Result<(), String> {
    let kernel = Arc::clone(&core.kernel);
    tokio::task::spawn_blocking(move || {
        kernel.spawn()?;
        if kernel.wait_healthy(Duration::from_secs(10)) {
            Ok(())
        } else {
            Err("内核健康检查未通过".into())
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn apply(core: &ManagementCore, patch: SettingsPatch) -> ApplyOutcome {
    if let Err(error) = patch.validate(core.supervisor_port) {
        return outcome(Err(error), false);
    }
    if patch.port.is_none() && patch.log_level.is_none() {
        return outcome(Ok("配置未更改".into()), false);
    }

    let old_port = core.kernel.port();
    let old_level = core.kernel.log_level();
    let new_port = patch.port.unwrap_or(old_port);
    let new_level = patch.log_level.unwrap_or_else(|| old_level.clone());
    let restart = old_port != new_port || old_level != new_level;
    let installed = {
        let kernel = Arc::clone(&core.kernel);
        match tokio::task::spawn_blocking(move || kernel.installed()).await {
            Ok(installed) => installed,
            Err(error) => return outcome(Err(failed(error.to_string())), false),
        }
    };
    let var = core.var_dir.clone();
    let ui = core.ui_config_file.clone();
    let transaction = match tokio::task::spawn_blocking(move || {
        SettingsTransaction::begin(Path::new(&var), Path::new(&ui))
    })
    .await
    {
        Ok(Ok(transaction)) => Arc::new(transaction),
        Ok(Err(error)) => return outcome(Err(failed(format!("无法建立配置事务: {error}"))), false),
        Err(error) => return outcome(Err(failed(error.to_string())), false),
    };

    if restart {
        core.kernel.stop().await;
    }
    core.kernel.set_port(new_port);
    if let Err(error) = core.kernel.set_log_level(&new_level) {
        return outcome(Err(failed(error)), false);
    }

    let result = async {
        let var = core.var_dir.clone();
        let level = new_level.clone();
        tokio::task::spawn_blocking(move || {
            crate::install::atomic_write_0600(
                &format!("{var}/service_port"),
                &format!("{new_port}\n"),
            )?;
            crate::install::atomic_write_0600(
                &format!("{var}/qwenpaw-log-level"),
                &format!("{level}\n"),
            )
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("写入配置失败: {error}"))?;
        if installed && restart {
            spawn_healthy(core).await?;
        }
        let ui = core.ui_config_file.clone();
        tokio::task::spawn_blocking(move || {
            crate::desktop::sync_main_entry(Path::new(&ui), new_port, installed)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("桌面入口同步失败: {error}"))?;
        let transaction = Arc::clone(&transaction);
        tokio::task::spawn_blocking(move || transaction.commit())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("提交配置事务失败: {error}"))?;
        Ok::<(), String>(())
    }
    .await;

    let result = match result {
        Ok(()) => Ok(if !restart {
            "配置已同步,无需重启"
        } else if installed {
            "配置已更新,内核健康检查通过"
        } else {
            "配置已保存,内核尚未安装"
        }
        .into()),
        Err(error) => {
            if restart {
                core.kernel.stop().await;
            }
            let rollback = tokio::task::spawn_blocking(move || transaction.rollback())
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()));
            match rollback {
                Err(rollback_error) => {
                    core.kernel.stop().await;
                    let reason = format!("{error}; 配置恢复失败: {rollback_error}");
                    core.install.block_recovery(reason.clone());
                    Err(reason)
                }
                Ok(()) => {
                    core.kernel.set_port(old_port);
                    let _ = core.kernel.set_log_level(&old_level);
                    let restored = if installed && restart {
                        spawn_healthy(core).await
                    } else {
                        Ok(())
                    };
                    match restored {
                        Ok(()) => Err(format!("{error}; 已恢复原配置")),
                        Err(restore_error) => Err(format!(
                            "{error}; 原配置已恢复,但旧内核未就绪: {restore_error}"
                        )),
                    }
                }
            }
        }
    }
    .map_err(failed);
    outcome(result, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::Events,
        install::InstallState,
        kernel::{KernelConfig, KernelManager},
        management::ManagementCore,
    };
    use std::{path::PathBuf, sync::Arc};

    fn fixture(
        name: &str,
        fail_start: bool,
    ) -> (
        PathBuf,
        ManagementCore,
        crate::api::test_support::TestServer,
    ) {
        let root = std::env::temp_dir().join(format!(
            "qwenpaw-settings-core-{name}-{}",
            std::process::id()
        ));
        let home = root.join("home");
        let var = root.join("var");
        std::fs::create_dir_all(home.join("venv/bin")).unwrap();
        std::fs::create_dir_all(&var).unwrap();
        let script = if fail_start {
            "#!/usr/bin/env python3\nimport sys\nif sys.argv[1] == '-c': print('1.0.0'); sys.exit(0)\nsys.exit(1)\n"
        } else {
            "#!/usr/bin/env python3\nimport sys, time\nif sys.argv[1] == '-c': print('1.0.0'); sys.exit(0)\ntime.sleep(3600)\n"
        };
        let python = home.join("venv/bin/python3");
        std::fs::write(&python, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let port = crate::api::test_support::free_port();
        let server = crate::api::test_support::serve_reserved_port(port);
        let ui = root.join("ui-config");
        std::fs::write(&ui, format!(r#"{{".url":{{"qwenpaw.main":{{"port":"{port}","noDisplay":false}},"qwenpaw.console":{{"port":"15666"}}}}}}"#)).unwrap();
        std::fs::write(var.join("service_port"), format!("{port}\n")).unwrap();
        std::fs::write(var.join("qwenpaw-log-level"), "info\n").unwrap();
        let kernel_log = var.join("kernel.log").to_string_lossy().into_owned();
        let core = ManagementCore {
            kernel: Arc::new(KernelManager::new(
                KernelConfig {
                    venv_dir: home.join("venv").to_string_lossy().into_owned(),
                    pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
                    log_file: kernel_log.clone(),
                    share_dir: root.to_string_lossy().into_owned(),
                    etc_dir: root.to_string_lossy().into_owned(),
                    log_level: "info".into(),
                },
                port,
            )),
            install: InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0))),
            events: Events::new(),
            var_dir: var.to_string_lossy().into_owned(),
            home_dir: home.to_string_lossy().into_owned(),
            share_dir: root.to_string_lossy().into_owned(),
            ui_config_file: ui.to_string_lossy().into_owned(),
            kernel_log,
            operations: Arc::new(tokio::sync::Mutex::new(())),
            supervisor_port: crate::api::test_support::free_port_other_than(port),
        };
        (root, core, server)
    }

    #[tokio::test]
    async fn batch_settings_commit_after_real_health_check() {
        let (root, core, _old_server) = fixture("success", false);
        let port = crate::api::test_support::free_port();
        let _new_server = crate::api::test_support::serve_reserved_port(port);
        let patch = SettingsPatch {
            port: Some(port),
            log_level: Some("debug".into()),
        };
        patch.validate(core.supervisor_port).unwrap();
        let applied = apply(&core, patch).await;
        assert!(applied.notify_observers);
        applied.result.unwrap();
        assert_eq!(core.kernel.port(), port);
        assert_eq!(core.kernel.log_level(), "debug");
        assert_eq!(
            std::fs::read_to_string(root.join("var/service_port")).unwrap(),
            format!("{port}\n")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("var/qwenpaw-log-level")).unwrap(),
            "debug\n"
        );
        let ui: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&core.ui_config_file).unwrap()).unwrap();
        assert_eq!(ui[".url"]["qwenpaw.main"]["port"], port.to_string());
        assert_eq!(ui[".url"]["qwenpaw.console"]["port"], "15666");
        assert!(!root.join("var/settings-transaction.json").exists());
        core.kernel.stop().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn failed_restart_restores_files_and_memory_together() {
        use std::os::unix::fs::PermissionsExt;

        let (root, core, _old_server) = fixture("failure", true);
        let old_port = core.kernel.port();
        let old_ui = std::fs::read(&core.ui_config_file).unwrap();
        let files = [
            (root.join("var/service_port"), 0o640),
            (root.join("var/qwenpaw-log-level"), 0o600),
            (PathBuf::from(&core.ui_config_file), 0o644),
        ];
        for (path, mode) in &files {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode)).unwrap();
        }
        let new_port = crate::api::test_support::free_port_other_than(old_port);
        let new_server = crate::api::test_support::serve_reserved_port(new_port);
        drop(new_server);
        let patch = SettingsPatch {
            port: Some(new_port),
            log_level: Some("debug".into()),
        };
        let applied = apply(&core, patch).await;
        assert!(applied.result.is_err());
        assert!(applied.notify_observers);
        assert_eq!(core.kernel.port(), old_port);
        assert_eq!(core.kernel.log_level(), "info");
        assert_eq!(
            std::fs::read_to_string(root.join("var/service_port")).unwrap(),
            format!("{old_port}\n")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("var/qwenpaw-log-level")).unwrap(),
            "info\n"
        );
        assert_eq!(std::fs::read(&core.ui_config_file).unwrap(), old_ui);
        for (path, mode) in files {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
        assert!(!root.join("var/settings-transaction.json").exists());
        core.kernel.stop().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn unchanged_settings_do_not_restart_a_running_kernel() {
        let (root, core, _server) = fixture("unchanged", false);
        let before = core.kernel.spawn().unwrap();
        assert!(core.kernel.wait_healthy(Duration::from_secs(3)));
        let outcome = apply(
            &core,
            SettingsPatch {
                port: Some(core.kernel.port()),
                log_level: Some("info".into()),
            },
        )
        .await;
        assert!(outcome.result.is_ok());
        assert!(outcome.notify_observers);
        let after = core.kernel.status_now().pid;
        core.kernel.stop().await;
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn empty_patch_does_not_notify_observers() {
        let (root, core, _server) = fixture("empty", false);
        let outcome = apply(&core, SettingsPatch::default()).await;
        assert_eq!(outcome.result.unwrap(), "配置未更改");
        assert!(!outcome.notify_observers);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_reject_reserved_ports_and_unknown_log_levels() {
        for port in [0, 80, 19090, 19091] {
            assert!(SettingsPatch {
                port: Some(port),
                log_level: None
            }
            .validate(19090)
            .is_err());
        }
        assert!(SettingsPatch {
            port: None,
            log_level: Some("verbose".into())
        }
        .validate(19090)
        .is_err());
    }
}
