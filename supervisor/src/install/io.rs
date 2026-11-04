use super::sync_dir;
use std::{
    ffi::{OsStr, OsString},
    fs::{File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) fn relocate_console_script_shebangs(
    staged: &Path,
    active: &Path,
) -> std::io::Result<()> {
    let staged_metadata = std::fs::symlink_metadata(staged)?;
    if !staged_metadata.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "venv staging path must be a real directory",
        ));
    }
    let bin = staged.join("bin");
    let bin_metadata = std::fs::symlink_metadata(&bin)?;
    if !bin_metadata.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "venv bin path must be a real directory",
        ));
    }

    let entries = std::fs::read_dir(&bin)?.collect::<std::io::Result<Vec<_>>>()?;
    let mut aliases = vec![OsString::from("python3")];
    for entry in &entries {
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        if (file_type.is_file() || file_type.is_symlink())
            && is_python_interpreter_alias(&name)
            && !aliases.contains(&name)
        {
            aliases.push(name);
        }
    }

    let active_python = active.join("bin/python3");
    let active_python_bytes = active_python.as_os_str().as_bytes();
    if active_python_bytes.contains(&b'\n') || active_python_bytes.contains(&b'\r') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "active venv interpreter path cannot contain a line break",
        ));
    }

    for entry in entries {
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let mut source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)?;
        let metadata = source.metadata()?;
        if !metadata.is_file() {
            continue;
        }

        let matched_interpreter = aliases.iter().find_map(|alias| {
            let interpreter = bin.join(alias);
            let interpreter_bytes = interpreter.as_os_str().as_bytes();
            if interpreter_bytes.contains(&b'\n') || interpreter_bytes.contains(&b'\r') {
                return None;
            }
            match exact_shebang(&mut source, metadata.len(), interpreter_bytes) {
                Ok(true) => Some(Ok(interpreter_bytes.to_vec())),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            }
        });
        let Some(matched_interpreter) = matched_interpreter else {
            continue;
        };
        let matched_interpreter = matched_interpreter?;

        source.seek(SeekFrom::Start(0))?;
        let mut original = Vec::new();
        source.read_to_end(&mut original)?;
        let path_end = 2 + matched_interpreter.len();
        let mut replacement = Vec::with_capacity(
            original.len() - matched_interpreter.len() + active_python_bytes.len(),
        );
        replacement.extend_from_slice(&original[..2]);
        replacement.extend_from_slice(active_python_bytes);
        replacement.extend_from_slice(&original[path_end..]);

        atomic_replace_file(&path, &bin, &metadata, &replacement)?;
    }
    Ok(())
}

fn is_python_interpreter_alias(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    bytes == b"python"
        || bytes
            .strip_prefix(b"python3.")
            .is_some_and(|version| !version.is_empty() && version.iter().all(u8::is_ascii_digit))
}

fn exact_shebang(file: &mut File, file_len: u64, interpreter: &[u8]) -> std::io::Result<bool> {
    let mut expected = Vec::with_capacity(interpreter.len() + 2);
    expected.extend_from_slice(b"#!");
    expected.extend_from_slice(interpreter);
    if file_len < expected.len() as u64 {
        return Ok(false);
    }

    file.seek(SeekFrom::Start(0))?;
    let mut prefix = vec![0; expected.len()];
    file.read_exact(&mut prefix)?;
    if prefix != expected {
        return Ok(false);
    }

    let mut delimiter = [0; 1];
    match file.read(&mut delimiter)? {
        0 => Ok(true),
        1 if delimiter[0] == b'\n' => Ok(true),
        1 if delimiter[0] == b'\r' => {
            let mut next = [0; 1];
            Ok(file.read(&mut next)? == 0 || next[0] == b'\n')
        }
        _ => Ok(false),
    }
}

fn atomic_replace_file(
    path: &Path,
    parent: &Path,
    original_metadata: &Metadata,
    contents: &[u8],
) -> std::io::Result<()> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let permissions = original_metadata.permissions();
    let (temporary_path, mut temporary_file) = {
        let mut attempts = 0;
        loop {
            if attempts == 32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "unable to allocate a unique console-script temporary file",
                ));
            }
            attempts += 1;
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary_path = parent.join(format!(
                ".qwenpaw-console-script.{}.{}.tmp",
                std::process::id(),
                sequence
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(permissions.mode() & 0o7777)
                .open(&temporary_path)
            {
                Ok(file) => break (temporary_path, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    };

    let write_result = temporary_file
        .write_all(contents)
        .and_then(|()| temporary_file.set_permissions(permissions))
        .and_then(|()| temporary_file.sync_all());
    if let Err(error) = write_result {
        drop(temporary_file);
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(temporary_file);

    let replace_result = (|| {
        let current = std::fs::symlink_metadata(path)?;
        if !current.file_type().is_file()
            || current.dev() != original_metadata.dev()
            || current.ino() != original_metadata.ino()
        {
            return Err(std::io::Error::other(
                "console script changed while its shebang was being relocated",
            ));
        }
        std::fs::rename(&temporary_path, path)?;
        sync_dir(parent)
    })();
    if replace_result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    replace_result
}

/// 0600 权限的原子写:临时文件 → fsync → rename。半写内容不会出现在目标路径。
pub(crate) fn atomic_write_0600(path: &str, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    // tmp 名带 pid:固定 "{path}.tmp" 在并发写(如变更端口与状态落盘同时发生)
    // 时会互相截断,rename 出空文件或对方的内容。
    let tmp = format!("{}.tmp.{}", path, std::process::id());
    {
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        let write_result = f.write_all(content.as_bytes()).and_then(|_| f.sync_all());
        if let Err(e) = write_result {
            drop(f);
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    let rename_result = std::fs::rename(&tmp, path);
    if rename_result.is_ok() {
        // rename 后 fsync 父目录:否则断电时 rename 可能未持久,
        // 版本钉/端口/pid 回退为旧值(文件内容本身已 fsync 不够)
        if let Some(dir) = std::path::Path::new(path).parent() {
            sync_dir(dir)?;
        }
    }
    rename_result.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// 清扫 var_dir 下的原子写/临时落盘残骸(`*.tmp` 与 `*.tmp.<pid>`)。
///
/// 这些文件只在"写入途中被 SIGKILL/断电"时才会留下。它们本身无害,但:
/// 老实现用固定 tmp 名,残骸会让后续 create_new 直接失败;
/// 长期累积也会把"目录里有陌生文件"变成常态,掩盖真正的事务残留。
/// 只在启动恢复阶段调用(此时无人正在写),按 mtime 之外的保守规则一律视为残骸。
pub(crate) fn sweep_stale_tmp_files(var_dir: &str) -> usize {
    let mut removed = 0usize;
    let Ok(entries) = std::fs::read_dir(var_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_tmp = name.contains(".tmp");
        if !is_tmp {
            continue;
        }
        // 只清理普通文件;目录型残留由事务恢复各自处理,不在此处误删。
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}
