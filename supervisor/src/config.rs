//! 路径解析:TRIM_* 注入优先(官方规范),否则回退 fnOS 应用目录约定。
//! 运行时自产文件统一挂 var_dir 之下;生命周期锁也置于其中,确保应用用户可创建并持有互斥锁。

use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub var_dir: String,
    pub etc_dir: String,
    pub home_dir: String,
    pub share_dir: String,
    pub ui_config_file: String,
    pub supervisor_binary: PathBuf,
    pub kernel_venv: String,
    pub kernel_pid_file: String,
    pub kernel_log: String,
    pub kernel_port: u16,
    pub web_proxy_port: Option<u16>,
    pub kernel_log_level: String,
    pub token_file: String,
    pub supervisor_pid_file: String,
    pub supervisor_log: String,
    pub listen_port: u16,
    pub auto_start: bool,
    pub gateway_socket: Option<PathBuf>,
}

#[derive(Default)]
struct ConfigValues {
    var_dir: Option<String>,
    etc_dir: Option<String>,
    home_dir: Option<String>,
    data_share_paths: Option<String>,
    service_port: Option<String>,
    service_port_file: Option<String>,
    web_proxy_port: Option<String>,
    kernel_log_level_file: Option<String>,
    supervisor_port: Option<String>,
    auto_start: Option<String>,
    app_dest: Option<String>,
    gateway_socket: Option<PathBuf>,
}

fn value_or(value: Option<&str>, fallback: &str) -> String {
    match value {
        Some(value) if !value.trim().is_empty() => value.to_string(),
        _ => fallback.to_string(),
    }
}

fn parse_port(value: Option<&str>, fallback: u16) -> u16 {
    value
        .and_then(|value| value.trim().parse().ok())
        .filter(|&value| (1024..=65535).contains(&value))
        .unwrap_or(fallback)
}

fn resolve_gateway_socket(explicit: Option<PathBuf>, app_dest: Option<&str>) -> Option<PathBuf> {
    explicit.or_else(|| {
        app_dest
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(path).join("app.sock"))
    })
}

fn target_supervisor_binary(app_dest: &str) -> PathBuf {
    PathBuf::from(app_dest).join("bin/qwenpaw-supervisor")
}

fn parse_kernel_log_level(contents: Option<&str>) -> String {
    const ALLOWED: [&str; 6] = ["critical", "error", "warning", "info", "debug", "trace"];
    contents
        .map(str::trim)
        .map(str::to_lowercase)
        .filter(|value| ALLOWED.contains(&value.as_str()))
        .unwrap_or_else(|| "info".to_string())
}

fn parse_config(values: ConfigValues) -> (Config, Option<String>) {
    let var_dir = value_or(values.var_dir.as_deref(), "/var/apps/qwenpaw/var");
    let etc_dir = value_or(values.etc_dir.as_deref(), "/var/apps/qwenpaw/etc");
    let home_dir = value_or(values.home_dir.as_deref(), "/var/apps/qwenpaw/home");
    let first_share = values
        .data_share_paths
        .as_deref()
        .unwrap_or_default()
        .split(':')
        .find(|path| !path.trim().is_empty());
    let share_dir = first_share.map_or_else(
        || "/var/apps/qwenpaw/shares/qwenpaw/.qwenpaw".to_string(),
        |path| format!("{}/.qwenpaw", path.trim_end_matches('/')),
    );

    let file_port = values
        .service_port_file
        .as_deref()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|value| (1024..=65535).contains(value) && *value != 19091);
    let configured_kernel_port =
        file_port.unwrap_or_else(|| parse_port(values.service_port.as_deref(), 18088));
    let listen_port = parse_port(values.supervisor_port.as_deref(), 19090);
    let (kernel_port, warning) = if configured_kernel_port == listen_port {
        let fallback = if listen_port == 18088 { 18089 } else { 18088 };
        (
            fallback,
            Some(format!(
                "[Supervisor] service_port({configured_kernel_port}) 与监督者监听端口冲突,忽略该文件值,回退到 {fallback}"
            )),
        )
    } else {
        (configured_kernel_port, None)
    };
    let app_dest = value_or(values.app_dest.as_deref(), "/var/apps/qwenpaw/target");
    let gateway_socket = resolve_gateway_socket(values.gateway_socket, values.app_dest.as_deref());

    (
        Config {
            var_dir: var_dir.clone(),
            etc_dir,
            home_dir: home_dir.clone(),
            share_dir,
            ui_config_file: format!("{app_dest}/ui/config"),
            supervisor_binary: target_supervisor_binary(&app_dest),
            kernel_venv: format!("{home_dir}/venv"),
            kernel_pid_file: format!("{var_dir}/kernel.pid"),
            kernel_log: format!("{var_dir}/qwenpaw.log"),
            kernel_port,
            web_proxy_port: match values.web_proxy_port.as_deref().map(str::trim) {
                Some("off") => None,
                Some(value) => Some(parse_port(Some(value), 0)),
                None => Some(0),
            },
            kernel_log_level: parse_kernel_log_level(values.kernel_log_level_file.as_deref()),
            token_file: format!("{var_dir}/supervisor-token"),
            supervisor_pid_file: format!("{var_dir}/supervisor.pid"),
            supervisor_log: format!("{var_dir}/supervisor.log"),
            listen_port,
            auto_start: values.auto_start.as_deref().unwrap_or_default() != "0",
            gateway_socket,
        },
        warning,
    )
}

