//! Explicit reset intent distinguishes rollback from committed backup cleanup.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    committed: bool,
}

pub(crate) struct AuthReset {
    auth: PathBuf,
    backup: PathBuf,
    journal: PathBuf,
}

fn regular_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(io::Error::other("认证恢复路径不是普通文件")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

impl AuthReset {
    pub(crate) fn new(auth: &Path, var: &Path) -> Self {
        Self {
            auth: auth.into(),
            backup: auth.with_extension("json.bak-reset"),
            journal: var.join("auth-reset.json"),
        }
    }

    fn sync_auth(&self) -> io::Result<()> {
        fs::File::open(
            self.auth
                .parent()
                .ok_or_else(|| io::Error::other("认证目录缺失"))?,
        )?
        .sync_all()
    }

    fn write(&self, committed: bool) -> io::Result<()> {
        crate::install::atomic_write_0600(
            &self.journal.to_string_lossy(),
            &serde_json::to_string(&Record {
                schema: 1,
                committed,
            })?,
        )
    }

    fn clear(&self) -> io::Result<()> {
        fs::remove_file(&self.journal)?;
        fs::File::open(
            self.journal
                .parent()
                .ok_or_else(|| io::Error::other("事务目录缺失"))?,
        )?
        .sync_all()
    }

    pub(crate) fn pending(&self) -> io::Result<bool> {
        Ok(regular_exists(&self.journal)? || regular_exists(&self.backup)?)
    }

    pub(crate) fn begin(&self) -> io::Result<bool> {
        if self.pending()? {
            return Err(io::Error::other("认证重置尚待恢复"));
        }
        if !regular_exists(&self.auth)? {
            return Ok(false);
        }
        self.write(false)?;
        fs::rename(&self.auth, &self.backup)?;
        self.sync_auth()?;
        Ok(true)
    }

    pub(crate) fn commit(&self) -> io::Result<()> {
        self.write(true)?;
        self.recover()
    }

    pub(crate) fn recover(&self) -> io::Result<()> {
        let has_journal = regular_exists(&self.journal)?;
        let has_backup = regular_exists(&self.backup)?;
        let has_auth = regular_exists(&self.auth)?;
        if !has_journal {
            if has_backup {
                if has_auth {
                    return Err(io::Error::other("旧认证与重置备份同时存在，保留文件待核对"));
                }
                fs::rename(&self.backup, &self.auth)?;
                self.sync_auth()?;
            }
            return Ok(());
        }
        let record: Record = serde_json::from_slice(&fs::read(&self.journal)?)?;
        if record.schema != 1 {
            return Err(io::Error::other("不支持的认证重置记录"));
        }
        if record.committed {
            if has_backup {
                fs::remove_file(&self.backup)?;
            }
        } else if has_backup {
            // The prepared record proves this backup is authoritative, including
            // when the restarted kernel created a replacement auth file.
            fs::rename(&self.backup, &self.auth)?;
        } else if !has_auth {
            return Err(io::Error::other("认证重置恢复所需的原文件与备份均缺失"));
        }
        self.sync_auth()?;
        self.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;

    #[test]
    fn pending_reset_restores_original_even_if_new_auth_was_created() {
        let dir = TestDir::new("auth-reset-pending");
        let auth = dir.0.join("auth.json");
        fs::write(&auth, "old-secret").unwrap();
        let tx = AuthReset::new(&auth, &dir.0);
        assert!(tx.begin().unwrap());
        fs::write(&auth, "new-secret").unwrap();
        tx.recover().unwrap();
        tx.recover().unwrap();
        assert_eq!(fs::read_to_string(auth).unwrap(), "old-secret");
        assert!(!tx.pending().unwrap());
    }

    #[test]
    fn committed_reset_never_restores_old_credentials() {
        let dir = TestDir::new("auth-reset-committed");
        let auth = dir.0.join("auth.json");
        fs::write(&auth, "old-secret").unwrap();
        let tx = AuthReset::new(&auth, &dir.0);
        tx.begin().unwrap();
        tx.write(true).unwrap(); // crash before cleanup
        fs::write(&auth, "new-secret").unwrap();
        tx.recover().unwrap();
        assert_eq!(fs::read_to_string(auth).unwrap(), "new-secret");
        assert!(!tx.pending().unwrap());
    }

    #[test]
    fn ambiguous_legacy_or_corrupt_journal_preserves_both_credentials() {
        let dir = TestDir::new("auth-reset-ambiguous");
        let auth = dir.0.join("auth.json");
        let tx = AuthReset::new(&auth, &dir.0);
        fs::write(&auth, "new-secret").unwrap();
        fs::write(&tx.backup, "old-secret").unwrap();
        assert!(tx.recover().is_err());
        fs::write(&tx.journal, "broken").unwrap();
        assert!(tx.recover().is_err());
        assert_eq!(fs::read_to_string(&tx.backup).unwrap(), "old-secret");
        assert_eq!(fs::read_to_string(&auth).unwrap(), "new-secret");
    }
}
