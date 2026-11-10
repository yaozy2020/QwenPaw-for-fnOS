use std::{
    env,
    ffi::CString,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use crate::{
    cli::{PlatformAction, ServiceAction},
    config::Config,
    control, helper,
    runtime_settings::SettingsPatch,
    service,
};

pub fn run(action: PlatformAction, cfg: &Config) -> i32 {
    match action {
        PlatformAction::Configure => configure_from_environment(cfg),
        PlatformAction::ConfigInit => {
            let (output, diagnostics) = config_init(&cfg.var_dir);
            for diagnostic in diagnostics {
                eprintln!("{diagnostic}");
            }
            print!("{output}");
            0
        }
        PlatformAction::UpgradeInit => {
            let output = upgrade_init(&cfg.var_dir);
            match helper::stage_current_executable(&cfg.var_dir) {
                Ok(()) => {
                    print!("{output}");
                    0
                }
                Err(error) => {
                    eprintln!("升级初始化无法暂存 supervisor helper: {error}");
                    1
                }
            }
        }
        PlatformAction::Install => finish_operation(cfg, "安装", install_callback),
        PlatformAction::Upgrade => {
            let status = finish_operation(cfg, "升级", upgrade_callback);
            if status != 0 {
                if let Err(error) = helper::restore_installed(&cfg.var_dir, &cfg.supervisor_binary)
                {
                    log_message(
                        cfg,
                        &format!("ERROR: 升级失败后恢复 helper 失败: {error}"),
                        true,
                    );
                }
            }
            status
        }
        PlatformAction::Uninstall => crate::uninstall::run(cfg),
        PlatformAction::MirrorGet => {
            let (status, response) = crate::mirror::get_json(&cfg.var_dir);
            println!("{response}");
            status
        }
        PlatformAction::MirrorSet(value) => {
            let (status, response) = mirror_set(value.as_deref(), cfg);
            println!("{response}");
            status
        }
    }
}

fn finish_operation(
    cfg: &Config,
    label: &str,
    operation: fn(&Config) -> Result<(), String>,
) -> i32 {
    match operation(cfg) {
        Ok(()) => {
            log_message(cfg, &format!("QwenPaw {label}完成"), false);
            0
        }
        Err(error) => {
            log_message(cfg, &format!("ERROR: QwenPaw {label}失败: {error}"), true);
            1
        }
    }
}

#[derive(Clone, Copy)]
struct PlatformOwner {
    uid: libc::uid_t,
    gid: libc::gid_t,
}

#[derive(Debug, PartialEq, Eq)]
enum PinState {
    Missing,
    Exact(String),
    Invalid(String),
}

#[derive(Debug, PartialEq, Eq)]
enum RuntimePlan {
    FirstInstall,
    Keep,
    Repair(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Install,
    Upgrade,
}

fn install_callback(cfg: &Config) -> Result<(), String> {
    let port = env::var("wizard_port").unwrap_or_else(|_| "18088".into());
    let log_level = env::var("wizard_log_level").unwrap_or_default();
    configure_patch(&port, &log_level, cfg.listen_port)?;
    prepare_var_directory(cfg)?;
    helper::stage_current_executable(&cfg.var_dir)?;
    let mirror_seed = env::var("wizard_pypi_mirror").unwrap_or_else(|_| "aliyun".into());
    mirror_seed_if_needed(cfg, &mirror_seed)?;

    if service::run(ServiceAction::Status, cfg) != 0 {
        apply_platform_permissions(cfg, true)?;
    }
    if configure(cfg, &port, &log_level) != 0 {
        return Err("平台端口或日志设置未能通过监督者事务应用".into());
    }
    run_runtime_install_flow(cfg, Lifecycle::Install)
}

fn upgrade_callback(cfg: &Config) -> Result<(), String> {
    log_message(cfg, "开始升级回调", false);
    prepare_var_directory(cfg)?;
    log_message(cfg, "准备var完成", false);

    if service::run(ServiceAction::Stop, cfg) != 0 {
        log_message(cfg, "ERROR: stop失败", true);
        return Err("升级前停止监督者失败".into());
    }
    log_message(cfg, "stop成功", false);

    apply_platform_permissions(cfg, true)?;
    log_message(cfg, "权限应用完成", false);

    match env::current_exe() {
        Ok(path) => log_message(
            cfg,
            &format!(
                "准备启动: current_exe={} exists={}",
                path.display(),
                path.exists()
            ),
            false,
        ),
        Err(_) => log_message(
            cfg,
            "准备启动: current_exe=<unavailable> exists=false",
            false,
        ),
    }
    if service::run(ServiceAction::Start, cfg) != 0 {
        return Err("升级后启动监督者失败".into());
    }
    log_message(cfg, "start成功", false);

    log_message(cfg, "runtime flow开始", false);
    let result = run_runtime_install_flow(cfg, Lifecycle::Upgrade);
    if result.is_ok() {
        log_message(cfg, "runtime flow完成", false);
    }
    result
}

fn run_runtime_install_flow(cfg: &Config, lifecycle: Lifecycle) -> Result<(), String> {
    let socket = control::socket_path(&cfg.var_dir);
    let health = control::runtime_health(&socket)?;
    let active_venv = Path::new(&cfg.kernel_venv).exists();
    let installed = match runtime_is_installed(&health) {
        Ok(installed) => installed,
        Err(_) if active_venv && health["runtime"]["installed"].as_bool() == Some(true) => false,
        Err(error) => return Err(error),
    };
    let plan = if installed {
        RuntimePlan::Keep
    } else {
        let plan = runtime_plan(false, active_venv, read_pin_state(&cfg.var_dir)?)?;
        if let RuntimePlan::Repair(version) = &plan {
            if running_import_failure_matches_pin(&health, version) {
                return Ok(());
            }
        }
        plan
    };
    match plan {
        RuntimePlan::Keep => {
            control::ensure_initialized(&socket)?;
            Ok(())
        }
        RuntimePlan::FirstInstall => {
            let message = match lifecycle {
                Lifecycle::Install => {
                    "首次安装未提供合法版本钉；仅安装外壳，不创建 venv 或运行 pip"
                }
                Lifecycle::Upgrade => {
                    "外壳升级未检测到内核且缺少版本钉；仅升级外壳，不创建 venv 或运行 pip"
                }
            };
            log_message(cfg, message, false);
            Ok(())
        }
        RuntimePlan::Repair(version) => {
            let mirror = mirror_for_install(cfg)?;
            log_message(
                cfg,
                &format!("运行环境缺损，按精确版本钉 {version} 恢复"),
                false,
            );
            control::install_and_wait(&socket, &version, &mirror)
        }
    }
}

fn runtime_is_installed(response: &serde_json::Value) -> Result<bool, String> {
    let runtime = response
        .get("runtime")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "监督者 runtime-health 响应缺少 runtime 对象".to_string())?;
    let installed = runtime
        .get("installed")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "监督者 runtime-health 响应缺少 installed 状态".to_string())?;
    let version = runtime
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "监督者 runtime-health 响应缺少 version".to_string())?;
    if installed && version.trim().is_empty() {
        if runtime
            .get("metadata_repairable")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            return Ok(false);
        }
        return Err("监督者报告内核已安装，但版本为空或无法可靠探测".into());
    }
    Ok(installed)
}

