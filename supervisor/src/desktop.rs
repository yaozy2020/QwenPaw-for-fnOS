use serde_json::json;

fn write_ui_config(path: &std::path::Path, config: &serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    static UI_TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut bytes = serde_json::to_vec_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    bytes.push(b'\n');
    let (tmp, mut file) = loop {
        let tmp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            UI_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        file.write_all(&bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn read_ui_config(path: &std::path::Path) -> std::io::Result<serde_json::Value> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn sync_main_entry(path: &std::path::Path, port: u16, installed: bool) -> std::io::Result<()> {
    let mut config = read_ui_config(path)?;
    let urls = config
        .get_mut(".url")
        .and_then(|value| value.as_object_mut())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing .url"))?;
    let entry_matches = urls
        .get("qwenpaw.main")
        .map(|entry| {
            entry["port"].as_str() == Some(&port.to_string())
                && entry["noDisplay"].as_bool() == Some(!installed)
        })
        .unwrap_or(false);
    if entry_matches {
        return Ok(());
    }
    let entry = urls.entry("qwenpaw.main").or_insert_with(|| {
        json!({
            "title": "QwenPaw",
            "icon": "images/icon_{0}.png",
            "type": "url",
            "protocol": "http",
            "port": port.to_string(),
            "url": "/",
            "allUsers": false,
            "control": {"accessPerm": "editable"}
        })
    });
    // 存量配置可能把该键写成字符串/数组等非 object 值,
    // 直接 entry["port"] 赋值会 panic(serde_json IndexMut 仅对 object 有效)
    if !entry.is_object() {
        *entry = serde_json::json!({});
    }
    entry["port"] = serde_json::Value::String(port.to_string());
    entry["noDisplay"] = serde_json::Value::Bool(!installed);
    write_ui_config(path, &config)
}

#[cfg(test)]
fn sync_ui_port(path: &std::path::Path, port: u16) -> std::io::Result<()> {
    let mut config = read_ui_config(path)?;
    if let Some(entry) = config.pointer_mut("/.url/qwenpaw.main") {
        // 同上:非 object 时赋值会 panic
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        entry["port"] = serde_json::Value::String(port.to_string());
        write_ui_config(path, &config)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;

    #[test]
    fn ui_config_commit_failure_removes_temporary_file() {
        let dir = TestDir::new("ui-commit-failure");
        let path = dir.0.join("config");
        std::fs::create_dir(&path).unwrap();

        assert!(write_ui_config(&path, &json!({"custom": "keep"})).is_err());

        let entries: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config")]);
    }

    #[test]
    fn ui_config_write_preserves_other_keys_and_0644_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("ui-permissions");
        let path = dir.0.join("config");
        std::fs::write(
            &path,
            r#"{".url":{"qwenpaw.main":{"port":"18088","title":"custom"}},"custom":{"nested":42}}"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        sync_main_entry(&path, 18099, true).unwrap();

        let config = read_ui_config(&path).unwrap();
        assert_eq!(config["custom"]["nested"], 42);
        assert_eq!(config[".url"]["qwenpaw.main"]["title"], "custom");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn ui_main_entry_follows_kernel_installation() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-ui-entry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, r#"{".url":{"qwenpaw.console":{"type":"iframe"}}}"#).unwrap();
        sync_main_entry(&path, 18088, false).unwrap();
        sync_main_entry(&path, 18088, true).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["port"], "18088");
        assert_eq!(value[".url"]["qwenpaw.main"]["noDisplay"], false);
        assert_eq!(value[".url"]["qwenpaw.console"]["type"], "iframe");
        sync_main_entry(&path, 18088, false).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["noDisplay"], true);
        sync_ui_port(&path, 18099).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["port"], "18099");
        assert_eq!(value[".url"]["qwenpaw.main"]["noDisplay"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_port_sync_updates_only_the_main_entry() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-ui-port-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(
            &path,
            r#"{".url":{"qwenpaw.console":{"type":"iframe"},"qwenpaw.main":{"type":"url","port":"18088"}}}"#,
        )
        .unwrap();
        sync_ui_port(&path, 18099).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value[".url"]["qwenpaw.main"]["port"], "18099");
        assert_eq!(value[".url"]["qwenpaw.console"]["type"], "iframe");
        assert!(!dir.join("config.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
