use std::{
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use crate::{config::Config, service};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Keep,
    Remove,
}

struct DeleteTarget {
    label: &'static str,
    path: PathBuf,
}

struct Plan {
    mode: Mode,
    targets: Vec<DeleteTarget>,
    warnings: Vec<String>,
}

struct RemovalReport {
    failures: Vec<String>,
    remaining: Vec<PathBuf>,
}

pub fn run(cfg: &Config) -> i32 {
    let option = env::var("wizard_keep_data").unwrap_or_else(|_| "keep".to_string());
    let share_paths = env::var("TRIM_DATA_SHARE_PATHS")
        .ok()
        .filter(|paths| !paths.trim().is_empty());
    let log_path = env::var_os("TRIM_TEMP_LOGFILE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let preserve_runtime = env::var("TRIM_APP_STATUS").is_ok_and(|status| status == "UPGRADE");
    match service::with_lifecycle_lock(cfg, |expected_exe| {
        run_with_runtime(
            cfg,
            &option,
            share_paths.as_deref(),
            log_path.as_deref(),
            preserve_runtime,
            || match service::stop_for_uninstall(cfg, expected_exe) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("service stop during uninstall failed: {error}");
                    1
                }
            },
            remove_tree,
        )
    }) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("cannot acquire uninstall lifecycle lock: {error}");
            1
        }
    }
}

#[cfg(test)]
fn run_with<FStop, FRemove>(
    cfg: &Config,
    option: &str,
    share_paths: Option<&str>,
    log_path: Option<&Path>,
    stop: FStop,
    remove: FRemove,
) -> i32
where
    FStop: FnOnce() -> i32,
    FRemove: FnMut(&Path) -> io::Result<()>,
{
    run_with_runtime(cfg, option, share_paths, log_path, false, stop, remove)
}

fn run_with_runtime<FStop, FRemove>(
    cfg: &Config,
    option: &str,
    share_paths: Option<&str>,
    log_path: Option<&Path>,
    preserve_runtime: bool,
    stop: FStop,
    remove: FRemove,
) -> i32
where
    FStop: FnOnce() -> i32,
    FRemove: FnMut(&Path) -> io::Result<()>,
{
    let (mode, unknown_option) = parse_mode(option);
    let plan = match build_plan_with_runtime(cfg, mode, share_paths, preserve_runtime) {
        Ok(plan) => plan,
        Err(error) => {
            if unknown_option {
                log_message(
                    "WARN: 未知卸载选项，默认保留数据。",
                    log_path,
                    &log_guard_paths(cfg, mode, share_paths),
                );
            }
            log_message(
                &format!("卸载路径校验失败: {error}"),
                log_path,
                &log_guard_paths(cfg, mode, share_paths),
            );
            return 1;
        }
    };
    let guards = plan
        .targets
        .iter()
        .map(|target| target.path.clone())
        .collect::<Vec<_>>();

    for warning in &plan.warnings {
        log_message(warning, log_path, &guards);
    }
    if unknown_option {
        log_message("WARN: 未知卸载选项，默认保留数据。", log_path, &guards);
    }
    match plan.mode {
        Mode::Keep => log_message(
            if preserve_runtime {
                "外壳更新保留数据与 home/venv；不删除运行环境。"
            } else {
                "用户选择保留数据；移除 home/venv，保留 pin、共享数据和认证。"
            },
            log_path,
            &guards,
        ),
        Mode::Remove => log_message("用户选择完整删除；该操作不可回滚。", log_path, &guards),
    }

    let stop_status = stop();
    if stop_status != 0 {
        log_message(
            &format!("原生 service stop 失败，退出码 {stop_status}；未执行删除。"),
            log_path,
            &guards,
        );
        return 1;
    }

    let report = remove_targets(&plan.targets, remove);
    for failure in &report.failures {
        log_message(&format!("删除失败: {failure}"), log_path, &guards);
    }
    if !report.failures.is_empty() || !report.remaining.is_empty() {
        log_message(
            "卸载删除未完整完成；不会回滚已删除内容。剩余路径清单:",
            log_path,
            &guards,
        );
        if report.remaining.is_empty() {
            log_message(
                "  (扫描时未发现剩余路径；删除操作仍报告失败)",
                log_path,
                &guards,
            );
        }
        for path in &report.remaining {
            log_message(&format!("  {}", path.display()), log_path, &guards);
        }
        return 1;
    }

    log_message("卸载清理完成。", log_path, &guards);
    0
}

