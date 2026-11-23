use super::{InstallChannel, TxPhase, VenvTxRecord};
use ring::digest::{digest, SHA256};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

const MARKER_FILE: &str = ".qwenpaw-base-path-patch";
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_MANIFEST_FILES: usize = 4096;
const MAX_MANIFEST_DELETE_FILES: usize = 8192;
const MAX_PAYLOAD_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 128 * 1024 * 1024;

#[derive(Deserialize)]
struct Manifest {
    schema_version: u32,
    base_version: String,
    files: Vec<ManifestFile>,
    #[serde(default)]
    console_complete: bool,
    #[serde(default)]
    delete_files: Vec<ManifestDelete>,
}

#[derive(Deserialize)]
struct ManifestFile {
    path: String,
    original_sha256: Option<String>,
    sha256: String,
    #[serde(default)]
    previous_sha256: Vec<String>,
}

#[derive(Deserialize)]
struct ManifestDelete {
    path: String,
    original_sha256: String,
    #[serde(default)]
    previous_sha256: Vec<String>,
}

pub(crate) struct BasePathPatch {
    root: PathBuf,
    fingerprint: String,
    base_version: String,
    files: Vec<ManifestFile>,
    delete_files: Vec<ManifestDelete>,
}

fn channel_base_version(channel: &InstallChannel) -> Option<&str> {
    match channel {
        InstallChannel::Exact(version) if version == "2.3.0" => Some("2.3.0"),
        InstallChannel::Exact(version) if version == crate::version::verified_kernel_version() => {
            Some(version)
        }
        _ => None,
    }
}

pub(crate) fn for_channel(channel: &InstallChannel) -> Result<Option<BasePathPatch>, String> {
    let Some(base_version) = channel_base_version(channel) else {
        return Ok(None);
    };
    let explicit = std::env::var_os("QWENPAW_BASE_PATH_PATCH").map(PathBuf::from);
    let root = explicit.clone().unwrap_or_else(|| {
        let appdest = std::env::var_os("TRIM_APPDEST")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/apps/qwenpaw/target"));
        appdest.join(format!("www/patches/qwenpaw-{base_version}-base-path"))
    });
    if explicit.is_none() && !root.exists() {
        return Ok(None);
    }
    BasePathPatch::from_root(&root, channel)
}

