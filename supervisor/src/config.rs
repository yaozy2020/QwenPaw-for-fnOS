//! 路径解析:TRIM_* 注入优先(官方规范),否则回退 /var/apps 约定(官方 cgi 示例即硬编码)。
//! 所有自产文件统一挂 var_dir 之下。

use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub var_dir: String,
    pub etc_dir: String,
    pub home_dir: String,
    pub share_dir: String,
    pub kernel_venv: String,
    pub kernel_pid_file: String,
    pub kernel_log: String,
    pub kernel_port: u16,
    pub token_file: String,
    pub supervisor_pid_file: String,
    pub supervisor_log: String,
    pub listen_port: u16,
    pub auto_start: bool,
}

fn env_or(key: &str, fallback: &str) -> String {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => fallback.to_string(),
    }
}

fn env_port(key: &str, fallback: u16) -> u16 {
    env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|&v| (1024..=65535).contains(&v))
        .unwrap_or(fallback)
}

impl Config {
    pub fn load() -> Self {
        let var_dir = env_or("TRIM_PKGVAR", "/var/apps/qwenpaw/var");
        let etc_dir = env_or("TRIM_PKGETC", "/var/apps/qwenpaw/etc");
        let home_dir = env_or("TRIM_PKGHOME", "/var/apps/qwenpaw/home");
        let share_raw = env::var("TRIM_DATA_SHARE_PATHS").unwrap_or_default();
        let first = share_raw.split(':').find(|s| !s.trim().is_empty());
        let share_dir = match first {
            Some(p) => format!("{}/.qwenpaw", p.trim_end_matches('/')),
            None => "/var/apps/qwenpaw/shares/qwenpaw/.qwenpaw".to_string(),
        };

        // 内核端口:service_port 文件优先,19091 视为老冲突端口自愈为 18088
        let mut kernel_port = env_port("TRIM_SERVICE_PORT", 18088);
        let port_file = format!("{var_dir}/service_port");
        if let Ok(p) = std::fs::read_to_string(&port_file) {
            if let Ok(v) = p.trim().parse::<u16>() {
                if (1024..=65535).contains(&v) && v != 19091 {
                    kernel_port = v;
                }
            }
        }

        let listen = env_port("SUPERVISOR_PORT", 19090);
        let auto_start = env::var("SUPERVISOR_AUTOSTART").unwrap_or_default() != "0";

        Config {
            var_dir: var_dir.clone(),
            etc_dir,
            home_dir: home_dir.clone(),
            share_dir: share_dir.clone(),
            kernel_venv: format!("{home_dir}/venv"),
            kernel_pid_file: format!("{var_dir}/kernel.pid"),
            kernel_log: format!("{var_dir}/qwenpaw.log"),
            kernel_port,
            token_file: format!("{var_dir}/supervisor-token"),
            supervisor_pid_file: format!("{var_dir}/supervisor.pid"),
            supervisor_log: format!("{var_dir}/supervisor.log"),
            listen_port: listen,
            auto_start,
        }
    }
}