impl Config {
    pub fn stable_helper_binary(&self) -> PathBuf {
        let package_root = self
            .supervisor_binary
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("/var/apps/qwenpaw"));
        package_root.join("cmd/qwenpaw-supervisor")
    }

    pub fn load() -> Self {
        let raw_var_dir = env::var("TRIM_PKGVAR").ok();
        let effective_var_dir = value_or(raw_var_dir.as_deref(), "/var/apps/qwenpaw/var");
        let values = ConfigValues {
            var_dir: raw_var_dir,
            etc_dir: env::var("TRIM_PKGETC").ok(),
            home_dir: env::var("TRIM_PKGHOME").ok(),
            data_share_paths: env::var("TRIM_DATA_SHARE_PATHS").ok(),
            service_port: env::var("TRIM_SERVICE_PORT").ok(),
            service_port_file: std::fs::read_to_string(
                PathBuf::from(&effective_var_dir).join("service_port"),
            )
            .ok(),
            web_proxy_port: env::var("QWENPAW_WEB_PROXY_PORT")
                .ok()
                .or_else(|| {
                    std::fs::read_to_string(
                        PathBuf::from(&effective_var_dir).join("web_proxy_port"),
                    )
                    .ok()
                })
                .or_else(|| Some("0".into())),
            kernel_log_level_file: Some(load_kernel_log_level(&effective_var_dir)),
            supervisor_port: env::var("SUPERVISOR_PORT").ok(),
            auto_start: env::var("SUPERVISOR_AUTOSTART").ok(),
            app_dest: env::var("TRIM_APPDEST").ok(),
            gateway_socket: env::var_os("QWENPAW_GATEWAY_SOCKET").map(PathBuf::from),
        };
        let (config, warning) = parse_config(values);
        if let Some(warning) = warning {
            eprintln!("{warning}");
        }
        config
    }
}