pub(crate) fn active(venv: &Path) -> bool {
    fs::symlink_metadata(venv.join(MARKER_FILE))
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

impl BasePathPatch {
    pub(crate) fn from_root(root: &Path, channel: &InstallChannel) -> Result<Option<Self>, String> {
        let Some(base_version) = channel_base_version(channel) else {
            return Ok(None);
        };
        require_directory(root)?;
        let raw = read_regular(&root.join("manifest.json"), MAX_MANIFEST_BYTES)?;
        let fingerprint = sha256(&raw);
        let mut manifest: Manifest = serde_json::from_slice(&raw)
            .map_err(|error| format!("base-path 补丁清单不是有效 JSON: {error}"))?;
        if manifest.schema_version != 1 || manifest.base_version != base_version {
            return Err("base-path 补丁清单版本不兼容".into());
        }
        if manifest.files.is_empty()
            || manifest.files.len() > MAX_MANIFEST_FILES
            || manifest.delete_files.len() > MAX_MANIFEST_DELETE_FILES
        {
            return Err("base-path 补丁清单文件数量无效".into());
        }
        if manifest.console_complete
            && !manifest
                .files
                .iter()
                .any(|file| file.path == "console/index.html")
        {
            return Err("完整 Console 补丁清单缺少 console/index.html".into());
        }
        let mut paths = BTreeSet::new();
        for file in &mut manifest.files {
            insert_manifest_path(&mut paths, &file.path)?;
            normalize_digest(&mut file.sha256)?;
            if let Some(original) = &mut file.original_sha256 {
                normalize_digest(original)?;
            }
            for previous in &mut file.previous_sha256 {
                normalize_digest(previous)?;
            }
        }
        for file in &mut manifest.delete_files {
            insert_manifest_path(&mut paths, &file.path)?;
            if !file.path.starts_with("console/") {
                return Err("base-path 删除清单只能包含 Console 文件".into());
            }
            normalize_digest(&mut file.original_sha256)?;
            for previous in &mut file.previous_sha256 {
                normalize_digest(previous)?;
            }
        }
        for path in &paths {
            if Path::new(path)
                .ancestors()
                .skip(1)
                .any(|parent| paths.contains(&parent.to_string_lossy().into_owned()))
            {
                return Err(format!("base-path 补丁清单文件路径相互嵌套: {path}"));
            }
        }
        let patch = Self {
            root: root.to_path_buf(),
            fingerprint,
            base_version: base_version.into(),
            files: manifest.files,
            delete_files: manifest.delete_files,
        };
        // Validate the entire source before an upgrade can request a venv repair.
        patch.load_payload()?;
        Ok(Some(patch))
    }

    pub(crate) fn needs_apply(&self, venv: &Path) -> Result<bool, String> {
        let marker = venv.join(MARKER_FILE);
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err("base-path 补丁指纹路径不是普通文件".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(format!("检查 base-path 补丁指纹失败: {error}")),
        }
        let current = read_regular(&marker, MAX_MANIFEST_BYTES)?;
        let current = std::str::from_utf8(&current)
            .map_err(|error| format!("读取 base-path 补丁指纹失败: {error}"))?;
        Ok(current.trim() != self.fingerprint)
    }

    pub(crate) fn includes_console(&self) -> bool {
        self.files
            .iter()
            .any(|file| file.path.starts_with("console/"))
    }

    fn load_payload(&self) -> Result<Vec<Vec<u8>>, String> {
        require_directory(&self.root)?;
        let payload = self.root.join("payload");
        require_directory(&payload)?;
        let expected: BTreeSet<_> = self.files.iter().map(|file| file.path.as_str()).collect();
        let mut pending = vec![payload.clone()];
        let mut entries = 0;
        while let Some(directory) = pending.pop() {
            require_directory(&directory)?;
            for entry in fs::read_dir(&directory)
                .map_err(|error| format!("读取补丁载荷目录失败: {error}"))?
            {
                let entry = entry.map_err(|error| format!("读取补丁载荷条目失败: {error}"))?;
                entries += 1;
                let path = entry.path();
                let relative = path
                    .strip_prefix(&payload)
                    .map_err(|error| format!("补丁载荷路径越界: {error}"))?;
                if entries > MAX_MANIFEST_FILES * 8 || relative.components().count() > 64 {
                    return Err("base-path 补丁载荷目录超过限制".into());
                }
                let kind = entry
                    .file_type()
                    .map_err(|error| format!("检查补丁载荷条目失败: {error}"))?;
                if kind.is_dir() {
                    pending.push(path);
                } else if !kind.is_file() {
                    return Err(format!(
                        "补丁载荷不是普通文件或实际目录: {}",
                        path.display()
                    ));
                } else if !relative
                    .to_str()
                    .map(|name| expected.contains(name))
                    .unwrap_or(false)
                {
                    return Err(format!("补丁载荷包含清单外文件: {}", relative.display()));
                }
            }
        }
        let mut contents = Vec::with_capacity(self.files.len());
        let mut total = 0;
        for file in &self.files {
            let relative = relative_payload_path(&file.path)?;
            let source = existing_target(&payload, relative)?
                .ok_or_else(|| format!("base-path 补丁源文件缺失: {}", file.path))?;
            let patched = read_regular(&source, MAX_PAYLOAD_FILE_BYTES)?;
            if sha256(&patched) != file.sha256 {
                return Err(format!("base-path 补丁文件哈希不匹配: {}", file.path));
            }
            total += patched.len();
            if total > MAX_PAYLOAD_BYTES {
                return Err("base-path 补丁载荷总大小超过限制".into());
            }
            contents.push(patched);
        }
        Ok(contents)
    }

    pub(crate) fn apply(&self, venv: &Path) -> Result<(), String> {
        if !self.delete_files.is_empty() {
            require_prepared_staging(venv, &self.base_version)?;
        }
        let package = python_package(venv)?;
        // Keep the verified bytes; do not copy a source that could change after preflight.
        let payload = self.load_payload()?;
        for file in &self.files {
            let relative = relative_payload_path(&file.path)?;
            match existing_target(&package, relative)? {
                Some(target) => {
                    let current_hash = sha256(&read_regular(&target, MAX_PAYLOAD_FILE_BYTES)?);
                    if current_hash != file.sha256
                        && file.original_sha256.as_ref() != Some(&current_hash)
                        && !file.previous_sha256.contains(&current_hash)
                    {
                        return Err(format!("官方内核文件已被修改，拒绝覆盖: {}", file.path));
                    }
                }
                None if file.original_sha256.is_some() => {
                    return Err(format!("官方内核文件缺失: {}", file.path));
                }
                None => {}
            }
        }
        for file in &self.delete_files {
            check_delete_target(&package, file)?;
        }
        // All writes/deletes have passed preflight. The outer venv transaction owns rollback.
        for (file, data) in self.files.iter().zip(payload) {
            let relative = relative_payload_path(&file.path)?;
            ensure_parent_directories(&package, relative)?;
            let target = package.join(relative);
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(0o644)
                .open(&target)
                .map_err(|error| format!("写入内核补丁 {} 失败: {error}", file.path))?;
            output
                .write_all(&data)
                .map_err(|error| format!("写入内核补丁 {} 失败: {error}", file.path))?;
            output
                .set_permissions(fs::Permissions::from_mode(0o644))
                .map_err(|error| format!("设置内核补丁权限 {} 失败: {error}", file.path))?;
        }
        for file in &self.delete_files {
            // Recheck the directory chain and hash immediately before removing this file.
            if let Some(target) = check_delete_target(&package, file)? {
                fs::remove_file(&target)
                    .map_err(|error| format!("删除旧 Console 文件 {} 失败: {error}", file.path))?;
            }
        }
        let marker = venv.join(MARKER_FILE);
        super::atomic_write_0600(
            &marker.to_string_lossy(),
            &format!("{}\n", self.fingerprint),
        )
        .map_err(|error| format!("写入 base-path 补丁指纹失败: {error}"))?;
        Ok(())
    }
}

fn normalize_digest(value: &mut str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("base-path 补丁清单包含无效 SHA256".into());
    }
    value.make_ascii_lowercase();
    Ok(())
}