fn parse_mode(option: &str) -> (Mode, bool) {
    match option {
        "keep" | "yes" | "true" | "1" => (Mode::Keep, false),
        "remove" | "no" | "false" | "0" => (Mode::Remove, false),
        _ => (Mode::Keep, true),
    }
}

#[cfg(test)]
fn build_plan(cfg: &Config, mode: Mode, share_paths: Option<&str>) -> Result<Plan, String> {
    build_plan_with_runtime(cfg, mode, share_paths, false)
}

fn build_plan_with_runtime(
    cfg: &Config,
    mode: Mode,
    share_paths: Option<&str>,
    preserve_runtime: bool,
) -> Result<Plan, String> {
    let home = validate_package_directory(Path::new(&cfg.home_dir), "home", "@apphome")?;
    let etc = validate_package_directory(Path::new(&cfg.etc_dir), "etc", "@appconf")?;
    let var = validate_package_directory(Path::new(&cfg.var_dir), "var", "@appdata")?;
    ensure_disjoint(&[home.clone(), etc.clone(), var.clone()], "package roots")?;

    match mode {
        Mode::Keep => {
            let venv = home.join("venv");
            if Path::new(&cfg.kernel_venv) != venv {
                return Err("kernel_venv does not match home/venv".into());
            }
            validate_named_directory(&venv, "venv", "home virtual environment")?;
            Ok(Plan {
                mode,
                targets: if preserve_runtime {
                    Vec::new()
                } else {
                    vec![DeleteTarget {
                        label: "home/venv",
                        path: venv,
                    }]
                },
                warnings: Vec::new(),
            })
        }
        Mode::Remove => {
            let (share_base, warnings) = select_share_base(cfg, share_paths, &[&home, &etc, &var])?;
            let shared_data = validate_named_directory(
                &share_base.join(".qwenpaw"),
                ".qwenpaw",
                "shared application data",
            )?;
            let shared_secret = validate_named_directory(
                &share_base.join(".qwenpaw.secret"),
                ".qwenpaw.secret",
                "shared application secrets",
            )?;
            let targets = vec![
                DeleteTarget {
                    label: "home",
                    path: home,
                },
                DeleteTarget {
                    label: "etc",
                    path: etc,
                },
                DeleteTarget {
                    label: "shared .qwenpaw",
                    path: shared_data,
                },
                DeleteTarget {
                    label: "shared .qwenpaw.secret",
                    path: shared_secret,
                },
                DeleteTarget {
                    label: "var",
                    path: var,
                },
            ];
            ensure_disjoint(
                &targets
                    .iter()
                    .map(|target| target.path.clone())
                    .collect::<Vec<_>>(),
                "deletion targets",
            )?;
            Ok(Plan {
                mode,
                targets,
                warnings,
            })
        }
    }
}

fn select_share_base(
    cfg: &Config,
    share_paths: Option<&str>,
    package_roots: &[&PathBuf],
) -> Result<(PathBuf, Vec<String>), String> {
    let configured = Path::new(&cfg.share_dir);
    if configured.file_name() != Some(OsStr::new(".qwenpaw")) {
        return Err("configured share_dir must end in .qwenpaw".into());
    }
    let configured_base = configured
        .parent()
        .ok_or_else(|| "configured share_dir has no parent".to_string())?;
    let configured_base = validate_share_base(configured_base)?;
    let children = [
        configured_base.join(".qwenpaw"),
        configured_base.join(".qwenpaw.secret"),
    ];
    if package_roots
        .iter()
        .any(|root| children.iter().any(|child| paths_overlap(root, child)))
    {
        return Err("shared deletion paths overlap home/etc/var".into());
    }
    for name in [".qwenpaw", ".qwenpaw.secret"] {
        validate_named_directory(&configured_base.join(name), name, "shared data")?;
    }

    if let Some(raw) = share_paths {
        let first_segment = raw
            .split(':')
            .map(str::trim)
            .find(|segment| !segment.is_empty())
            .ok_or_else(|| "TRIM_DATA_SHARE_PATHS has no non-empty shared root".to_string())?;
        let first_base = validate_share_base(Path::new(first_segment))?;
        if first_base != configured_base {
            return Err(
                "TRIM_DATA_SHARE_PATHS first shared root does not match configured share_dir parent"
                    .into(),
            );
        }
    }

    Ok((configured_base, Vec::new()))
}

