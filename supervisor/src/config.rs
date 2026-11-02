//! 路径解析:TRIM_* 注入优先(官方规范),否则回退 /var/apps 约定(官方 cgi 示例即硬编码)。
//! 所有自产文件统一挂 var_dir 之下。

use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub var_dir: String,
    pub etc_dir: String,
    pub home_dir: String,
    pub share_dir: String,
    pub ui_config_file: String,
    pub kernel_venv: String,
    pub kernel_pid_file: String,
    pub kernel_log: String,
    pub kernel_port: u16,
    pub kernel_log_level: String,
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

        // 内核日志级别:向导落盘文件 → 白名单校验(见 load_kernel_log_level)
        let kernel_log_level = load_kernel_log_level(&var_dir);

        let listen = env_port("SUPERVISOR_PORT", 19090);
        let auto_start = env::var("SUPERVISOR_AUTOSTART").unwrap_or_default() != "0";
        let app_dest = env_or("TRIM_APPDEST", "/var/apps/qwenpaw/target");

        Config {
            var_dir: var_dir.clone(),
            etc_dir,
            home_dir: home_dir.clone(),
            share_dir: share_dir.clone(),
            ui_config_file: format!("{app_dest}/ui/config"),
            kernel_venv: format!("{home_dir}/venv"),
            kernel_pid_file: format!("{var_dir}/kernel.pid"),
            kernel_log: format!("{var_dir}/qwenpaw.log"),
            kernel_port,
            kernel_log_level,
            token_file: format!("{var_dir}/supervisor-token"),
            supervisor_pid_file: format!("{var_dir}/supervisor.pid"),
            supervisor_log: format!("{var_dir}/supervisor.log"),
            listen_port: listen,
            auto_start,
        }
    }
}

/// 内核日志级别:向导(wizard/install 与 wizard/config 均收集 wizard_log_level)
/// 经生命周期回调落盘到 {var_dir}/qwenpaw-log-level,监督者启动时读取。
/// 白名单与上游 app_cmd.py `--log-level` 的 click.Choice 完全一致 —— 该值会
/// 进入内核 argv,必须收敛取值;非法/缺失一律回退 info(宁缺勿滥,不猜)。
pub fn load_kernel_log_level(var_dir: &str) -> String {
    const ALLOWED: [&str; 6] = ["critical", "error", "warning", "info", "debug", "trace"];
    std::fs::read_to_string(format!("{var_dir}/qwenpaw-log-level"))
        .ok()
        .map(|v| v.trim().to_lowercase())
        .filter(|v| ALLOWED.contains(&v.as_str()))
        .unwrap_or_else(|| "info".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