fn insert_manifest_path(paths: &mut BTreeSet<String>, value: &str) -> Result<(), String> {
    relative_payload_path(value)?;
    if !paths.insert(value.to_string()) {
        return Err(format!("base-path 补丁清单包含重复路径: {value}"));
    }
    Ok(())
}

fn relative_payload_path(value: &str) -> Result<&Path, String> {
    let path = Path::new(value);
    let mut components = path.components();
    let valid_root = matches!(components.next(), Some(Component::Normal(root)) if root == "app" || root == "browser" || root == "cli" || root == "console" || root == "pawapp");
    let extra_client = matches!(
        value,
        "agents/tools/agent_management.py"
            | "utils/startup_display.py"
            | "agents/memory/proactive/proactive_responder.py"
            | "envs/store.py"
            | "tauri/entry.py"
            | "utils/runtime_api.py"
            | "channel_protocol/models.py"
            | "official_channels/voice-0.1.0.zip"
    );
    if !(valid_root || extra_client)
        || !value.contains('/')
        || value.len() > 4096
        || value.contains(['\\', ':'])
        || value.chars().any(char::is_control)
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("base-path 补丁路径越界: {value}"));
    }
    Ok(path)
}

fn require_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法检查目录 {}: {error}", path.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(format!("不是实际目录: {}", path.display()));
    }
    Ok(())
}

fn read_regular(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("无法打开普通文件 {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("无法检查普通文件 {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("不是普通文件: {}", path.display()));
    }
    if metadata.len() > maximum as u64 {
        return Err(format!("文件超过大小限制: {}", path.display()));
    }
    let mut data = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|error| format!("读取文件 {} 失败: {error}", path.display()))?;
    if data.len() > maximum {
        return Err(format!("文件超过大小限制: {}", path.display()));
    }
    Ok(data)
}

// None means a missing target or parent, never a symlink or a non-directory parent.
fn existing_target(package: &Path, relative: &Path) -> Result<Option<PathBuf>, String> {
    require_directory(package)?;
    let mut current = package.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("内核补丁路径无效".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if current == package.join(relative) => {
                if !metadata.file_type().is_file() {
                    return Err(format!("内核补丁目标不是普通文件: {}", current.display()));
                }
                return Ok(Some(current));
            }
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(format!("内核补丁父路径不是实际目录: {}", current.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("检查内核补丁路径失败: {error}")),
        }
    }
    Err("内核补丁路径为空".into())
}

fn check_delete_target(package: &Path, file: &ManifestDelete) -> Result<Option<PathBuf>, String> {
    let relative = relative_payload_path(&file.path)?;
    let Some(target) = existing_target(package, relative)? else {
        return Ok(None);
    };
    let current_hash = sha256(&read_regular(&target, MAX_PAYLOAD_FILE_BYTES)?);
    if current_hash != file.original_sha256 && !file.previous_sha256.contains(&current_hash) {
        return Err(format!("旧 Console 文件已被修改，拒绝删除: {}", file.path));
    }
    Ok(Some(target))
}

fn require_prepared_staging(venv: &Path, base_version: &str) -> Result<(), String> {
    require_directory(venv)?;
    let tx_id = venv
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".venv.staging."))
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
        .ok_or("删除旧 Console 文件只允许在 venv 事务 staging 中执行")?;
    let parent = venv.parent().ok_or("venv staging 缺少父目录")?;
    let raw = read_regular(
        &parent.join(format!(".venv.transaction.{tx_id}.json")),
        64 * 1024,
    )?;
    let record: VenvTxRecord = serde_json::from_slice(&raw)
        .map_err(|error| format!("无法读取 Console 清理所属的 venv 事务: {error}"))?;
    if record.schema_version != 1
        || record.tx_id != tx_id
        || record.phase != TxPhase::Prepared
        || record.target_version != base_version
        || record.staged != venv
        || record.active.parent() != Some(parent)
        || record.active == record.staged
        || record.active == record.backup
        || record.backup != parent.join(format!(".venv.rollback.{tx_id}"))
    {
        return Err(format!(
            "Console 清理目标不属于已准备的 {base_version} venv 事务"
        ));
    }
    Ok(())
}