fn running_import_failure_matches_pin(response: &serde_json::Value, pin: &str) -> bool {
    use std::io::Read;
    use std::time::Duration;

    let runtime = &response["runtime"];
    if runtime
        .get("installed")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
        || !runtime
            .get("version")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|version| version.trim().is_empty())
        || runtime
            .get("metadata_repairable")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || runtime.get("running").and_then(serde_json::Value::as_bool) != Some(true)
        || runtime
            .get("reachable")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return false;
    }
    let Some(port) = runtime
        .get("port")
        .and_then(serde_json::Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
    else {
        return false;
    };
    let Ok(client) = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(1))
        .build()
    else {
        return false;
    };
    let Ok(response) = client
        .get(format!("http://127.0.0.1:{port}/api/version"))
        .send()
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let mut body = Vec::new();
    if response.take(64 * 1024 + 1).read_to_end(&mut body).is_err() || body.len() > 64 * 1024 {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|payload| {
            payload
                .get("version")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .as_deref()
        == Some(pin)
}

fn runtime_plan(installed: bool, active_venv: bool, pin: PinState) -> Result<RuntimePlan, String> {
    if installed {
        return Ok(RuntimePlan::Keep);
    }
    match pin {
        PinState::Exact(version) => Ok(RuntimePlan::Repair(version)),
        PinState::Invalid(reason) => Err(format!("版本钉无效，拒绝猜测内核版本: {reason}")),
        PinState::Missing if active_venv => {
            Err("运行环境缺损且没有合法精确版本钉，拒绝重装或升级内核".into())
        }
        PinState::Missing => Ok(RuntimePlan::FirstInstall),
    }
}

fn read_pin_state(var_dir: &str) -> Result<PinState, String> {
    let path = Path::new(var_dir).join("qwenpaw-version");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Ok(PinState::Invalid("版本钉不是普通文件".into())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(PinState::Missing),
        Err(error) => return Err(format!("检查版本钉失败: {error}")),
    }
    let raw = fs::read_to_string(&path).map_err(|error| format!("读取版本钉失败: {error}"))?;
    let compact = raw
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    let version = compact
        .strip_prefix("Version:")
        .or_else(|| compact.strip_prefix("version:"))
        .unwrap_or(&compact);
    match crate::install::InstallChannel::parse(version) {
        Some(crate::install::InstallChannel::Exact(version)) => Ok(PinState::Exact(version)),
        _ => Ok(PinState::Invalid(compact)),
    }
}

fn mirror_seed_if_needed(cfg: &Config, raw_seed: &str) -> Result<(), String> {
    let path = Path::new(&cfg.var_dir).join("pip-mirror");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err("pip-mirror 不是普通文件，拒绝覆盖".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let seed = if raw_seed == "official" {
                "https://pypi.org/simple"
            } else {
                raw_seed
            };
            let resolved = crate::install::resolve_mirror(seed)?;
            crate::mirror::write(&cfg.var_dir, &resolved)
                .map_err(|error| format!("初始化 pip-mirror 失败: {error:?}"))
        }
        Err(error) => Err(format!("检查 pip-mirror 失败: {error}")),
    }
}

