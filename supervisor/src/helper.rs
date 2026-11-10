use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const HELPER_NAME: &str = "qwenpaw-supervisor-helper";
const HELPER_MODE: u32 = 0o750;

pub fn path(var_dir: &str) -> PathBuf {
    Path::new(var_dir).join(HELPER_NAME)
}

pub fn stage_current_executable(var_dir: &str) -> Result<(), String> {
    let source = std::env::current_exe()
        .map_err(|error| format!("cannot determine supervisor executable for helper: {error}"))?;
    stage_from_path(var_dir, &source).map_err(|error| {
        format!(
            "stage supervisor helper {} from {}: {error}",
            path(var_dir).display(),
            source.display()
        )
    })
}

pub fn restore_installed(var_dir: &str, installed: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(installed)
        .map_err(|error| format!("installed target is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("installed target is not an executable regular file".into());
    }
    stage_from_path(var_dir, installed).map_err(|error| {
        format!(
            "restore supervisor helper {} from installed target {}: {error}",
            path(var_dir).display(),
            installed.display()
        )
    })
}

fn stage_from_path(var_dir: &str, source_path: &Path) -> io::Result<()> {
    let var = Path::new(var_dir);
    fs::create_dir_all(var)?;
    let var_metadata = fs::symlink_metadata(var)?;
    if !var_metadata.is_dir() || var_metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "helper var path is not a real directory",
        ));
    }

    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(source_path)?;
    let source_metadata = source.metadata()?;
    if !source_metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supervisor source is not a regular file",
        ));
    }

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let (temporary_path, mut temporary) = {
        let mut attempts = 0;
        loop {
            if attempts == 32 {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "unable to allocate a unique supervisor helper temporary file",
                ));
            }
            attempts += 1;
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary_path = var.join(format!(
                ".{HELPER_NAME}.{}.{}.tmp",
                std::process::id(),
                sequence
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(HELPER_MODE)
                .open(&temporary_path)
            {
                Ok(file) => break (temporary_path, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    };

    let write_result = (|| {
        io::copy(&mut source, &mut temporary)?;
        temporary.set_permissions(fs::Permissions::from_mode(HELPER_MODE))?;
        temporary.sync_all()
    })();
    if let Err(error) = write_result {
        drop(temporary);
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(temporary);

    let destination = path(var_dir);
    if let Err(error) = fs::rename(&temporary_path, &destination) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    File::open(var).and_then(|directory| directory.sync_all())?;

    let metadata = fs::symlink_metadata(&destination)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "staged supervisor helper is not a regular file",
        ));
    }
    if metadata.permissions().mode() & 0o7777 != HELPER_MODE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "staged supervisor helper does not have mode 0750",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::atomic::AtomicU64};

    fn test_root(label: &str) -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "qwenpaw-helper-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn stages_a_regular_0750_helper_atomically() {
        let root = test_root("stage");
        let var = root.join("var");
        let source = root.join("source");
        fs::create_dir_all(&var).unwrap();
        fs::write(&source, b"supervisor-source").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();

        stage_from_path(var.to_str().unwrap(), &source).unwrap();

        let helper = path(var.to_str().unwrap());
        let metadata = fs::symlink_metadata(&helper).unwrap();
        assert!(metadata.file_type().is_file());
        assert_eq!(metadata.permissions().mode() & 0o7777, HELPER_MODE);
        assert_eq!(fs::read(&helper).unwrap(), b"supervisor-source");
        let temporary_prefix = format!(".{HELPER_NAME}.");
        assert!(fs::read_dir(&var)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .all(|name| !name.to_string_lossy().starts_with(&temporary_prefix)));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_replace_a_non_regular_helper_destination() {
        let root = test_root("destination");
        let var = root.join("var");
        let source = root.join("source");
        fs::create_dir_all(&var).unwrap();
        fs::write(&source, b"supervisor-source").unwrap();
        fs::create_dir(var.join(HELPER_NAME)).unwrap();

        assert!(stage_from_path(var.to_str().unwrap(), &source).is_err());
        assert!(var.join(HELPER_NAME).is_dir());
        let temporary_prefix = format!(".{HELPER_NAME}.");
        assert!(fs::read_dir(&var)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .all(|name| !name.to_string_lossy().starts_with(&temporary_prefix)));

        fs::remove_dir_all(root).unwrap();
    }
}