fn validate_share_base(path: &Path) -> Result<PathBuf, String> {
    let base = validate_path(path, "shared root")?;
    let Some(name) = base.file_name() else {
        return Err("shared root has no final component".into());
    };
    if name == OsStr::new(".qwenpaw") || name == OsStr::new(".qwenpaw.secret") {
        return Err("shared root cannot be an application data directory".into());
    }
    reject_symlink_components(&base)?;
    validate_directory_if_present(&base, "shared root")?;
    Ok(base)
}

fn validate_package_directory(
    path: &Path,
    legacy_name: &str,
    platform_parent: &str,
) -> Result<PathBuf, String> {
    let path = validate_path(path, legacy_name)?;
    let has_legacy_name = path.file_name() == Some(OsStr::new(legacy_name));
    let has_platform_name = path.file_name() == Some(OsStr::new("qwenpaw"))
        && path.parent().and_then(|parent| parent.file_name()) == Some(OsStr::new(platform_parent));
    if !has_legacy_name && !has_platform_name {
        return Err(format!(
            "{legacy_name} must be named {legacy_name} or qwenpaw under {platform_parent}"
        ));
    }
    reject_symlink_components(&path)?;
    validate_directory_if_present(&path, legacy_name)?;
    Ok(path)
}

fn validate_named_directory(
    path: &Path,
    expected_name: &str,
    label: &str,
) -> Result<PathBuf, String> {
    let path = validate_path(path, label)?;
    if path.file_name() != Some(OsStr::new(expected_name)) {
        return Err(format!("{label} must be named {expected_name}"));
    }
    reject_symlink_components(&path)?;
    validate_directory_if_present(&path, label)?;
    Ok(path)
}

fn validate_path(path: &Path, label: &str) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err(format!("{label} path is empty"));
    }
    if !path.is_absolute() {
        return Err(format!("{label} path must be absolute"));
    }
    if path == Path::new("/") {
        return Err(format!("{label} path cannot be the filesystem root"));
    }
    if path.to_string_lossy().trim() != path.to_string_lossy() {
        return Err(format!("{label} path has leading or trailing whitespace"));
    }
    for component in path.components() {
        if matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::Prefix(_)
        ) {
            return Err(format!("{label} path contains an unsafe component"));
        }
    }
    if path.file_name().is_none() {
        return Err(format!("{label} path has no final component"));
    }
    Ok(path.to_path_buf())
}

fn reject_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push(Path::new("/")),
            Component::Normal(name) => {
                current.push(name);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(format!(
                            "path traverses a symbolic link: {}",
                            current.display()
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                    Err(error) => {
                        return Err(format!("cannot inspect {}: {error}", current.display()));
                    }
                }
            }
            _ => return Err("path contains an unsafe component".into()),
        }
    }
    Ok(())
}

fn validate_directory_if_present(path: &Path, label: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(format!(
            "{label} is not a real directory: {}",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect {label} {}: {error}",
            path.display()
        )),
    }
}

fn ensure_disjoint(paths: &[PathBuf], label: &str) -> Result<(), String> {
    for (index, left) in paths.iter().enumerate() {
        for right in paths.iter().skip(index + 1) {
            if paths_overlap(left, right) {
                return Err(format!(
                    "{label} overlap: {} and {}",
                    left.display(),
                    right.display()
                ));
            }
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to remove a symlink deletion root",
        )),
        Ok(metadata) if !metadata.is_dir() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to remove a non-directory deletion root",
        )),
        Ok(_) => fs::remove_dir_all(path),
    }
}