fn mirror_for_install(cfg: &Config) -> Result<String, String> {
    prepare_var_directory(cfg)?;
    let raw_seed = env::var("wizard_pypi_mirror").unwrap_or_else(|_| "aliyun".into());
    mirror_seed_if_needed(cfg, &raw_seed)?;
    let mirror = crate::mirror::read(&cfg.var_dir)
        .map_err(|error| format!("读取 pip-mirror 失败: {error}"))?;
    if mirror.is_empty() {
        Ok(String::new())
    } else {
        crate::install::resolve_mirror(&mirror)
    }
}

fn prepare_var_directory(cfg: &Config) -> Result<(), String> {
    let path = Path::new(&cfg.var_dir);
    fs::create_dir_all(path).map_err(|error| format!("创建应用 var 目录失败: {error}"))?;
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("检查应用 var 目录失败: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("应用 var 路径不是实际目录".into());
    }
    Ok(())
}

fn platform_owner() -> Result<Option<PlatformOwner>, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Ok(None);
    }
    let username = env::var("TRIM_USERNAME")
        .ok()
        .filter(|value| !value.is_empty());
    let groupname = env::var("TRIM_GROUPNAME")
        .ok()
        .filter(|value| !value.is_empty());
    let (Some(username), Some(groupname)) = (username, groupname) else {
        return Ok(None);
    };
    let username = CString::new(username).map_err(|_| "TRIM_USERNAME 含 NUL".to_string())?;
    let groupname = CString::new(groupname).map_err(|_| "TRIM_GROUPNAME 含 NUL".to_string())?;
    let user = unsafe { libc::getpwnam(username.as_ptr()) };
    if user.is_null() {
        return Err("TRIM_USERNAME 不存在".into());
    }
    let group = unsafe { libc::getgrnam(groupname.as_ptr()) };
    if group.is_null() {
        return Err("TRIM_GROUPNAME 不存在".into());
    }
    Ok(Some(PlatformOwner {
        uid: unsafe { (*user).pw_uid },
        gid: unsafe { (*group).gr_gid },
    }))
}

fn apply_platform_permissions(cfg: &Config, include_home: bool) -> Result<(), String> {
    let owner = platform_owner()?;
    let roots = [
        (PathBuf::from(&cfg.var_dir), 0o750, 0o640, true),
        (PathBuf::from(&cfg.etc_dir), 0o755, 0o640, false),
        (PathBuf::from(&cfg.home_dir), 0o750, 0o640, false),
        // 数据分享根(TRIM_DATA_SHARE_PATHS 下的 .qwenpaw):初始化以 root
        // 运行内核 init 时会在其中创建 root 属主的 config.json 等文件,而
        // 内核以 package 用户运行 → 打开即 EACCES(真机报错)。属主必须
        // 收敛到应用用户;文件 0600/目录 0o750 与 var/home 的隐私口径一致。
        (PathBuf::from(&cfg.share_dir), 0o750, 0o600, true),
    ];
    for (path, directory_mode, file_mode, private_files) in roots {
        if path == Path::new(&cfg.home_dir) && !include_home {
            continue;
        }
        if path == Path::new(&cfg.var_dir) && path.exists() {
            let active_socket = control::socket_path(&cfg.var_dir);
            if active_socket.exists() && service::run(ServiceAction::Status, cfg) == 0 {
                continue;
            }
        }
        secure_tree(&path, directory_mode, file_mode, private_files, owner)
            .map_err(|error| format!("设置应用权限失败 {}: {error}", path.display()))?;
    }
    Ok(())
}

fn secure_tree(
    path: &Path,
    directory_mode: u32,
    file_mode: u32,
    private_files: bool,
    owner: Option<PlatformOwner>,
) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => secure_tree_node(path, directory_mode, file_mode, private_files, owner),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn secure_tree_node(
    path: &Path,
    directory_mode: u32,
    file_mode: u32,
    private_files: bool,
    owner: Option<PlatformOwner>,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Ok(());
    }
    if file_type.is_dir() {
        let entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        for entry in entries {
            secure_tree_node(
                &entry.path(),
                directory_mode,
                file_mode,
                private_files,
                owner,
            )?;
        }
        set_owner(path, owner)?;
        fs::set_permissions(path, fs::Permissions::from_mode(directory_mode))?;
    } else if file_type.is_file() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if private_files && is_private_var_file(&name) {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        } else {
            let mode = if directory_mode == 0o750 && metadata.permissions().mode() & 0o111 != 0 {
                0o750
            } else {
                file_mode
            };
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        set_owner(path, owner)?;
    }
    Ok(())
}

