use super::InstallState;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    process::Command,
    time::Duration,
};

const ARCHIVE: &str = "preinstall-data.tar";

pub(super) fn create(state: &InstallState, data: &Path, pid_file: &Path) -> io::Result<()> {
    let var = pid_file
        .parent()
        .ok_or_else(|| io::Error::other("缺少应用 var 目录"))?;
    let parent = data
        .parent()
        .filter(|_| data.ends_with(".qwenpaw"))
        .ok_or_else(|| io::Error::other("无法确认数据根目录"))?;
    let private = fs::metadata(var)?;
    if !var.is_absolute() || !private.is_dir() || private.mode() & 0o022 != 0 {
        return Err(io::Error::other("应用 var 不是受保护的目录，拒绝写入备份"));
    }
    let final_path = var.join(ARCHIVE);
    let absent = |path: &Path| match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    };
    if !absent(&final_path)? {
        return Err(io::Error::other(
            "已有 preinstall-data.tar；请先将它移到私有备份目录，再创建新备份",
        ));
    }
    let mut names = Vec::new();
    let mut nonempty = false;
    for name in [".qwenpaw", ".qwenpaw.secret"] {
        let root = parent.join(name);
        if absent(&root)? {
            continue;
        }
        if !root.symlink_metadata()?.is_dir() {
            return Err(io::Error::other("数据根不是实际目录"));
        }
        nonempty |= fs::read_dir(root)?.next().transpose()?.is_some();
        names.push(name);
    }
    if !nonempty {
        return Ok(());
    }
    let tmp = var.join("preinstall-data.tar.partial");
    // A leftover partial file is never taken as a good backup or overwritten.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("无法创建 preinstall-data.tar.partial（中断残留请先移走）: {e}"),
            )
        })?;
    let result = (|| {
        if file.metadata()?.mode() & 0o077 != 0 {
            return Err(io::Error::other("存储卷未保留备份文件的私有权限"));
        }
        let mut tar = Command::new("/usr/bin/tar");
        tar.env_clear()
            .env("LC_ALL", "C")
            .current_dir(parent)
            .args([
                "--create",
                "--verify",
                "--one-file-system",
                "--acls",
                "--xattrs",
                "--sparse",
                "--file",
            ])
            .arg(&tmp)
            .arg("--")
            .args(names);
        let output = state.run_cancellable_command(
            tar,
            Duration::from_secs(3600),
            "校验安装前备份",
            pid_file,
        )?;
        if !output.status.success() || !output.stderr.is_empty() {
            return Err(io::Error::other(format!(
                "tar 未完整备份或校验失败（{}）；请检查剩余空间、读取权限和特殊文件",
                output.status
            )));
        }
        file.sync_all()?;
        fs::hard_link(&tmp, &final_path)?;
        File::open(var)?.sync_all()
    })();
    fs::remove_file(&tmp)?;
    result
}