fn remove_targets<FRemove>(targets: &[DeleteTarget], mut remove: FRemove) -> RemovalReport
where
    FRemove: FnMut(&Path) -> io::Result<()>,
{
    let mut failures = Vec::new();
    for target in targets {
        if let Err(error) = remove(&target.path) {
            failures.push(format!(
                "{} ({}): {error}",
                target.label,
                target.path.display()
            ));
        }
    }
    let mut remaining = Vec::new();
    for target in targets {
        collect_remaining(&target.path, &mut remaining);
    }
    remaining.sort();
    remaining.dedup();
    RemovalReport {
        failures,
        remaining,
    }
}

fn collect_remaining(path: &Path, remaining: &mut Vec<PathBuf>) {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(_) => {
            remaining.push(path.to_path_buf());
            return;
        }
    };
    remaining.push(path.to_path_buf());
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    match fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => collect_remaining(&entry.path(), remaining),
                    Err(_) => remaining.push(path.to_path_buf()),
                }
            }
        }
        Err(_) => remaining.push(path.to_path_buf()),
    }
}

fn log_guard_paths(cfg: &Config, mode: Mode, share_paths: Option<&str>) -> Vec<PathBuf> {
    match mode {
        Mode::Keep => vec![Path::new(&cfg.home_dir).join("venv")],
        Mode::Remove => {
            let mut paths = vec![
                PathBuf::from(&cfg.home_dir),
                PathBuf::from(&cfg.etc_dir),
                PathBuf::from(&cfg.var_dir),
            ];
            if let Some(raw) = share_paths {
                for segment in raw.split(':').map(str::trim).filter(|s| !s.is_empty()) {
                    let base = Path::new(segment);
                    if base.is_absolute() && base != Path::new("/") {
                        paths.push(base.join(".qwenpaw"));
                        paths.push(base.join(".qwenpaw.secret"));
                    }
                }
            } else {
                paths.push(PathBuf::from(&cfg.share_dir));
                paths.push(Path::new(&cfg.share_dir).with_file_name(".qwenpaw.secret"));
            }
            paths
        }
    }
}

fn log_message(message: &str, log_path: Option<&Path>, guards: &[PathBuf]) {
    println!("{message}");
    let Some(log_path) = log_path else {
        return;
    };
    if log_path_is_unsafe(log_path, guards) {
        return;
    }
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .and_then(|mut file| writeln!(file, "{message}"));
    if let Err(error) = result {
        eprintln!("cannot append uninstall log to TRIM_TEMP_LOGFILE: {error}");
    }
}

fn log_path_is_unsafe(log_path: &Path, guards: &[PathBuf]) -> bool {
    let absolute_log = if log_path.is_absolute() {
        log_path.to_path_buf()
    } else {
        match env::current_dir() {
            Ok(current) => current.join(log_path),
            Err(_) => return true,
        }
    };
    if reject_symlink_components(&absolute_log).is_err() {
        return true;
    }
    let canonical_log = canonicalize_with_missing(&absolute_log);
    guards.iter().any(|guard| {
        absolute_log.starts_with(guard)
            || canonical_log
                .as_ref()
                .zip(canonicalize_with_missing(guard).as_ref())
                .is_some_and(|(log, root)| log.starts_with(root))
    })
}