fn is_private_var_file(name: &str) -> bool {
    name == "supervisor-token"
        || name == "qwenpaw-version"
        || name == "supervisor-control.sock"
        || name.ends_with(".lock")
        || name.ends_with(".sock")
        || name.ends_with(".tmp")
        || name.starts_with(".venv.")
        || name.starts_with(".qwenpaw-version.")
        || name == "settings-transaction.json"
}

fn set_owner(path: &Path, owner: Option<PlatformOwner>) -> io::Result<()> {
    let Some(owner) = owner else {
        return Ok(());
    };
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    if unsafe { libc::lchown(path.as_ptr(), owner.uid, owner.gid) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn mirror_set(value: Option<&str>, cfg: &Config) -> (i32, String) {
    let Some(value) = value else {
        return (
            1,
            serde_json::json!({
                "success": false,
                "message": "下载源参数缺失"
            })
            .to_string(),
        );
    };
    if let Err(message) = crate::install::validate_mirror(value) {
        return (
            1,
            serde_json::json!({"success": false, "message": message}).to_string(),
        );
    }

    let socket = control::socket_path(&cfg.var_dir);
    match control::submit_mirror(&socket, value) {
        Ok(()) => (
            0,
            serde_json::json!({"success": true, "message": "下载源已保存"}).to_string(),
        ),
        Err(error) => (
            1,
            serde_json::json!({
                "success": false,
                "message": "下载源写入失败(权限?)",
                "error": error
            })
            .to_string(),
        ),
    }
}

fn configure_from_environment(cfg: &Config) -> i32 {
    let port = std::env::var("wizard_port").unwrap_or_default();
    let log_level = std::env::var("wizard_log_level").unwrap_or_default();
    configure(cfg, &port, &log_level)
}

fn configure(cfg: &Config, port: &str, log_level: &str) -> i32 {
    let patch = match configure_patch(port, log_level, cfg.listen_port) {
        Ok(Some(patch)) => patch,
        Ok(None) => {
            log_message(cfg, "未提供 wizard_port/wizard_log_level，无需修改", false);
            return 0;
        }
        Err(error) => {
            log_message(cfg, &error, true);
            return 1;
        }
    };

    log_message(cfg, "确保监督者在线...", false);
    if service::run(crate::cli::ServiceAction::Start, cfg) != 0 {
        log_message(cfg, "ERROR: 监督者启动失败，配置未应用", true);
        return 1;
    }

    let socket = control::socket_path(&cfg.var_dir);
    match control::submit_settings(&socket, &patch) {
        Ok(()) => {
            log_message(cfg, "设置已由监督者通过私有控制 Socket 应用", false);
            0
        }
        Err(error) => {
            log_message(
                cfg,
                &format!("ERROR: 私有控制 Socket 提交设置失败: {error}"),
                true,
            );
            1
        }
    }
}

fn configure_patch(
    port: &str,
    log_level: &str,
    supervisor_port: u16,
) -> Result<Option<SettingsPatch>, String> {
    let parsed_port = if port.is_empty() {
        None
    } else {
        if port.len() > 5
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port.parse::<u16>().map_or(true, |value| {
                !(1024..=65535).contains(&value) || value == 19090 || value == 19091
            })
        {
            return Err(
                "ERROR: 端口无效（有效范围 1024-65535，不可使用监督者保留端口 19090/19091）".into(),
            );
        }
        Some(port.parse::<u16>().expect("validated ASCII port"))
    };

    if !log_level.is_empty()
        && !matches!(
            log_level,
            "critical" | "error" | "warning" | "info" | "debug" | "trace"
        )
    {
        return Err("ERROR: 日志级别无效（可选 critical/error/warning/info/debug/trace）".into());
    }

    let patch = SettingsPatch {
        port: parsed_port,
        log_level: (!log_level.is_empty()).then(|| log_level.to_string()),
    };
    if patch.port.is_none() && patch.log_level.is_none() {
        return Ok(None);
    }
    patch
        .validate(supervisor_port)
        .map_err(|error| format!("ERROR: {error}"))?;
    Ok(Some(patch))
}

fn first_line_without_ascii_whitespace(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    Some(
        fs::read_to_string(path)
            .unwrap_or_default()
            .split('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect(),
    )
}

fn config_init(var_dir: &str) -> (String, Vec<String>) {
    let var = Path::new(var_dir);
    let port = first_line_without_ascii_whitespace(&var.join("service_port"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "18088".into());
    let mut diagnostics = Vec::new();
    let level = match first_line_without_ascii_whitespace(&var.join("qwenpaw-log-level"))
        .unwrap_or_default()
        .as_str()
    {
        "error" => "error",
        "warning" => "warning",
        "info" => "info",
        "debug" => "debug",
        "trace" => {
            diagnostics.push(
                "[config_init] 落盘日志级别为 trace(向导无此档),面板回显为 debug;未改动落盘值"
                    .into(),
            );
            "debug"
        }
        "critical" => {
            diagnostics.push(
                "[config_init] 落盘日志级别为 critical(向导无此档),面板回显为 error;未改动落盘值"
                    .into(),
            );
            "error"
        }
        "" => "info",
        invalid => {
            diagnostics.push(format!(
                "[config_init] 落盘日志级别 '{invalid}' 不在白名单,回显默认值 info"
            ));
            "info"
        }
    };
    (
        format!("wizard_port={port}\nservice_port={port}\nwizard_log_level={level}\n"),
        diagnostics,
    )
}

fn upgrade_init(var_dir: &str) -> String {
    let path = Path::new(var_dir).join("pip-mirror");
    let mirror = match first_line_without_ascii_whitespace(&path) {
        None => "aliyun".to_string(),
        Some(value) if value.contains("tsinghua") => "tsinghua".into(),
        Some(value) if value.contains("aliyun") => "aliyun".into(),
        Some(value) if value.contains("ustc") => "ustc".into(),
        Some(value) if value.contains("pypi.org") || value.is_empty() => "official".into(),
        Some(value) => value,
    };
    format!("wizard_pypi_mirror={mirror}\n")
}

fn log_message(cfg: &Config, message: &str, is_error: bool) {
    let line = format!("{} - {message}", local_timestamp());
    println!("{line}");

    let log_path = Path::new(&cfg.var_dir).join("qwenpaw-config.log");
    if let Some(parent) = log_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("cannot create platform configuration log directory: {error}");
            return write_temp_error(message, is_error);
        }
    }
    if let Err(error) = append_line(&log_path, &line) {
        eprintln!("cannot append platform configuration log: {error}");
    }
    write_temp_error(message, is_error);
}

fn local_timestamp() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut local) }.is_null() {
        return now.to_string();
    }
    let mut output = [0u8; 20];
    let length = unsafe {
        libc::strftime(
            output.as_mut_ptr().cast(),
            output.len(),
            c"%Y-%m-%d %H:%M:%S".as_ptr(),
            &local,
        )
    };
    if length == 0 {
        now.to_string()
    } else {
        String::from_utf8_lossy(&output[..length]).into_owned()
    }
}

