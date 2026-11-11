//! Kernel removal has its own durable decision; installation journals do not
//! describe whether an uninstall backup is still needed for rollback.
use super::{atomic_write_0600, sync_dir};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const JOURNAL: &str = "kernel-uninstall.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    id: String,
    committed: bool,
    had_runtime: bool,
    had_pin: bool,
}

#[derive(Debug)]
pub(crate) enum RemovalError {
    BeforeRemoval(String),
    PartialRemoval(String),
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
        && id.split('.').all(|part| !part.is_empty())
}

fn ensure_kind(path: &Path, directory: bool) -> io::Result<()> {
    if exists(path)? {
        let kind = fs::symlink_metadata(path)?.file_type();
        if (directory && !kind.is_dir()) || (!directory && !kind.is_file()) {
            return Err(io::Error::other(format!(
                "unexpected uninstall file type: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn paths(home: &Path, var: &Path, id: &str) -> (PathBuf, PathBuf) {
    (
        home.join(format!(".venv.uninstall.{id}")),
        var.join(format!(".qwenpaw-version.uninstall.{id}")),
    )
}

fn write(var: &Path, record: &Record) -> io::Result<()> {
    atomic_write_0600(
        &var.join(JOURNAL).to_string_lossy(),
        &serde_json::to_string(record)?,
    )
}

fn clear(var: &Path) -> io::Result<()> {
    fs::remove_file(var.join(JOURNAL))?;
    sync_dir(var)
}

fn restore(active: &Path, backup: &Path, had_file: bool) -> io::Result<()> {
    if exists(backup)? {
        if exists(active)? {
            return Err(io::Error::other(
                "uninstall rollback has both active and backup; preserving both",
            ));
        }
        fs::rename(backup, active)?;
    } else if had_file != exists(active)? {
        return Err(io::Error::other(
            "uninstall rollback is missing its original file",
        ));
    }
    Ok(())
}

fn recover_record(home: &Path, var: &Path, record: &Record) -> io::Result<()> {
    if record.schema != 1 || !valid_id(&record.id) {
        return Err(io::Error::other("invalid kernel uninstall journal"));
    }
    let active = home.join("venv");
    let pin = var.join("qwenpaw-version");
    let (trash, backup) = paths(home, var, &record.id);
    for path in [&active, &trash] {
        ensure_kind(path, true)?;
    }
    for path in [&pin, &backup] {
        ensure_kind(path, false)?;
    }
    if record.committed {
        if exists(&active)? || exists(&pin)? {
            return Err(io::Error::other(
                "committed uninstall unexpectedly has active files",
            ));
        }
        if exists(&trash)? {
            fs::remove_dir_all(trash)?;
        }
        if exists(&backup)? {
            fs::remove_file(backup)?;
        }
    } else {
        restore(&active, &trash, record.had_runtime)?;
        restore(&pin, &backup, record.had_pin)?;
    }
    sync_dir(home)?;
    sync_dir(var)?;
    clear(var)
}

pub(crate) fn pending(home: &Path, var: &Path) -> io::Result<bool> {
    Ok(exists(&var.join(JOURNAL))? || !legacy_ids(home, var)?.is_empty())
}

fn legacy_ids(home: &Path, var: &Path) -> io::Result<Vec<String>> {
    let mut ids = Vec::new();
    for (root, prefix) in [
        (home, ".venv.uninstall."),
        (var, ".qwenpaw-version.uninstall."),
    ] {
        if !root.try_exists()? {
            continue;
        }
        for entry in fs::read_dir(root)? {
            let name = entry?.file_name();
            if let Some(id) = name.to_string_lossy().strip_prefix(prefix) {
                if !valid_id(id) {
                    return Err(io::Error::other("invalid legacy uninstall backup name"));
                }
                ids.push(id.to_owned());
            }
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

pub(crate) fn recover(home: &Path, var: &Path) -> io::Result<()> {
    let journal = var.join(JOURNAL);
    ensure_kind(&journal, false)?;
    if exists(&journal)? {
        let record: Record = serde_json::from_slice(&fs::read(&journal)?)?;
        recover_record(home, var, &record)?;
    }
    let ids = legacy_ids(home, var)?;
    if ids.len() > 1 {
        return Err(io::Error::other(
            "multiple legacy uninstall generations; preserving evidence",
        ));
    }
    if let Some(id) = ids.first() {
        let (trash, backup) = paths(home, var, id);
        ensure_kind(&trash, true)?;
        ensure_kind(&backup, false)?;
        let active = home.join("venv");
        let pin = var.join("qwenpaw-version");
        ensure_kind(&active, true)?;
        ensure_kind(&pin, false)?;
        if exists(&active)? {
            if exists(&trash)? || exists(&pin)? || !exists(&backup)? {
                return Err(io::Error::other(
                    "ambiguous legacy uninstall; preserving evidence",
                ));
            }
            fs::rename(backup, pin)?;
        } else {
            if exists(&pin)? {
                return Err(io::Error::other(
                    "legacy uninstall has an unexpected active pin",
                ));
            }
            if exists(&trash)? {
                fs::remove_dir_all(trash)?;
            }
            if exists(&backup)? {
                fs::remove_file(backup)?;
            }
        }
        sync_dir(home)?;
        sync_dir(var)?;
    }
    Ok(())
}

pub(crate) fn remove(home: &Path, var: &Path) -> Result<Vec<String>, RemovalError> {
    let prepare = || -> io::Result<Record> {
        if pending(home, var)? {
            return Err(io::Error::other("kernel uninstall recovery is pending"));
        }
        let active = home.join("venv");
        let pin = var.join("qwenpaw-version");
        ensure_kind(&active, true)?;
        ensure_kind(&pin, false)?;
        let record = Record {
            schema: 1,
            id: format!(
                "{}.{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(io::Error::other)?
                    .as_nanos()
            ),
            committed: false,
            had_runtime: exists(&active)?,
            had_pin: exists(&pin)?,
        };
        write(var, &record)?;
        Ok(record)
    };
    let mut record = prepare().map_err(|error| {
        if pending(home, var).unwrap_or(true) {
            RemovalError::PartialRemoval(format!("卸载准备状态待恢复: {error}"))
        } else {
            RemovalError::BeforeRemoval(error.to_string())
        }
    })?;
    let (trash, backup) = paths(home, var, &record.id);
    let move_files = || -> io::Result<()> {
        if record.had_pin {
            fs::rename(var.join("qwenpaw-version"), &backup)?;
            sync_dir(var)?;
        }
        if record.had_runtime {
            fs::rename(home.join("venv"), &trash)?;
            sync_dir(home)?;
        }
        Ok(())
    };
    if let Err(error) = move_files() {
        return match recover_record(home, var, &record) {
            Ok(()) => Err(RemovalError::BeforeRemoval(format!(
                "卸载未提交，已恢复原环境: {error}"
            ))),
            Err(restore) => Err(RemovalError::PartialRemoval(format!(
                "卸载恢复失败: {error}; {restore}"
            ))),
        };
    }
    record.committed = true;
    // If write/fsync fails after rename, leave the on-disk decision for recovery;
    // never guess that a commit was not persisted and roll it back.
    write(var, &record)
        .map_err(|error| RemovalError::PartialRemoval(format!("卸载提交状态待恢复: {error}")))?;
    recover_record(home, var, &record).map_err(|error| {
        RemovalError::PartialRemoval(format!("内核已卸载，清理待恢复: {error}"))
    })?;
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::TestDir;

    #[test]
    fn recovers_every_uncommitted_removal_boundary_and_is_idempotent() {
        for boundary in 0..=2 {
            let dir = TestDir::new(&format!("removal-boundary-{boundary}"));
            let home = dir.0.join("home");
            let var = dir.0.join("var");
            fs::create_dir_all(home.join("venv")).unwrap();
            fs::create_dir_all(&var).unwrap();
            fs::write(home.join("venv/marker"), "old-runtime").unwrap();
            fs::write(var.join("qwenpaw-version"), "2.3.0b2\n").unwrap();
            let record = Record {
                schema: 1,
                id: "12.34".into(),
                committed: false,
                had_runtime: true,
                had_pin: true,
            };
            write(&var, &record).unwrap();
            let (trash, backup) = paths(&home, &var, &record.id);
            if boundary >= 1 {
                fs::rename(var.join("qwenpaw-version"), backup).unwrap();
            }
            if boundary >= 2 {
                fs::rename(home.join("venv"), trash).unwrap();
            }
            recover(&home, &var).unwrap();
            recover(&home, &var).unwrap();
            assert_eq!(
                fs::read_to_string(home.join("venv/marker")).unwrap(),
                "old-runtime"
            );
            assert_eq!(
                fs::read_to_string(var.join("qwenpaw-version")).unwrap(),
                "2.3.0b2\n"
            );
            assert!(!pending(&home, &var).unwrap());
        }
    }

    #[test]
    fn legacy_pin_move_restores_pin_instead_of_deleting_it() {
        let dir = TestDir::new("legacy-removal-pin");
        let home = dir.0.join("home");
        let var = dir.0.join("var");
        fs::create_dir_all(home.join("venv")).unwrap();
        fs::create_dir_all(&var).unwrap();
        fs::write(var.join(".qwenpaw-version.uninstall.12.34"), "2.0.0\n").unwrap();
        recover(&home, &var).unwrap();
        assert_eq!(
            fs::read_to_string(var.join("qwenpaw-version")).unwrap(),
            "2.0.0\n"
        );
    }

    #[test]
    fn committed_removal_finishes_and_corrupt_journal_preserves_backups() {
        let dir = TestDir::new("removal-commit");
        let home = dir.0.join("home");
        let var = dir.0.join("var");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&var).unwrap();
        let record = Record {
            schema: 1,
            id: "12.34".into(),
            committed: true,
            had_runtime: true,
            had_pin: true,
        };
        let (trash, backup) = paths(&home, &var, &record.id);
        fs::create_dir(&trash).unwrap();
        fs::write(&backup, "2.0.0\n").unwrap();
        fs::write(var.join(JOURNAL), "invalid").unwrap();
        assert!(recover(&home, &var).is_err());
        assert!(trash.exists() && backup.exists());
        write(&var, &record).unwrap();
        recover(&home, &var).unwrap();
        assert!(!trash.exists() && !backup.exists());
    }
}
