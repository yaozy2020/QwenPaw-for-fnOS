use super::InstallChannel;
use ring::digest::{digest, SHA256};
use serde::Deserialize;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

const MARKER_FILE: &str = ".qwenpaw-base-path-patch";
const PATCH_DIRECTORY: &str = "www/patches/qwenpaw-2.3.0-base-path";
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAYLOAD_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize)]
struct Manifest {
    schema_version: u32,
    base_version: String,
    files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
struct ManifestFile {
    path: String,
    original_sha256: Option<String>,
    sha256: String,
    #[serde(default)]
    previous_sha256: Vec<String>,
}

pub(crate) struct BasePathPatch {
    root: PathBuf,
    fingerprint: String,
    files: Vec<ManifestFile>,
}

pub(crate) fn for_channel(channel: &InstallChannel) -> Result<Option<BasePathPatch>, String> {
    let explicit = std::env::var_os("QWENPAW_BASE_PATH_PATCH").map(PathBuf::from);
    let root = explicit.clone().unwrap_or_else(|| {
        let appdest = std::env::var_os("TRIM_APPDEST")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/apps/qwenpaw/target"));
        appdest.join(PATCH_DIRECTORY)
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
        if channel != &InstallChannel::Exact("2.3.0".into()) {
            return Ok(None);
        }
        let manifest_path = root.join("manifest.json");
        let raw = read_regular(&manifest_path, MAX_MANIFEST_BYTES)?;
        let fingerprint = sha256(&raw);
        let manifest: Manifest = serde_json::from_slice(&raw)
            .map_err(|error| format!("base-path 补丁清单不是有效 JSON: {error}"))?;
        if manifest.schema_version != 1 || manifest.base_version != "2.3.0" {
            return Err("base-path 补丁清单版本不兼容".into());
        }
        if manifest.files.is_empty() || manifest.files.len() > 4096 {
            return Err("base-path 补丁清单文件数量无效".into());
        }
        for file in &manifest.files {
            relative_payload_path(&file.path)?;
            validate_digest(&file.sha256)?;
            if let Some(original) = &file.original_sha256 {
                validate_digest(original)?;
            }
            for previous in &file.previous_sha256 {
                validate_digest(previous)?;
            }
            read_regular(
                &root.join("payload").join(&file.path),
                MAX_PAYLOAD_FILE_BYTES,
            )?;
        }
        Ok(Some(Self {
            root: root.to_path_buf(),
            fingerprint,
            files: manifest.files,
        }))
    }

    pub(crate) fn needs_apply(&self, venv: &Path) -> Result<bool, String> {
        let marker = venv.join(MARKER_FILE);
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err("base-path 补丁指纹路径不是普通文件".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(format!("检查 base-path 补丁指纹失败: {error}")),
        }
        let current = fs::read_to_string(&marker)
            .map_err(|error| format!("读取 base-path 补丁指纹失败: {error}"))?;
        Ok(current.trim() != self.fingerprint)
    }

    pub(crate) fn includes_console(&self) -> bool {
        self.files
            .iter()
            .any(|file| file.path.starts_with("console/"))
    }

    pub(crate) fn apply(&self, venv: &Path) -> Result<(), String> {
        let package = python_package(venv)?;
        for file in &self.files {
            let relative = relative_payload_path(&file.path)?;
            let source = self.root.join("payload").join(relative);
            let patched = read_regular(&source, MAX_PAYLOAD_FILE_BYTES)?;
            if sha256(&patched) != file.sha256 {
                return Err(format!("base-path 补丁文件哈希不匹配: {}", file.path));
            }
            let target = package.join(relative);
            ensure_parent_directories(&package, relative)?;
            match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    let current = read_regular(&target, MAX_PAYLOAD_FILE_BYTES)?;
                    let current_hash = sha256(&current);
                    if current_hash != file.sha256
                        && file.original_sha256.as_ref() != Some(&current_hash)
                        && !file.previous_sha256.contains(&current_hash)
                    {
                        return Err(format!("官方内核文件已被修改，拒绝覆盖: {}", file.path));
                    }
                }
                Ok(_) => return Err(format!("内核补丁目标不是普通文件: {}", file.path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if file.original_sha256.is_some() {
                        return Err(format!("官方内核文件缺失: {}", file.path));
                    }
                }
                Err(error) => return Err(format!("检查内核补丁目标失败: {error}")),
            }
        }
        for file in &self.files {
            let relative = relative_payload_path(&file.path)?;
            let target = package.join(relative);
            fs::copy(self.root.join("payload").join(relative), &target)
                .map_err(|error| format!("写入内核补丁 {} 失败: {error}", file.path))?;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o644))
                .map_err(|error| format!("设置内核补丁权限 {} 失败: {error}", file.path))?;
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

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("base-path 补丁清单包含无效 SHA256".into())
    }
}

fn relative_payload_path(value: &str) -> Result<&Path, String> {
    let path = Path::new(value);
    let mut components = path.components();
    let valid_root = matches!(components.next(), Some(Component::Normal(root)) if root == "app" || root == "console");
    if !valid_root
        || value.contains('\\')
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("base-path 补丁路径越界: {value}"));
    }
    Ok(path)
}

fn read_regular(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法检查普通文件 {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("不是普通文件: {}", path.display()));
    }
    if metadata.len() > maximum as u64 {
        return Err(format!("文件超过大小限制: {}", path.display()));
    }
    fs::read(path).map_err(|error| format!("读取文件 {} 失败: {error}", path.display()))
}

pub(crate) fn python_package(venv: &Path) -> Result<PathBuf, String> {
    let mut packages = Vec::new();
    for entry in fs::read_dir(venv.join("lib"))
        .map_err(|error| format!("读取 venv lib 目录失败: {error}"))?
    {
        let entry = entry.map_err(|error| format!("读取 venv lib 条目失败: {error}"))?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("python3.") {
            continue;
        }
        let package = entry.path().join("site-packages/qwenpaw");
        if fs::symlink_metadata(&package)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false)
        {
            packages.push(package);
        }
    }
    if packages.len() == 1 {
        Ok(packages.remove(0))
    } else {
        Err("无法唯一确定 venv 内的 qwenpaw 包目录".into())
    }
}

fn ensure_parent_directories(package: &Path, relative: &Path) -> Result<(), String> {
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
                fs::create_dir(&current)
                    .map_err(|error| format!("创建内核补丁父目录失败 {}: {error}", current.display()))?;
            }
            Err(error) => {
                return Err(format!("检查内核补丁父目录失败 {}: {error}", current.display()));
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
        manifest["files"][0]["previous_sha256"] =
            serde_json::json!([sha256(b"previous-patched")]);
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
        manifest["files"].as_array_mut().unwrap().push(serde_json::json!({
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
}