fn append_line(path: &Path, line: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{line}")
}

fn write_temp_error(message: &str, is_error: bool) {
    if !is_error {
        return;
    }
    let Some(path) = std::env::var_os("TRIM_TEMP_LOGFILE").filter(|path| !path.is_empty()) else {
        return;
    };
    if let Err(error) = append_line(Path::new(&path), message) {
        eprintln!("cannot append platform error to TRIM_TEMP_LOGFILE: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_var(label: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "qwenpaw-platform-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn local_timestamps_match_the_platform_log_format() {
        let timestamp = local_timestamp();
        assert_eq!(timestamp.len(), 19);
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[7..8], "-");
        assert_eq!(&timestamp[10..11], " ");
        assert_eq!(&timestamp[13..14], ":");
        assert_eq!(&timestamp[16..17], ":");
    }

    #[test]
    fn validates_legacy_configure_input_without_normalizing_values() {
        assert!(configure_patch("", "", 19090).unwrap().is_none());
        let patch = configure_patch("01024", "trace", 19090).unwrap().unwrap();
        assert_eq!(patch.port, Some(1024));
        assert_eq!(patch.log_level.as_deref(), Some("trace"));
        assert_eq!(
            configure_patch("65535", "critical", 19090)
                .unwrap()
                .unwrap()
                .port,
            Some(65535)
        );

        for invalid in [
            "1", "1023", "19090", "19091", "65536", "10000x", "+18088", " 18088", "180880",
        ] {
            assert!(
                configure_patch(invalid, "", 19090).is_err(),
                "accepted port {invalid:?}"
            );
        }
        for invalid in ["INFO", "verbose", " info", "debug "] {
            assert!(
                configure_patch("", invalid, 19090).is_err(),
                "accepted level {invalid:?}"
            );
        }
        assert!(configure_patch("19092", "", 19092).is_err());
    }

    #[test]
    fn config_init_preserves_legacy_echo_mapping_and_does_not_rewrite_files() {
        let var = temp_var("config-init");
        fs::create_dir_all(&var).unwrap();
        fs::write(var.join("service_port"), " 18 088 \nignored\n").unwrap();
        fs::write(var.join("qwenpaw-log-level"), "trace\n").unwrap();
        let before = fs::read(var.join("qwenpaw-log-level")).unwrap();
        let (output, diagnostics) = config_init(var.to_str().unwrap());
        assert_eq!(
            output,
            "wizard_port=18088\nservice_port=18088\nwizard_log_level=debug\n"
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("trace"));
        assert_eq!(fs::read(var.join("qwenpaw-log-level")).unwrap(), before);

        fs::write(var.join("qwenpaw-log-level"), "critical\n").unwrap();
        let (output, diagnostics) = config_init(var.to_str().unwrap());
        assert!(output.ends_with("wizard_log_level=error\n"));
        assert!(diagnostics[0].contains("critical"));
        fs::remove_dir_all(var).unwrap();
    }

    #[test]
    fn config_init_uses_legacy_defaults_for_missing_empty_and_unknown_values() {
        let var = temp_var("config-defaults");
        fs::create_dir_all(&var).unwrap();
        let (output, diagnostics) = config_init(var.to_str().unwrap());
        assert_eq!(
            output,
            "wizard_port=18088\nservice_port=18088\nwizard_log_level=info\n"
        );
        assert!(diagnostics.is_empty());

        fs::write(var.join("service_port"), "\n19090\n").unwrap();
        fs::write(var.join("qwenpaw-log-level"), "verbose\n").unwrap();
        let (output, diagnostics) = config_init(var.to_str().unwrap());
        assert_eq!(
            output,
            "wizard_port=18088\nservice_port=18088\nwizard_log_level=info\n"
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("verbose"));
        fs::remove_dir_all(var).unwrap();
    }

    #[test]
    fn upgrade_init_preserves_legacy_mirror_mapping_and_custom_values() {
        let var = temp_var("upgrade-init");
        fs::create_dir_all(&var).unwrap();
        assert_eq!(
            upgrade_init(var.to_str().unwrap()),
            "wizard_pypi_mirror=aliyun\n"
        );
        let cases = [
            ("https://pypi.tuna.tsinghua.edu.cn/simple", "tsinghua"),
            ("https://mirrors.aliyun.com/pypi/simple/", "aliyun"),
            ("https://pypi.mirrors.ustc.edu.cn/simple/", "ustc"),
            ("https://pypi.org/simple", "official"),
            ("https://pypi.org/aliyun", "aliyun"),
            ("", "official"),
            (
                "https://mirror.example/simple",
                "https://mirror.example/simple",
            ),
        ];
        for (input, expected) in cases {
            fs::write(var.join("pip-mirror"), format!("{input}\nignored\n")).unwrap();
            assert_eq!(
                upgrade_init(var.to_str().unwrap()),
                format!("wizard_pypi_mirror={expected}\n")
            );
        }
        fs::remove_dir_all(var).unwrap();
    }

    #[test]
    fn mirror_set_distinguishes_missing_empty_and_invalid_inputs() {
        let var = temp_var("mirror-set-inputs");
        fs::create_dir_all(&var).unwrap();
        let cfg = Config {
            var_dir: var.to_string_lossy().into_owned(),
            etc_dir: var.join("etc").to_string_lossy().into_owned(),
            home_dir: var.join("home").to_string_lossy().into_owned(),
            share_dir: var.join("share").to_string_lossy().into_owned(),
            ui_config_file: var.join("ui-config").to_string_lossy().into_owned(),
            supervisor_binary: var.join("bin/qwenpaw-supervisor"),
            kernel_venv: var.join("venv").to_string_lossy().into_owned(),
            kernel_pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
            kernel_log: var.join("kernel.log").to_string_lossy().into_owned(),
            kernel_port: crate::api::test_support::free_port(),
            web_proxy_port: None,
            kernel_log_level: "info".into(),
            token_file: var.join("supervisor-token").to_string_lossy().into_owned(),
            supervisor_pid_file: var.join("supervisor.pid").to_string_lossy().into_owned(),
            supervisor_log: var.join("supervisor.log").to_string_lossy().into_owned(),
            listen_port: crate::api::test_support::free_port(),
            auto_start: false,
            gateway_socket: None,
        };

        let (missing_status, missing_json) = mirror_set(None, &cfg);
        let missing: serde_json::Value = serde_json::from_str(&missing_json).unwrap();
        let (invalid_status, invalid_json) =
            mirror_set(Some("https://mirror.example/simple;rm"), &cfg);
        let invalid: serde_json::Value = serde_json::from_str(&invalid_json).unwrap();
        let (empty_status, empty_json) = mirror_set(Some(""), &cfg);
        let empty: serde_json::Value = serde_json::from_str(&empty_json).unwrap();

        assert_eq!(missing_status, 1);
        assert_eq!(missing["message"], "下载源参数缺失");
        assert_eq!(invalid_status, 1);
        assert_eq!(
            invalid["message"],
            "下载源需为 http(s):// 开头且仅含 URL 安全字符的地址"
        );
        assert_eq!(empty_status, 1);
        assert_ne!(empty["message"], invalid["message"]);
        assert!(empty["error"]
            .as_str()
            .unwrap()
            .contains("private control socket"));
        assert!(!var.join("pip-mirror").exists());
        fs::remove_dir_all(var).unwrap();
    }

    #[test]
    fn healthy_runtime_ignores_missing_or_invalid_version_pins() {
        for pin in [
            PinState::Missing,
            PinState::Invalid("latest".into()),
            PinState::Invalid("malformed".into()),
        ] {
            assert_eq!(runtime_plan(true, true, pin), Ok(RuntimePlan::Keep));
        }
    }

    #[test]
    fn empty_runtime_without_pin_skips_kernel_install() {
        assert_eq!(
            runtime_plan(false, false, PinState::Missing),
            Ok(RuntimePlan::FirstInstall)
        );
    }

    #[test]
    fn broken_runtime_without_pin_fails_closed() {
        assert!(runtime_plan(false, true, PinState::Missing).is_err());
    }

    #[test]
    fn metadata_failure_with_existing_runtime_never_triggers_automatic_repair() {
        use crate::api::test_support::{free_port, install_python, TestDir};

        let dir = TestDir::new("platform-runtime-metadata-failure");
        let state = dir.state(free_port());
        let python = install_python(&dir, &state, "");
        fs::write(python, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(state.kernel.installed());
        let runtime = state.core.runtime_health();
        assert_eq!(runtime["version"], "");
        assert!(!runtime["error"].is_null());
        let response = serde_json::json!({"runtime": runtime});
        let decision = runtime_is_installed(&response)
            .and_then(|installed| runtime_plan(installed, true, PinState::Exact("2.3.0".into())));
        assert!(
            !matches!(decision, Ok(RuntimePlan::Repair(_))),
            "metadata failure must not trigger automatic repair: {decision:?}"
        );
    }

    fn live_version_probe(
        mut runtime: serde_json::Value,
        status: &str,
        body: &[u8],
        delay: std::time::Duration,
    ) -> (bool, Option<String>, std::time::Duration) {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{atomic::AtomicBool, Arc},
            time::{Duration, Instant},
        };

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        runtime["port"] = listener.local_addr().unwrap().port().into();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let body = body.to_vec();
        let status = status.to_string();
        let server = std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                        let count = stream.read(&mut buffer).unwrap();
                        if count == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..count]);
                    }
                    std::thread::sleep(delay);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                    break Some(String::from_utf8(request).unwrap());
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if server_stop.load(std::sync::atomic::Ordering::Acquire) {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("version fixture accept failed: {error}"),
            }
        });
        let started = Instant::now();
        let kept =
            running_import_failure_matches_pin(&serde_json::json!({"runtime": runtime}), "2.4.1");
        let elapsed = started.elapsed();
        stop.store(true, std::sync::atomic::Ordering::Release);
        (kept, server.join().unwrap(), elapsed)
    }

    fn repairable_running_runtime() -> serde_json::Value {
        serde_json::json!({
            "installed": true,
            "version": "",
            "metadata_repairable": true,
            "running": true,
            "reachable": true,
        })
    }

    #[test]
    fn live_import_probe_requires_success_and_exact_version_with_a_short_deadline() {
        use std::time::Duration;

        for (status, body, expected) in [
            ("200 OK", r#"{"version":"2.4.1"}"#, true),
            ("201 Created", r#"{"version":"2.4.1"}"#, true),
            ("500 Internal Server Error", r#"{"version":"2.4.1"}"#, false),
            ("302 Found", r#"{"version":"2.4.1"}"#, false),
            ("200 OK", r#"{"version":"2.4.0"}"#, false),
            ("200 OK", r#"{"version":"2.4.1 "}"#, false),
            ("200 OK", "not json", false),
        ] {
            let (kept, request, _) = live_version_probe(
                repairable_running_runtime(),
                status,
                body.as_bytes(),
                Duration::ZERO,
            );
            assert_eq!(kept, expected, "status={status}, body={body}");
            assert!(request
                .unwrap()
                .starts_with("GET /api/version HTTP/1.1\r\n"));
        }

        let oversized = format!("{{\"version\":\"2.4.1\"}}{}", " ".repeat(64 * 1024));
        let (kept, request, _) = live_version_probe(
            repairable_running_runtime(),
            "200 OK",
            oversized.as_bytes(),
            Duration::ZERO,
        );
        assert!(!kept);
        assert!(request.is_some());

        let (kept, request, elapsed) = live_version_probe(
            repairable_running_runtime(),
            "200 OK",
            br#"{"version":"2.4.1"}"#,
            Duration::from_millis(1300),
        );
        assert!(!kept);
        assert!(request.is_some());
        assert!(elapsed < Duration::from_secs(2), "probe took {elapsed:?}");
    }

    #[test]
    fn live_import_probe_does_not_generalize_to_other_runtime_states() {
        use std::time::Duration;

        for (field, value) in [
            ("installed", serde_json::json!(false)),
            ("version", serde_json::json!("2.4.1")),
            ("metadata_repairable", serde_json::json!(false)),
            ("running", serde_json::json!(false)),
            ("reachable", serde_json::json!(false)),
            ("running", serde_json::Value::Null),
        ] {
            let mut runtime = repairable_running_runtime();
            runtime[field] = value;
            let (kept, request, _) =
                live_version_probe(runtime, "200 OK", br#"{"version":"2.4.1"}"#, Duration::ZERO);
            assert!(!kept, "field={field}");
            assert!(request.is_none(), "unexpected HTTP request for {field}");
        }
        assert!(runtime_is_installed(&serde_json::json!({
            "runtime": {"installed": true, "version": "", "metadata_repairable": false}
        }))
        .is_err());
    }

    #[test]
    fn missing_runtime_is_repaired_only_from_an_exact_pin() {
        assert_eq!(
            runtime_plan(false, false, PinState::Exact("2.4.1".into())),
            Ok(RuntimePlan::Repair("2.4.1".into()))
        );
        assert!(runtime_plan(false, false, PinState::Invalid("latest".into())).is_err());
    }

    #[test]
    fn reads_only_regular_exact_version_pins_and_accepts_legacy_prefix() {
        use std::os::unix::fs::symlink;

        let var = temp_var("pin-state");
        fs::create_dir_all(&var).unwrap();
        let pin = var.join("qwenpaw-version");
        assert_eq!(
            read_pin_state(var.to_str().unwrap()).unwrap(),
            PinState::Missing
        );

        fs::write(&pin, "Version: 2.4.1\n").unwrap();
        assert_eq!(
            read_pin_state(var.to_str().unwrap()).unwrap(),
            PinState::Exact("2.4.1".into())
        );
        fs::write(&pin, "latest\n").unwrap();
        assert_eq!(
            read_pin_state(var.to_str().unwrap()).unwrap(),
            PinState::Invalid("latest".into())
        );
        fs::remove_file(&pin).unwrap();
        fs::write(var.join("pin-target"), "2.4.1\n").unwrap();
        symlink(var.join("pin-target"), &pin).unwrap();
        assert_eq!(
            read_pin_state(var.to_str().unwrap()).unwrap(),
            PinState::Invalid("版本钉不是普通文件".into())
        );
        fs::remove_dir_all(var).unwrap();
    }

    #[test]
    fn mirror_seed_is_written_only_when_missing_and_non_regular_paths_are_rejected() {
        use std::os::unix::fs::symlink;

        let var = temp_var("mirror-seed");
        fs::create_dir_all(&var).unwrap();
        let cfg = Config {
            var_dir: var.to_string_lossy().into_owned(),
            etc_dir: var.join("etc").to_string_lossy().into_owned(),
            home_dir: var.join("home").to_string_lossy().into_owned(),
            share_dir: var.join("share").to_string_lossy().into_owned(),
            ui_config_file: var.join("ui-config").to_string_lossy().into_owned(),
            supervisor_binary: var.join("bin/qwenpaw-supervisor"),
            kernel_venv: var.join("venv").to_string_lossy().into_owned(),
            kernel_pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
            kernel_log: var.join("kernel.log").to_string_lossy().into_owned(),
            kernel_port: crate::api::test_support::free_port(),
            web_proxy_port: None,
            kernel_log_level: "info".into(),
            token_file: var.join("supervisor-token").to_string_lossy().into_owned(),
            supervisor_pid_file: var.join("supervisor.pid").to_string_lossy().into_owned(),
            supervisor_log: var.join("supervisor.log").to_string_lossy().into_owned(),
            listen_port: crate::api::test_support::free_port(),
            auto_start: false,
            gateway_socket: None,
        };

        mirror_seed_if_needed(&cfg, "official").unwrap();
        let path = var.join("pip-mirror");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "https://pypi.org/simple\n"
        );
        fs::write(&path, "https://custom.example/simple\n").unwrap();
        mirror_seed_if_needed(&cfg, "aliyun").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "https://custom.example/simple\n"
        );
        assert_eq!(
            mirror_for_install(&cfg).unwrap(),
            "https://custom.example/simple"
        );
        fs::remove_file(path).unwrap();
        let target = var.join("mirror-target");
        fs::write(&target, "https://target.example/simple\n").unwrap();
        symlink(&target, var.join("pip-mirror")).unwrap();
        assert!(mirror_seed_if_needed(&cfg, "aliyun").is_err());
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            "https://target.example/simple\n"
        );
        fs::remove_dir_all(var).unwrap();
    }
}
