use std::{fs, io, path::Path};

use crate::{install::atomic_write_0600, install::validate_mirror};

#[derive(Debug)]
pub enum MirrorError {
    Invalid(String),
    Io(io::Error),
}

pub fn read(var_dir: &str) -> io::Result<String> {
    let path = Path::new(var_dir).join("pip-mirror");
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error),
    };
    Ok(contents
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r')
        .to_string())
}

pub fn write(var_dir: &str, value: &str) -> Result<(), MirrorError> {
    validate_mirror(value).map_err(MirrorError::Invalid)?;
    let path = Path::new(var_dir).join("pip-mirror");
    let path = path.to_string_lossy();
    atomic_write_0600(&path, &format!("{value}\n")).map_err(MirrorError::Io)
}

pub fn get_json(var_dir: &str) -> (i32, String) {
    match read(var_dir) {
        Ok(value) => (
            0,
            serde_json::json!({"success": true, "mirror": value}).to_string(),
        ),
        Err(error) => (
            1,
            serde_json::json!({
                "success": false,
                "mirror": "",
                "message": format!("下载源读取失败: {error}")
            })
            .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::MetadataExt, time::SystemTime};

    #[test]
    fn get_json_escapes_legacy_file_contents_without_mutating_them() {
        let root = std::env::temp_dir().join(format!("qwenpaw-mirror-get-{}", std::process::id()));
        let var = root.join("var");
        fs::create_dir_all(&var).unwrap();
        let path = var.join("pip-mirror");
        let original = b"https://mirror.example/a\"b\\c\r\nignored\n";
        fs::write(&path, original).unwrap();
        let before = fs::metadata(&path).unwrap();
        let before_modified = before.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        let (status, json) = get_json(var.to_str().unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(status, 0);
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["mirror"], "https://mirror.example/a\"b\\c");
        let after = fs::metadata(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(before.ino(), after.ino());
        assert_eq!(
            before_modified,
            after.modified().unwrap_or(SystemTime::UNIX_EPOCH)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_mirror_reads_as_explicit_official_value_without_creating_a_file() {
        let root =
            std::env::temp_dir().join(format!("qwenpaw-mirror-missing-{}", std::process::id()));
        let var = root.join("var");
        fs::create_dir_all(&var).unwrap();

        let (status, json) = get_json(var.to_str().unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(status, 0);
        assert_eq!(parsed["mirror"], "");
        assert!(!var.join("pip-mirror").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