pub(crate) fn python_package(venv: &Path) -> Result<PathBuf, String> {
    require_directory(venv)?;
    let lib = venv.join("lib");
    require_directory(&lib)?;
    let mut packages = Vec::new();
    for entry in fs::read_dir(&lib).map_err(|error| format!("读取 venv lib 目录失败: {error}"))?
    {
        let entry = entry.map_err(|error| format!("读取 venv lib 条目失败: {error}"))?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("python3.") {
            continue;
        }
        require_directory(&entry.path())?;
        let site = entry.path().join("site-packages");
        match fs::symlink_metadata(&site) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => require_directory(&site)?,
        }
        let package = site.join("qwenpaw");
        match fs::symlink_metadata(&package) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => require_directory(&package)?,
        }
        packages.push(package);
    }
    if packages.len() == 1 {
        Ok(packages.remove(0))
    } else {
        Err("无法唯一确定 venv 内的 qwenpaw 包目录".into())
    }
}

fn ensure_parent_directories(package: &Path, relative: &Path) -> Result<(), String> {
    require_directory(package)?;
    let mut current = package.to_path_buf();
    for component in relative.parent().into_iter().flat_map(Path::components) {
        let Component::Normal(name) = component else {
            return Err("内核补丁父路径无效".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(format!("内核补丁父路径不是实际目录: {}", current.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    format!("创建内核补丁父目录失败 {}: {error}", current.display())
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "检查内核补丁父目录失败 {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn sha256(data: &[u8]) -> String {
    digest(&SHA256, data)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::digest::{digest, SHA256};

    fn sha256(data: &[u8]) -> String {
        hex(digest(&SHA256, data).as_ref())
    }

    fn hex(data: &[u8]) -> String {
        data.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "qwenpaw-base-path-patch-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("patch");
        let venv = dir.join("venv");
        let package = venv.join("lib/python3.12/site-packages/qwenpaw");
        std::fs::create_dir_all(root.join("payload/app")).unwrap();
        std::fs::create_dir_all(package.join("app")).unwrap();
        (root, venv, package)
    }

    fn write_manifest(root: &Path, original: &[u8], patched: &[u8]) {
        std::fs::write(root.join("payload/app/_app.py"), patched).unwrap();
        let manifest = serde_json::json!({
            "schema_version": 1,
            "base_version": "2.3.0",
            "files": [{
                "path": "app/_app.py",
                "original_sha256": sha256(original),
                "sha256": sha256(patched),
            }]
        });
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn edit_manifest(root: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
        let path = root.join("manifest.json");
        let mut manifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        edit(&mut manifest);
        fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    }

    fn load_patch(root: &Path) -> BasePathPatch {
        BasePathPatch::from_root(root, &InstallChannel::Exact("2.3.0".into()))
            .unwrap()
            .unwrap()
    }

    fn staging_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let (root, active, _) = fixture(label);
        let parent = active.parent().unwrap().to_path_buf();
        let tx_id = "123.1";
        let staged = parent.join(format!(".venv.staging.{tx_id}"));
        fs::rename(&active, &staged).unwrap();
        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: tx_id.into(),
            phase: TxPhase::Prepared,
            active,
            staged: staged.clone(),
            backup: parent.join(format!(".venv.rollback.{tx_id}")),
            had_active: false,
            target_version: "2.3.0".into(),
            pin_path: parent.join("qwenpaw-version"),
            previous_pin: None,
            resolved_version: None,
        };
        fs::write(
            parent.join(format!(".venv.transaction.{tx_id}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let package = staged.join("lib/python3.12/site-packages/qwenpaw");
        (root, staged, package)
    }

    fn write_console_manifest(root: &Path) {
        write_manifest(root, b"official", b"patched");
        fs::create_dir_all(root.join("payload/console/assets")).unwrap();
        fs::write(root.join("payload/console/index.html"), b"new-index").unwrap();
        fs::write(root.join("payload/console/assets/new.js"), b"new-js").unwrap();
        edit_manifest(root, |manifest| {
            manifest["files"].as_array_mut().unwrap().extend([
                serde_json::json!({
                    "path": "console/index.html",
                    "original_sha256": sha256(b"official-index"),
                    "sha256": sha256(b"new-index"),
                }),
                serde_json::json!({
                    "path": "console/assets/new.js",
                    "original_sha256": null,
                    "sha256": sha256(b"new-js"),
                }),
            ]);
            manifest["delete_files"] = serde_json::json!([
                {"path": "console/assets/old.js", "original_sha256": sha256(b"old-js")},
                {"path": "console/assets/missing.js", "original_sha256": sha256(b"missing")},
            ]);
        });
    }

    fn write_official_console(package: &Path) {
        fs::create_dir_all(package.join("console/assets")).unwrap();
        fs::write(package.join("app/_app.py"), b"official").unwrap();
        fs::write(package.join("console/index.html"), b"official-index").unwrap();
        fs::write(package.join("console/assets/old.js"), b"old-js").unwrap();
        // A file unchanged from the wheel need not appear in the patch's files list.
        fs::write(package.join("console/assets/shared.js"), b"unchanged-wheel").unwrap();
    }

    #[test]
    fn deletes_only_explicit_known_console_files_inside_prepared_staging() {
        let (root, staged, package) = staging_fixture("delete-console");
        write_console_manifest(&root);
        write_official_console(&package);
        fs::write(package.join("console/unlisted.txt"), b"keep-unlisted").unwrap();
        let patch = load_patch(&root);

        patch.apply(&staged).unwrap();

        assert_eq!(
            fs::read(package.join("console/index.html")).unwrap(),
            b"new-index"
        );
        assert_eq!(
            fs::read(package.join("console/assets/new.js")).unwrap(),
            b"new-js"
        );
        assert!(!package.join("console/assets/old.js").exists());
        assert_eq!(
            fs::read(package.join("console/assets/shared.js")).unwrap(),
            b"unchanged-wheel"
        );
        assert_eq!(
            fs::read(package.join("console/unlisted.txt")).unwrap(),
            b"keep-unlisted"
        );
        assert_eq!(fs::read(package.join("app/_app.py")).unwrap(), b"patched");
        assert!(!patch.needs_apply(&staged).unwrap());
        assert_eq!(
            fs::metadata(package.join("console/index.html"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn requires_an_index_for_console_complete_but_accepts_legacy_schema_one() {
        let (root, _venv, _package) = fixture("complete-no-index");
        write_manifest(&root, b"official", b"patched");
        assert!(!load_patch(&root).includes_console());
        edit_manifest(&root, |manifest| {
            manifest["console_complete"] = serde_json::json!(true)
        });

        assert!(BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into())).is_err());

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn applies_a_complete_console_including_unchanged_wheel_files() {
        let (root, staged, package) = staging_fixture("complete-console");
        write_console_manifest(&root);
        write_official_console(&package);
        fs::write(
            root.join("payload/console/assets/shared.js"),
            b"unchanged-wheel",
        )
        .unwrap();
        edit_manifest(&root, |manifest| {
            manifest["console_complete"] = serde_json::json!(true);
            manifest["files"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "path": "console/assets/shared.js",
                    "original_sha256": sha256(b"unchanged-wheel"),
                    "sha256": sha256(b"unchanged-wheel"),
                }));
        });
        let patch = load_patch(&root);

        patch.apply(&staged).unwrap();
        patch.apply(&staged).unwrap();

        assert_eq!(
            fs::read(package.join("console/assets/shared.js")).unwrap(),
            b"unchanged-wheel"
        );
        assert_eq!(
            fs::read(package.join("console/index.html")).unwrap(),
            b"new-index"
        );
        assert!(!package.join("console/assets/old.js").exists());
        assert!(!patch.needs_apply(&staged).unwrap());
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn refuses_console_deletion_in_an_active_venv_without_writing_anything() {
        let (root, active, package) = fixture("active-no-delete");
        write_console_manifest(&root);
        write_official_console(&package);
        let patch = load_patch(&root);

        assert!(patch.apply(&active).is_err());

        assert_eq!(fs::read(package.join("app/_app.py")).unwrap(), b"official");
        assert_eq!(
            fs::read(package.join("console/index.html")).unwrap(),
            b"official-index"
        );
        assert_eq!(
            fs::read(package.join("console/assets/old.js")).unwrap(),
            b"old-js"
        );
        assert!(!package.join("console/assets/new.js").exists());
        assert!(!active.join(MARKER_FILE).exists());
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn requires_a_matching_prepared_exact_version_transaction_for_deletes() {
        for field in [
            "missing",
            "schema_version",
            "phase",
            "staged",
            "target_version",
            "backup",
        ] {
            let (root, staged, package) = staging_fixture(&format!("bad-journal-{field}"));
            write_console_manifest(&root);
            write_official_console(&package);
            let journal = staged
                .parent()
                .unwrap()
                .join(".venv.transaction.123.1.json");
            if field == "missing" {
                fs::remove_file(&journal).unwrap();
            } else {
                let mut record: serde_json::Value =
                    serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
                record[field] = match field {
                    "schema_version" => serde_json::json!(2),
                    "phase" => serde_json::json!("activated"),
                    "target_version" => serde_json::json!("2.3.1"),
                    _ => serde_json::json!("/outside"),
                };
                fs::write(journal, serde_json::to_vec(&record).unwrap()).unwrap();
            }

            assert!(load_patch(&root).apply(&staged).is_err(), "{field}");
            assert_eq!(
                fs::read(package.join("console/assets/old.js")).unwrap(),
                b"old-js"
            );
            assert_eq!(fs::read(package.join("app/_app.py")).unwrap(), b"official");
            assert!(!staged.join(MARKER_FILE).exists());
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn rejects_unknown_console_or_backend_modification_before_any_write_or_delete() {
        for relative in ["console/assets/old.js", "app/_app.py"] {
            let (root, staged, package) =
                staging_fixture(&format!("unknown-{}", relative.replace('/', "-")));
            write_console_manifest(&root);
            write_official_console(&package);
            fs::write(package.join(relative), b"unknown-local-change").unwrap();
            fs::write(staged.join(MARKER_FILE), b"old-marker\n").unwrap();
            let patch = load_patch(&root);

            assert!(patch.apply(&staged).is_err());

            assert_eq!(
                fs::read(package.join(relative)).unwrap(),
                b"unknown-local-change"
            );
            assert_eq!(
                fs::read(package.join("console/index.html")).unwrap(),
                b"official-index"
            );
            assert!(package.join("console/assets/old.js").exists());
            assert!(!package.join("console/assets/new.js").exists());
            assert_eq!(fs::read(staged.join(MARKER_FILE)).unwrap(), b"old-marker\n");
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn updates_an_old_marker_with_declared_previous_hashes_and_is_idempotent() {
        let (root, staged, package) = staging_fixture("old-marker");
        write_console_manifest(&root);
        write_official_console(&package);
        edit_manifest(&root, |manifest| {
            manifest["files"][0]["previous_sha256"] =
                serde_json::json!([sha256(b"previous-backend")]);
            manifest["delete_files"][0]["previous_sha256"] =
                serde_json::json!([sha256(b"previous-console")]);
        });
        fs::write(package.join("app/_app.py"), b"previous-backend").unwrap();
        fs::write(package.join("console/assets/old.js"), b"previous-console").unwrap();
        fs::write(staged.join(MARKER_FILE), b"old-marker\n").unwrap();
        let patch = load_patch(&root);

        assert!(patch.needs_apply(&staged).unwrap());
        patch.apply(&staged).unwrap();
        assert!(!patch.needs_apply(&staged).unwrap());
        let marker = fs::read(staged.join(MARKER_FILE)).unwrap();
        patch.apply(&staged).unwrap();

        assert_eq!(fs::read(staged.join(MARKER_FILE)).unwrap(), marker);
        assert_eq!(fs::read(package.join("app/_app.py")).unwrap(), b"patched");
        assert!(!package.join("console/assets/old.js").exists());
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn prevalidates_missing_extra_and_corrupt_sources_and_revalidates_before_apply() {
        for problem in ["missing", "extra", "hash"] {
            let (root, venv, package) = fixture(&format!("source-{problem}"));
            write_manifest(&root, b"official", b"patched");
            fs::write(package.join("app/_app.py"), b"official").unwrap();
            let patch = load_patch(&root);
            match problem {
                "missing" => fs::remove_file(root.join("payload/app/_app.py")).unwrap(),
                "extra" => {
                    fs::write(root.join("payload/app/extra.py"), b"not-in-manifest").unwrap()
                }
                _ => fs::write(root.join("payload/app/_app.py"), b"bad-hash").unwrap(),
            }

            assert!(
                BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into())).is_err()
            );
            assert!(patch.apply(&venv).is_err());
            assert_eq!(fs::read(package.join("app/_app.py")).unwrap(), b"official");
            assert!(!venv.join(MARKER_FILE).exists());
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn accepts_browser_and_cli_payloads_without_opening_other_roots() {
        for relative in [
            "app/_app.py",
            "browser/control_link/chrome/ws_handler.py",
            "cli/http.py",
            "agents/tools/agent_management.py",
            "utils/startup_display.py",
            "agents/memory/proactive/proactive_responder.py",
            "envs/store.py",
            "tauri/entry.py",
            "utils/runtime_api.py",
            "channel_protocol/models.py",
            "official_channels/voice-0.1.0.zip",
            "pawapp/deps.py",
            "console/index.html",
        ] {
            assert_eq!(
                relative_payload_path(relative).unwrap(),
                Path::new(relative)
            );
        }
        for other in [
            "tests/unit/app/test_base_path.py",
            "utils/other.py",
            "agents/tools/other.py",
            "channel_protocol/other.py",
            "official_channels/other.zip",
        ] {
            assert!(relative_payload_path(other).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_overlapping_and_noncanonical_paths_and_invalid_digests() {
        for problem in [
            "duplicate",
            "overlap",
            "nested",
            "digest",
            "delete-digest",
            "delete-backend",
            "delete-root",
            "dot",
            "slash",
            "backslash",
            "nul",
        ] {
            let (root, _, _) = fixture(&format!("manifest-{problem}"));
            write_manifest(&root, b"official", b"patched");
            edit_manifest(&root, |manifest| match problem {
                "duplicate" => {
                    let duplicate = manifest["files"][0].clone();
                    manifest["files"].as_array_mut().unwrap().push(duplicate);
                }
                "overlap" => {
                    manifest["delete_files"] = serde_json::json!([{"path": "app/_app.py", "original_sha256": sha256(b"old")}])
                }
                "nested" => manifest["files"].as_array_mut().unwrap().push(
                    serde_json::json!({"path": "app/_app.py/child", "sha256": sha256(b"old")}),
                ),
                "digest" => manifest["files"][0]["sha256"] = serde_json::json!("g".repeat(64)),
                "delete-digest" => {
                    manifest["delete_files"] =
                        serde_json::json!([{"path": "console/old.js", "original_sha256": "bad"}])
                }
                "delete-backend" => {
                    manifest["delete_files"] = serde_json::json!([{"path": "app/other.py", "original_sha256": sha256(b"old")}])
                }
                "delete-root" => {
                    manifest["delete_files"] =
                        serde_json::json!([{"path": "console", "original_sha256": sha256(b"old")}])
                }
                "dot" => manifest["files"][0]["path"] = serde_json::json!("app/./_app.py"),
                "slash" => manifest["files"][0]["path"] = serde_json::json!("app//_app.py"),
                "backslash" => manifest["files"][0]["path"] = serde_json::json!("app/..\\escape"),
                _ => manifest["files"][0]["path"] = serde_json::json!("app/bad\0.py"),
            });

            assert!(
                BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into())).is_err(),
                "{problem}"
            );
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn accepts_uppercase_digest_without_weakening_hash_comparison() {
        let (root, venv, package) = fixture("uppercase-hash");
        fs::write(package.join("app/_app.py"), b"official").unwrap();
        write_manifest(&root, b"official", b"patched");
        edit_manifest(&root, |manifest| {
            for field in ["sha256", "original_sha256"] {
                manifest["files"][0][field] =
                    serde_json::json!(manifest["files"][0][field].as_str().unwrap().to_uppercase());
            }
        });

        load_patch(&root).apply(&venv).unwrap();

        assert_eq!(fs::read(package.join("app/_app.py")).unwrap(), b"patched");
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn rejects_symlinks_anywhere_in_the_payload_directory_chain() {
        use std::os::unix::fs::symlink;
        for relative in ["payload", "payload/app", "payload/app/_app.py"] {
            let (root, _, _) = fixture(&format!("source-link-{}", relative.replace('/', "-")));
            write_manifest(&root, b"official", b"patched");
            let path = root.join(relative);
            let outside = root.parent().unwrap().join("outside");
            fs::rename(&path, &outside).unwrap();
            symlink(&outside, &path).unwrap();

            assert!(
                BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into())).is_err(),
                "{relative}"
            );
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn rejects_symlinked_target_directories_and_files_without_touching_the_destination() {
        use std::os::unix::fs::symlink;
        for relative in [
            "lib",
            "lib/python3.12",
            "lib/python3.12/site-packages",
            "lib/python3.12/site-packages/qwenpaw",
            "lib/python3.12/site-packages/qwenpaw/console",
            "lib/python3.12/site-packages/qwenpaw/console/assets",
            "lib/python3.12/site-packages/qwenpaw/console/assets/old.js",
        ] {
            let (root, staged, package) =
                staging_fixture(&format!("target-link-{}", relative.replace('/', "-")));
            write_console_manifest(&root);
            write_official_console(&package);
            let path = staged.join(relative);
            let outside = staged.parent().unwrap().join("outside");
            fs::rename(&path, &outside).unwrap();
            symlink(&outside, &path).unwrap();

            assert!(load_patch(&root).apply(&staged).is_err(), "{relative}");
            assert_eq!(fs::read(package.join("app/_app.py")).unwrap(), b"official");
            assert_eq!(
                fs::read(package.join("console/assets/old.js")).unwrap(),
                b"old-js"
            );
            assert!(!staged.join(MARKER_FILE).exists());
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn applies_only_to_exact_2_3_0_and_records_the_manifest_fingerprint() {
        let (root, venv, package) = fixture("apply");
        std::fs::write(package.join("app/_app.py"), b"official").unwrap();
        write_manifest(&root, b"official", b"patched");

        let patch = BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into()))
            .unwrap()
            .unwrap();
        assert!(BasePathPatch::from_root(&root, &InstallChannel::Latest)
            .unwrap()
            .is_none());
        assert!(patch.needs_apply(&venv).unwrap());
        patch.apply(&venv).unwrap();
        assert_eq!(
            std::fs::read(package.join("app/_app.py")).unwrap(),
            b"patched"
        );
        assert!(!patch.needs_apply(&venv).unwrap());
        assert!(active(&venv));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn creates_missing_console_parent_directories() {
        let (root, venv, package) = fixture("new-console-directory");
        std::fs::create_dir_all(root.join("payload/console/.vite")).unwrap();
        std::fs::write(root.join("payload/console/.vite/manifest.json"), b"{}").unwrap();
        let manifest = serde_json::json!({
            "schema_version": 1,
            "base_version": "2.3.0",
            "files": [{
                "path": "console/.vite/manifest.json",
                "original_sha256": null,
                "sha256": sha256(b"{}"),
            }]
        });
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let patch = BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into()))
            .unwrap()
            .unwrap();

        patch.apply(&venv).unwrap();

        assert_eq!(
            std::fs::read(package.join("console/.vite/manifest.json")).unwrap(),
            b"{}"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn rejects_a_modified_official_file_before_overwriting_anything() {
        let (root, venv, package) = fixture("modified");
        std::fs::write(package.join("app/_app.py"), b"locally modified").unwrap();
        write_manifest(&root, b"official", b"patched");
        let patch = BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into()))
            .unwrap()
            .unwrap();

        let result = patch.apply(&venv);
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(package.join("app/_app.py")).unwrap(),
            b"locally modified"
        );
        assert!(!venv.join(MARKER_FILE).exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn accepts_a_real_sized_manifest_beyond_the_legacy_128_kib_limit() {
        let (root, _venv, _package) = fixture("manifest-size");
        write_manifest(&root, b"official", b"patched");
        let path = root.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["padding"] = serde_json::Value::String("x".repeat(256 * 1024));
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

        assert!(
            BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into()))
                .unwrap()
                .is_some()
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn rejects_manifest_paths_outside_the_python_package() {
        let (root, venv, _package) = fixture("traversal");
        std::fs::write(root.join("payload/escape"), b"bad").unwrap();
        let manifest = serde_json::json!({
            "schema_version": 1,
            "base_version": "2.3.0",
            "files": [{"path": "../escape", "original_sha256": null, "sha256": sha256(b"bad")}]
        });
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        assert!(BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into())).is_err());
        assert!(!venv.join(MARKER_FILE).exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn accepts_an_explicitly_declared_previous_patch_hash() {
        let (root, venv, package) = fixture("previous-patched");
        std::fs::write(package.join("app/_app.py"), b"previous-patched").unwrap();
        write_manifest(&root, b"official", b"patched");
        let path = root.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["files"][0]["previous_sha256"] = serde_json::json!([sha256(b"previous-patched")]);
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        let patch = BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into()))
            .unwrap()
            .unwrap();

        patch.apply(&venv).unwrap();
        assert_eq!(
            std::fs::read(package.join("app/_app.py")).unwrap(),
            b"patched"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn identifies_a_complete_prebuilt_console_payload() {
        let (root, _venv, _package) = fixture("prebuilt-console");
        std::fs::create_dir_all(root.join("payload/console")).unwrap();
        std::fs::write(root.join("payload/console/index.html"), b"patched").unwrap();
        write_manifest(&root, b"official", b"patched");
        let path = root.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["files"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "path": "console/index.html",
                "original_sha256": sha256(b"official"),
                "sha256": sha256(b"patched"),
            }));
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        let patch = BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into()))
            .unwrap()
            .unwrap();

        assert!(patch.includes_console());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn post1_uses_its_own_manifest_and_prepared_transaction() {
        let (root, staged, package) = staging_fixture("post1");
        write_console_manifest(&root);
        write_official_console(&package);
        edit_manifest(&root, |manifest| {
            manifest["base_version"] = serde_json::json!("2.3.0.post1");
        });
        let journal = staged
            .parent()
            .unwrap()
            .join(".venv.transaction.123.1.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
        record["target_version"] = serde_json::json!("2.3.0.post1");
        fs::write(journal, serde_json::to_vec(&record).unwrap()).unwrap();

        assert!(BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0".into())).is_err());
        let patch = BasePathPatch::from_root(&root, &InstallChannel::Exact("2.3.0.post1".into()))
            .unwrap()
            .unwrap();
        patch.apply(&staged).unwrap();

        assert_eq!(
            fs::read(package.join("console/index.html")).unwrap(),
            b"new-index"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