fn canonicalize_with_missing(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(&current) {
            Ok(mut resolved) => {
                for component in suffix.iter().rev() {
                    resolved.push(component);
                }
                return Some(resolved);
            }
            Err(_) => {
                suffix.push(current.file_name()?.to_os_string());
                current = current.parent()?.to_path_buf();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs, io,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use crate::config::Config;

    use super::{
        build_plan, parse_mode, remove_targets, remove_tree, run_with, run_with_runtime, Mode,
    };

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempTrimRoot(PathBuf);

    impl TempTrimRoot {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("qwenpaw-uninstall-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn config(&self) -> Config {
            let package = self.0.join("package");
            let home = package.join("home");
            let etc = package.join("etc");
            let var = package.join("var");
            let share = self.0.join("shared");
            for path in [&home, &etc, &var, &share] {
                fs::create_dir_all(path).unwrap();
            }
            Config {
                var_dir: var.to_string_lossy().into_owned(),
                etc_dir: etc.to_string_lossy().into_owned(),
                home_dir: home.to_string_lossy().into_owned(),
                share_dir: share.join(".qwenpaw").to_string_lossy().into_owned(),
                ui_config_file: String::new(),
                supervisor_binary: package.join("target/bin/qwenpaw-supervisor"),
                kernel_venv: home.join("venv").to_string_lossy().into_owned(),
                kernel_pid_file: var.join("kernel.pid").to_string_lossy().into_owned(),
                kernel_log: var.join("qwenpaw.log").to_string_lossy().into_owned(),
                kernel_port: crate::api::test_support::free_port(),
                web_proxy_port: None,
                kernel_log_level: "info".into(),
                token_file: var.join("supervisor-token").to_string_lossy().into_owned(),
                supervisor_pid_file: var.join("supervisor.pid").to_string_lossy().into_owned(),
                supervisor_log: var.join("supervisor.log").to_string_lossy().into_owned(),
                listen_port: crate::api::test_support::free_port(),
                auto_start: false,
                gateway_socket: None,
            }
        }

        fn populate(&self, cfg: &Config) {
            let home = Path::new(&cfg.home_dir);
            let etc = Path::new(&cfg.etc_dir);
            let var = Path::new(&cfg.var_dir);
            let share = Path::new(&cfg.share_dir);
            fs::create_dir_all(home.join("venv/bin")).unwrap();
            fs::write(home.join("venv/bin/python"), "venv").unwrap();
            fs::write(home.join("workspace"), "home data").unwrap();
            fs::write(etc.join("settings"), "settings").unwrap();
            fs::write(var.join("qwenpaw-version"), "1.2.3").unwrap();
            fs::create_dir_all(share).unwrap();
            fs::write(share.join("config.json"), "{}").unwrap();
            let secret = share.with_file_name(".qwenpaw.secret");
            fs::create_dir_all(&secret).unwrap();
            fs::write(secret.join("auth.json"), "auth").unwrap();
        }

        fn share_base(&self) -> PathBuf {
            self.0.join("shared")
        }
    }

    impl Drop for TempTrimRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn execute(cfg: &Config, option: &str, share_paths: Option<&str>) -> i32 {
        run_with(cfg, option, share_paths, None, || 0, remove_tree)
    }

    #[test]
    fn failed_service_stop_preserves_every_uninstall_target() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let share_paths = root.share_base().to_string_lossy().into_owned();
        let mut removed = Vec::new();

        assert_eq!(
            run_with(
                &cfg,
                "remove",
                Some(&share_paths),
                None,
                || 1,
                |path| {
                    removed.push(path.to_path_buf());
                    remove_tree(path)
                },
            ),
            1
        );

        assert!(
            removed.is_empty(),
            "failed service stop must gate all deletion"
        );
        assert!(Path::new(&cfg.home_dir).join("workspace").exists());
        assert!(Path::new(&cfg.etc_dir).join("settings").exists());
        assert!(Path::new(&cfg.var_dir).join("qwenpaw-version").exists());
        assert!(Path::new(&cfg.share_dir).join("config.json").exists());
        assert!(Path::new(&cfg.share_dir)
            .with_file_name(".qwenpaw.secret")
            .join("auth.json")
            .exists());
    }

    #[test]
    fn keep_removes_runtime_and_preserves_pin_data_and_auth() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);

        assert_eq!(execute(&cfg, "keep", None), 0);

        assert!(!Path::new(&cfg.home_dir).join("venv").exists());
        assert!(Path::new(&cfg.home_dir).join("workspace").exists());
        assert!(Path::new(&cfg.var_dir).join("qwenpaw-version").exists());
        assert!(Path::new(&cfg.etc_dir).join("settings").exists());
        assert!(Path::new(&cfg.share_dir).join("config.json").exists());
        assert!(Path::new(&cfg.share_dir)
            .with_file_name(".qwenpaw.secret")
            .join("auth.json")
            .exists());
    }

    #[test]
    fn upgrade_preserves_runtime_without_environment_mutation() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);

        assert_eq!(
            run_with_runtime(&cfg, "keep", None, None, true, || 0, remove_tree),
            0
        );

        assert!(Path::new(&cfg.home_dir).join("venv").exists());
        assert!(Path::new(&cfg.var_dir).join("qwenpaw-version").exists());
        assert!(Path::new(&cfg.share_dir).join("config.json").exists());
    }

    #[test]
    fn real_fn_os_package_roots_are_accepted_for_keep_and_remove() {
        let root = TempTrimRoot::new();
        let mut cfg = root.config();
        let package = root.0.join("vol1");
        let home = package.join("@apphome/qwenpaw");
        let etc = package.join("@appconf/qwenpaw");
        let var = package.join("@appdata/qwenpaw");
        for path in [&home, &etc, &var] {
            fs::create_dir_all(path).unwrap();
        }
        cfg.home_dir = home.to_string_lossy().into_owned();
        cfg.etc_dir = etc.to_string_lossy().into_owned();
        cfg.var_dir = var.to_string_lossy().into_owned();
        cfg.kernel_venv = home.join("venv").to_string_lossy().into_owned();
        cfg.kernel_pid_file = var.join("kernel.pid").to_string_lossy().into_owned();
        cfg.kernel_log = var.join("qwenpaw.log").to_string_lossy().into_owned();
        cfg.token_file = var.join("supervisor-token").to_string_lossy().into_owned();
        cfg.supervisor_pid_file = var.join("supervisor.pid").to_string_lossy().into_owned();
        cfg.supervisor_log = var.join("supervisor.log").to_string_lossy().into_owned();
        root.populate(&cfg);
        let share_paths = root.share_base().to_string_lossy().into_owned();

        let keep = build_plan(&cfg, Mode::Keep, None);
        assert!(
            keep.is_ok(),
            "real fnOS keep layout was rejected: {:?}",
            keep.err()
        );
        let remove = build_plan(&cfg, Mode::Remove, Some(&share_paths));
        assert!(
            remove.is_ok(),
            "real fnOS remove layout was rejected: {:?}",
            remove.err()
        );

        let invalid_roots = [
            ("home", package.join("@appdata/qwenpaw")),
            ("etc", package.join("@appconf/not-qwenpaw")),
            ("var", package.join("@wrong/qwenpaw")),
        ];
        for (field, path) in invalid_roots {
            let mut invalid_cfg = cfg.clone();
            match field {
                "home" => invalid_cfg.home_dir = path.to_string_lossy().into_owned(),
                "etc" => invalid_cfg.etc_dir = path.to_string_lossy().into_owned(),
                "var" => invalid_cfg.var_dir = path.to_string_lossy().into_owned(),
                _ => unreachable!(),
            }
            assert!(
                build_plan(&invalid_cfg, Mode::Keep, None).is_err(),
                "invalid {field} platform root was accepted: {}",
                path.display()
            );
        }
    }

    #[test]
    fn remove_deletes_only_the_explicit_manifest_and_keeps_outer_sentinel() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let sentinel = root.0.join("sentinel");
        fs::write(&sentinel, "outside").unwrap();
        let share_paths = root.share_base().to_string_lossy().into_owned();

        assert_eq!(execute(&cfg, "remove", Some(&share_paths)), 0);

        assert!(!Path::new(&cfg.home_dir).exists());
        assert!(!Path::new(&cfg.etc_dir).exists());
        assert!(!Path::new(&cfg.var_dir).exists());
        assert!(!Path::new(&cfg.share_dir).exists());
        assert!(!Path::new(&cfg.share_dir)
            .with_file_name(".qwenpaw.secret")
            .exists());
        assert!(sentinel.exists());
        assert!(root.0.join("package").is_dir());
        assert!(root.share_base().is_dir());
    }

    #[test]
    fn invalid_first_shared_root_is_not_skipped_for_a_later_valid_root() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let share_paths = format!("/:{}", root.share_base().display());
        let mut stopped = false;
        let mut removed = Vec::new();

        assert_eq!(
            run_with(
                &cfg,
                "remove",
                Some(&share_paths),
                None,
                || {
                    stopped = true;
                    0
                },
                |path| {
                    removed.push(path.to_path_buf());
                    remove_tree(path)
                },
            ),
            1
        );

        assert!(!stopped);
        assert!(removed.is_empty());
        assert!(Path::new(&cfg.home_dir).join("workspace").exists());
        assert!(Path::new(&cfg.share_dir).join("config.json").exists());
    }

    #[test]
    fn mismatched_runtime_shared_root_fails_before_stop_or_deletion() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let alternate = root.0.join("alternate-shared");
        let alternate_data = alternate.join(".qwenpaw");
        let alternate_secret = alternate.join(".qwenpaw.secret");
        fs::create_dir_all(&alternate_data).unwrap();
        fs::write(alternate_data.join("config.json"), "alternate").unwrap();
        fs::create_dir_all(&alternate_secret).unwrap();
        fs::write(alternate_secret.join("auth.json"), "alternate auth").unwrap();
        let share_paths = format!("{}:{}", alternate.display(), root.share_base().display());
        let mut stopped = false;
        let mut removed = Vec::new();

        assert_eq!(
            run_with(
                &cfg,
                "remove",
                Some(&share_paths),
                None,
                || {
                    stopped = true;
                    0
                },
                |path| {
                    removed.push(path.to_path_buf());
                    remove_tree(path)
                },
            ),
            1
        );

        assert!(!stopped);
        assert!(removed.is_empty());
        assert!(Path::new(&cfg.home_dir).join("workspace").exists());
        assert!(Path::new(&cfg.share_dir).join("config.json").exists());
        assert!(alternate_data.join("config.json").exists());
        assert!(alternate_secret.join("auth.json").exists());
    }

    #[test]
    fn valid_first_shared_root_matches_config_and_later_roots_are_not_targets() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let alternate = root.0.join("alternate-shared");
        let alternate_data = alternate.join(".qwenpaw");
        let alternate_secret = alternate.join(".qwenpaw.secret");
        fs::create_dir_all(&alternate_data).unwrap();
        fs::write(alternate_data.join("config.json"), "alternate").unwrap();
        fs::create_dir_all(&alternate_secret).unwrap();
        fs::write(alternate_secret.join("auth.json"), "alternate auth").unwrap();
        let share_paths = format!("{}:{}", root.share_base().display(), alternate.display());

        assert_eq!(execute(&cfg, "remove", Some(&share_paths)), 0);

        assert!(!Path::new(&cfg.share_dir).exists());
        assert!(!Path::new(&cfg.share_dir)
            .with_file_name(".qwenpaw.secret")
            .exists());
        assert!(alternate_data.join("config.json").exists());
        assert!(alternate_secret.join("auth.json").exists());
    }

    #[test]
    fn unknown_option_warns_and_defaults_to_keep() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);

        let (mode, unknown) = parse_mode("unexpected");
        assert_eq!(mode, Mode::Keep);
        assert!(unknown);
        assert_eq!(execute(&cfg, "unexpected", None), 0);
        assert!(!Path::new(&cfg.home_dir).join("venv").exists());
        assert!(Path::new(&cfg.home_dir).join("workspace").exists());
        assert!(Path::new(&cfg.var_dir).join("qwenpaw-version").exists());
    }

    #[test]
    fn explicit_option_aliases_map_to_the_expected_modes() {
        for option in ["keep", "yes", "true", "1"] {
            assert_eq!(parse_mode(option), (Mode::Keep, false));
        }
        for option in ["remove", "no", "false", "0"] {
            assert_eq!(parse_mode(option), (Mode::Remove, false));
        }
    }

    #[test]
    fn invalid_roots_and_overlapping_shared_root_are_rejected_before_stop() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let mut invalid_cfg = cfg.clone();
        invalid_cfg.home_dir.clear();
        let mut stopped = false;
        assert_eq!(
            run_with(
                &invalid_cfg,
                "remove",
                Some(&root.share_base().to_string_lossy()),
                None,
                || {
                    stopped = true;
                    0
                },
                remove_tree,
            ),
            1
        );
        assert!(!stopped);
        assert!(Path::new(&cfg.home_dir).join("workspace").exists());

        let overlapping = cfg.home_dir.to_string();
        assert_eq!(execute(&cfg, "remove", Some(&overlapping)), 1);
        assert!(Path::new(&cfg.home_dir).join("workspace").exists());
    }

    #[test]
    fn empty_root_parent_traversal_and_wrong_leaf_are_rejected() {
        let root = TempTrimRoot::new();
        assert!(super::validate_path(Path::new(""), "home").is_err());
        assert!(super::validate_path(Path::new("/"), "home").is_err());
        assert!(super::validate_path(&root.0.join("package/../outside"), "home").is_err());
        assert!(super::validate_named_directory(&root.0.join("outside"), "home", "home").is_err());
    }

    #[test]
    fn symlinked_root_is_rejected_and_external_target_is_untouched() {
        use std::os::unix::fs::symlink;

        let root = TempTrimRoot::new();
        let mut cfg = root.config();
        root.populate(&cfg);
        let home = PathBuf::from(&cfg.home_dir);
        let external = root.0.join("external-home");
        fs::rename(&home, &external).unwrap();
        symlink(&external, &home).unwrap();
        cfg.home_dir = home.to_string_lossy().into_owned();

        let mut stopped = false;
        assert_eq!(
            run_with(
                &cfg,
                "keep",
                None,
                None,
                || {
                    stopped = true;
                    0
                },
                remove_tree
            ),
            1
        );
        assert!(!stopped);
        assert!(external.join("workspace").exists());
    }

    #[test]
    fn recursive_delete_removes_symlink_itself_without_following_target() {
        use std::os::unix::fs::symlink;

        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let external = root.0.join("external");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("sentinel"), "outside").unwrap();
        symlink(&external, Path::new(&cfg.home_dir).join("linked-outside")).unwrap();
        let share_paths = root.share_base().to_string_lossy().into_owned();

        assert_eq!(execute(&cfg, "remove", Some(&share_paths)), 0);
        assert!(external.join("sentinel").exists());
    }

    #[test]
    fn partial_delete_failure_returns_nonzero_and_reports_remaining_paths() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let share_paths = root.share_base().to_string_lossy().into_owned();
        let plan = build_plan(&cfg, Mode::Remove, Some(&share_paths)).unwrap();
        let home = PathBuf::from(&cfg.home_dir);
        let report = remove_targets(&plan.targets, |path| {
            if path == home {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
            } else {
                remove_tree(path)
            }
        });
        assert_eq!(report.failures.len(), 1);
        assert!(report.remaining.contains(&home.join("workspace")));

        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let share_paths = root.share_base().to_string_lossy().into_owned();
        let home = PathBuf::from(&cfg.home_dir);
        assert_eq!(
            run_with(
                &cfg,
                "remove",
                Some(&share_paths),
                None,
                || 0,
                |path| {
                    if path == home {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
                    } else {
                        remove_tree(path)
                    }
                }
            ),
            1
        );
        assert!(home.join("workspace").exists());
        assert!(!Path::new(&cfg.var_dir).exists());
    }

    #[test]
    fn logfile_inside_removed_var_is_suppressed() {
        let root = TempTrimRoot::new();
        let cfg = root.config();
        root.populate(&cfg);
        let log_path = Path::new(&cfg.var_dir).join("uninstall.log");
        let share_paths = root.share_base().to_string_lossy().into_owned();
        let log_was_absent_during_deletion = std::cell::Cell::new(true);

        assert_eq!(
            run_with(
                &cfg,
                "remove",
                Some(&share_paths),
                Some(&log_path),
                || 0,
                |path| {
                    if path == Path::new(&cfg.var_dir) {
                        log_was_absent_during_deletion.set(!log_path.exists());
                    }
                    remove_tree(path)
                },
            ),
            0
        );
        assert!(log_was_absent_during_deletion.get());
    }
}