/// 内核日志级别:向导(wizard/install 与 wizard/config 均收集 wizard_log_level)
/// 经生命周期回调落盘到 {var_dir}/qwenpaw-log-level,监督者启动时读取。
/// 白名单与上游 app_cmd.py `--log-level` 的 click.Choice 完全一致 —— 该值会
/// 进入内核 argv,必须收敛取值;非法/缺失一律回退 info(宁缺勿滥,不猜)。
pub fn load_kernel_log_level(var_dir: &str) -> String {
    let contents = std::fs::read_to_string(format!("{var_dir}/qwenpaw-log-level")).ok();
    parse_kernel_log_level(contents.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_socket_prefers_explicit_then_app_dest_and_stays_disabled() {
        assert_eq!(
            resolve_gateway_socket(
                Some("/run/explicit.sock".into()),
                Some("/srv/qwenpaw/target")
            ),
            Some(PathBuf::from("/run/explicit.sock"))
        );
        assert_eq!(
            resolve_gateway_socket(None, Some("/srv/qwenpaw/target")),
            Some(PathBuf::from("/srv/qwenpaw/target/app.sock"))
        );
        assert_eq!(resolve_gateway_socket(None, None), None);
        assert_eq!(resolve_gateway_socket(None, Some("")), None);
    }

    #[test]
    fn config_parser_is_pure_and_pins_manager_to_installed_target_binary() {
        let (config, warning) = parse_config(ConfigValues {
            var_dir: Some("/tmp/qwenpaw/var".into()),
            etc_dir: Some("/tmp/qwenpaw/etc".into()),
            home_dir: Some("/tmp/qwenpaw/home".into()),
            data_share_paths: Some(":/tmp/qwenpaw/share/:/unused".into()),
            service_port: Some("19091".into()),
            service_port_file: Some("18088\n".into()),
            web_proxy_port: None,
            kernel_log_level_file: Some(" DEBUG \n".into()),
            supervisor_port: Some("19090".into()),
            auto_start: Some("0".into()),
            app_dest: Some("/tmp/qwenpaw/target".into()),
            gateway_socket: Some("/run/qwenpaw.sock".into()),
        });

        assert_eq!(
            config.supervisor_binary,
            PathBuf::from("/tmp/qwenpaw/target/bin/qwenpaw-supervisor")
        );
        assert_eq!(
            config.stable_helper_binary(),
            PathBuf::from("/tmp/qwenpaw/cmd/qwenpaw-supervisor")
        );
        assert_eq!(config.kernel_port, 18088);
        assert_eq!(config.kernel_log_level, "debug");
        assert_eq!(config.share_dir, "/tmp/qwenpaw/share/.qwenpaw");
        assert_eq!(
            config.gateway_socket,
            Some(PathBuf::from("/run/qwenpaw.sock"))
        );
        assert!(!config.auto_start);
        assert!(warning.is_none());

        let (conflicting, warning) = parse_config(ConfigValues {
            service_port: Some("18088".into()),
            supervisor_port: Some("18088".into()),
            ..ConfigValues::default()
        });
        assert_eq!(conflicting.kernel_port, 18089);
        assert!(warning.unwrap().contains("回退到 18089"));
        assert_eq!(
            conflicting.supervisor_binary,
            PathBuf::from("/var/apps/qwenpaw/target/bin/qwenpaw-supervisor")
        );
    }

    #[test]
    fn web_proxy_port_defaults_to_auto_and_off_disables_it() {
        let parse_web_proxy_port = |value: Option<&str>| {
            parse_config(ConfigValues {
                web_proxy_port: value.map(str::to_owned),
                ..ConfigValues::default()
            })
            .0
            .web_proxy_port
        };

        assert_eq!(parse_web_proxy_port(None), Some(0));
        assert_eq!(parse_web_proxy_port(Some("0")), Some(0));
        assert_eq!(parse_web_proxy_port(Some("off")), None);
        assert_eq!(parse_web_proxy_port(Some("18089")), Some(18089));
        assert_eq!(parse_web_proxy_port(Some("invalid")), Some(0));
    }

    #[test]
    fn production_main_entry_uses_persisted_web_proxy_port_when_not_overridden() {
        if std::env::var_os("QWENPAW_TEST_CONFIG_CHILD").is_some() {
            assert_eq!(Config::load().web_proxy_port, Some(23456));
            return;
        }

        use std::os::unix::fs::PermissionsExt;
        let dir = crate::api::test_support::TestDir::new("config-main-persisted-web-port");
        let var = dir.0.join("var");
        let target = dir.0.join("target");
        let bin = target.join("bin");
        std::fs::create_dir_all(&var).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(var.join("web_proxy_port"), "23456\n").unwrap();
        let fake_supervisor = bin.join("qwenpaw-supervisor");
        std::fs::write(
            &fake_supervisor,
            "#!/bin/sh\nexec \"$QWENPAW_TEST_CONFIG_BINARY\" --exact config::tests::production_main_entry_uses_persisted_web_proxy_port_when_not_overridden --nocapture\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_supervisor, std::fs::Permissions::from_mode(0o755)).unwrap();
        let binary = std::env::current_exe().unwrap();
        let direct = std::process::Command::new(&binary)
            .args([
                "--exact",
                "config::tests::production_main_entry_uses_persisted_web_proxy_port_when_not_overridden",
                "--nocapture",
            ])
            .env("TRIM_APPDEST", &target)
            .env("TRIM_PKGVAR", &var)
            .env("QWENPAW_TEST_CONFIG_CHILD", "1")
            .env_remove("QWENPAW_WEB_PROXY_PORT")
            .output()
            .unwrap();
        assert!(
            direct.status.success(),
            "direct configuration load did not use the saved port: {}",
            String::from_utf8_lossy(&direct.stderr),
        );

        let main = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("cmd/main");
        let output = std::process::Command::new("bash")
            .arg(main)
            .arg("start")
            .env("TRIM_APPDEST", target)
            .env("TRIM_PKGVAR", var)
            .env("QWENPAW_TEST_CONFIG_CHILD", "1")
            .env("QWENPAW_TEST_CONFIG_BINARY", binary)
            .env_remove("QWENPAW_WEB_PROXY_PORT")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "main entry ignored the saved port:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn log_level_whitelist_and_fallback() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-loglevel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let set = |content: &str| std::fs::write(dir.join("qwenpaw-log-level"), content).unwrap();

        set("debug\n");
        assert_eq!(load_kernel_log_level(dir.to_str().unwrap()), "debug");
        set("  WARNING \n");
        assert_eq!(load_kernel_log_level(dir.to_str().unwrap()), "warning");
        set("verbose");
        assert_eq!(load_kernel_log_level(dir.to_str().unwrap()), "info"); // 非白名单回退
        set("");
        assert_eq!(load_kernel_log_level(dir.to_str().unwrap()), "info"); // 空内容回退
        std::fs::remove_dir_all(&dir).ok();

        // 文件不存在 → info
        assert_eq!(load_kernel_log_level("/nonexistent-var-dir"), "info");
    }
}
