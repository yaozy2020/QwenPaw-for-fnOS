use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(serde::Serialize, serde::Deserialize)]
struct FileImage {
    bytes: Option<Vec<u8>>,
    mode: u32,
}

impl FileImage {
    fn read(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Self {
                bytes: Some(bytes),
                mode: std::fs::metadata(path)?.permissions().mode() & 0o777,
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self {
                bytes: None,
                mode: 0o600,
            }),
            Err(e) => Err(e),
        }
    }

    fn restore(&self, path: &Path) -> io::Result<()> {
        match &self.bytes {
            Some(bytes) => {
                static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let tmp = path.with_extension(format!(
                    "tmp.{}.{}",
                    std::process::id(),
                    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                let result = (|| {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(self.mode)
                        .open(&tmp)?;
                    file.write_all(bytes)?;
                    file.set_permissions(std::fs::Permissions::from_mode(self.mode))?;
                    file.sync_all()?;
                    std::fs::rename(&tmp, path)?;
                    sync_parent(path)
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
                result
            }
            None => match std::fs::remove_file(path) {
                Ok(()) => sync_parent(path),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            },
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SettingsJournal {
    schema: u32,
    port: FileImage,
    log_level: FileImage,
    desktop: FileImage,
}

pub struct SettingsTransaction {
    var_dir: PathBuf,
    ui_config: PathBuf,
    journal: SettingsJournal,
}

fn sync_parent(path: &Path) -> io::Result<()> {
    std::fs::File::open(
        path.parent()
            .ok_or_else(|| io::Error::other("missing parent"))?,
    )?
    .sync_all()
}

impl SettingsTransaction {
    fn journal_path(var_dir: &Path) -> PathBuf {
        var_dir.join("settings-transaction.json")
    }

    pub fn begin(var_dir: &Path, ui_config: &Path) -> io::Result<Self> {
        let path = Self::journal_path(var_dir);
        if path.try_exists()? {
            return Err(io::Error::other("配置事务尚待恢复"));
        }
        let journal = SettingsJournal {
            schema: 1,
            port: FileImage::read(&var_dir.join("service_port"))?,
            log_level: FileImage::read(&var_dir.join("qwenpaw-log-level"))?,
            desktop: FileImage::read(ui_config)?,
        };
        let content = serde_json::to_string(&journal).map_err(io::Error::other)?;
        crate::install::atomic_write_0600(&path.to_string_lossy(), &content)?;
        sync_parent(&path)?;
        Ok(Self {
            var_dir: var_dir.into(),
            ui_config: ui_config.into(),
            journal,
        })
    }

    pub fn commit(&self) -> io::Result<()> {
        self.clear()
    }

    pub fn rollback(&self) -> io::Result<()> {
        let path = Self::journal_path(&self.var_dir);
        if !path.try_exists()? {
            let content = serde_json::to_string(&self.journal).map_err(io::Error::other)?;
            crate::install::atomic_write_0600(&path.to_string_lossy(), &content)?;
            sync_parent(&path)?;
        }
        let mut errors = Vec::new();
        for (image, path) in [
            (&self.journal.port, self.var_dir.join("service_port")),
            (
                &self.journal.log_level,
                self.var_dir.join("qwenpaw-log-level"),
            ),
            (&self.journal.desktop, self.ui_config.clone()),
        ] {
            if let Err(e) = image.restore(&path) {
                errors.push(format!("{}: {e}", path.display()));
            }
        }
        if errors.is_empty() {
            self.clear()
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }

    fn clear(&self) -> io::Result<()> {
        let path = Self::journal_path(&self.var_dir);
        std::fs::remove_file(&path)?;
        sync_parent(&path)
    }
}

pub fn recover(var_dir: &Path, ui_config: &Path) -> io::Result<bool> {
    let path = SettingsTransaction::journal_path(var_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let journal: SettingsJournal = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if journal.schema != 1 {
        return Err(io::Error::other("不支持的配置事务版本"));
    }
    SettingsTransaction {
        var_dir: var_dir.into(),
        ui_config: ui_config.into(),
        journal,
    }
    .rollback()?;
    Ok(true)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WebProxyJournal {
    schema: u32,
    port: FileImage,
    public: FileImage,
}

const WEB_PROXY_JOURNAL: &str = "web-proxy-transaction.json";

pub(crate) fn recover_web_proxy(var: &Path) -> io::Result<bool> {
    let path = var.join(WEB_PROXY_JOURNAL);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let journal: WebProxyJournal = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if journal.schema != 1 {
        return Err(io::Error::other("不支持的反代配置事务版本"));
    }
    journal.port.restore(&var.join("web_proxy_port"))?;
    journal.public.restore(&var.join("web_proxy_public"))?;
    std::fs::remove_file(&path)?;
    sync_parent(&path)?;
    Ok(true)
}

pub(crate) fn persist_web_proxy(var: &Path, port: u16, public: bool) -> io::Result<()> {
    let path = var.join(WEB_PROXY_JOURNAL);
    if path.try_exists()? {
        return Err(io::Error::other("反代配置事务尚待恢复"));
    }
    let journal = WebProxyJournal {
        schema: 1,
        port: FileImage::read(&var.join("web_proxy_port"))?,
        public: FileImage::read(&var.join("web_proxy_public"))?,
    };
    let content = serde_json::to_string(&journal).map_err(io::Error::other)?;
    crate::install::atomic_write_0600(&path.to_string_lossy(), &content)?;
    let result = (|| {
        crate::install::atomic_write_0600(
            &var.join("web_proxy_port").to_string_lossy(),
            &format!("{port}\n"),
        )?;
        crate::install::atomic_write_0600(
            &var.join("web_proxy_public").to_string_lossy(),
            if public { "1\n" } else { "0\n" },
        )?;
        std::fs::remove_file(&path)?;
        sync_parent(&path)
    })();
    if let Err(error) = result {
        // A directory-sync failure may have removed the journal already. Restore
        // the original decision before attempting rollback, as SettingsTransaction does.
        if !path.try_exists()? {
            crate::install::atomic_write_0600(&path.to_string_lossy(), &content)?;
        }
        return match recover_web_proxy(var) {
            Ok(_) => Err(error),
            Err(restore) => Err(io::Error::other(format!(
                "{error}; 反代配置恢复失败: {restore}"
            ))),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("qwenpaw-settings-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let ui = root.join("ui-config");
        std::fs::write(root.join("service_port"), "18088\n").unwrap();
        std::fs::write(root.join("qwenpaw-log-level"), "info\n").unwrap();
        std::fs::write(&ui, b"original desktop\n").unwrap();
        (root, ui)
    }

    #[test]
    fn interrupted_settings_restore_all_original_bytes() {
        let (root, ui) = fixture("recover");
        let transaction = SettingsTransaction::begin(&root, &ui).unwrap();
        std::fs::write(root.join("service_port"), "18099\n").unwrap();
        std::fs::write(root.join("qwenpaw-log-level"), "debug\n").unwrap();
        std::fs::write(&ui, "new desktop").unwrap();
        drop(transaction);
        assert!(recover(&root, &ui).unwrap());
        assert_eq!(
            std::fs::read(root.join("service_port")).unwrap(),
            b"18088\n"
        );
        assert_eq!(
            std::fs::read(root.join("qwenpaw-log-level")).unwrap(),
            b"info\n"
        );
        assert_eq!(std::fs::read(&ui).unwrap(), b"original desktop\n");
        assert!(!recover(&root, &ui).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_keeps_new_configuration_and_rejects_overlapping_transaction() {
        let (root, ui) = fixture("commit");
        let transaction = SettingsTransaction::begin(&root, &ui).unwrap();
        assert!(SettingsTransaction::begin(&root, &ui).is_err());
        std::fs::write(root.join("service_port"), "18099\n").unwrap();
        transaction.commit().unwrap();
        assert!(!recover(&root, &ui).unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join("service_port")).unwrap(),
            "18099\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rollback_restores_absence_and_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (root, ui) = fixture("absence");
        std::fs::remove_file(root.join("qwenpaw-log-level")).unwrap();
        std::fs::set_permissions(&ui, std::fs::Permissions::from_mode(0o640)).unwrap();
        let transaction = SettingsTransaction::begin(&root, &ui).unwrap();
        std::fs::write(root.join("qwenpaw-log-level"), "debug\n").unwrap();
        std::fs::set_permissions(&ui, std::fs::Permissions::from_mode(0o600)).unwrap();
        transaction.rollback().unwrap();
        assert!(!root.join("qwenpaw-log-level").exists());
        assert_eq!(
            std::fs::metadata(&ui).unwrap().permissions().mode() & 0o777,
            0o640
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_rollback_retains_journal_until_all_files_can_be_restored() {
        let (root, ui) = fixture("blocked");
        let transaction = SettingsTransaction::begin(&root, &ui).unwrap();
        std::fs::remove_file(&ui).unwrap();
        std::fs::create_dir(&ui).unwrap();
        assert!(transaction.rollback().is_err());
        assert!(root.join("settings-transaction.json").exists());
        assert!(recover(&root, &ui).is_err());
        std::fs::remove_dir(&ui).unwrap();
        assert!(recover(&root, &ui).unwrap());
        assert_eq!(std::fs::read(&ui).unwrap(), b"original desktop\n");
        assert!(!root.join("settings-transaction.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn web_proxy_recovery_restores_port_and_access_together() {
        let (root, _) = fixture("web-proxy-recovery");
        std::fs::write(root.join("web_proxy_port"), "43210\n").unwrap();
        std::fs::write(root.join("web_proxy_public"), "0\n").unwrap();
        let journal = WebProxyJournal {
            schema: 1,
            port: FileImage::read(&root.join("web_proxy_port")).unwrap(),
            public: FileImage::read(&root.join("web_proxy_public")).unwrap(),
        };
        std::fs::write(
            root.join(WEB_PROXY_JOURNAL),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("web_proxy_port"), "43211\n").unwrap();
        std::fs::write(root.join("web_proxy_public"), "1\n").unwrap();
        assert!(recover_web_proxy(&root).unwrap());
        assert!(!recover_web_proxy(&root).unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join("web_proxy_port")).unwrap(),
            "43210\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("web_proxy_public")).unwrap(),
            "0\n"
        );
        persist_web_proxy(&root, 43212, true).unwrap();
        assert!(!recover_web_proxy(&root).unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join("web_proxy_public")).unwrap(),
            "1\n"
        );
        std::fs::write(root.join(WEB_PROXY_JOURNAL), "broken").unwrap();
        assert!(recover_web_proxy(&root).is_err());
        assert!(persist_web_proxy(&root, 0, false).is_err());
        assert!(root.join(WEB_PROXY_JOURNAL).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_journal_is_preserved_and_blocks_reuse() {
        let (root, ui) = fixture("corrupt");
        std::fs::write(root.join("settings-transaction.json"), "broken").unwrap();
        assert!(recover(&root, &ui).is_err());
        assert!(SettingsTransaction::begin(&root, &ui).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("service_port")).unwrap(),
            "18088\n"
        );
        assert!(root.join("settings-transaction.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
