//! /proc 基元(同步,无状态)—— 模式照搬 dsh backend(MIT)。
//! 血泪:僵尸判定看 stat 最后一个 ')' 之后的状态字符,不看目录存在;
//! comm 可含空格/括号,必须从最后一个 ')' 之后切字段。

use std::{fs, io};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub starttime: u64,
    pub pgrp: i32,
    pub session: i32,
    state: char,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityStatus {
    Alive,
    Gone,
    Reused,
    Unknown(String),
}

fn parse_process_identity(pid: i32, stat: &str) -> Result<(ProcessIdentity, i32), String> {
    let i = stat
        .rfind(')')
        .ok_or_else(|| format!("invalid /proc/{pid}/stat"))?;
    let fields: Vec<&str> = stat[i + 2..].split_whitespace().collect();
    if fields.len() < 20 {
        return Err(format!("incomplete /proc/{pid}/stat"));
    }
    let state = fields[0]
        .chars()
        .next()
        .ok_or_else(|| format!("missing state in /proc/{pid}/stat"))?;
    let ppid = fields[1]
        .parse()
        .map_err(|_| format!("invalid ppid in /proc/{pid}/stat"))?;
    let pgrp = fields[2]
        .parse()
        .map_err(|_| format!("invalid pgrp in /proc/{pid}/stat"))?;
    let session = fields[3]
        .parse()
        .map_err(|_| format!("invalid session in /proc/{pid}/stat"))?;
    let starttime = fields[19]
        .parse()
        .map_err(|_| format!("invalid starttime in /proc/{pid}/stat"))?;
    Ok((
        ProcessIdentity {
            pid,
            starttime,
            pgrp,
            session,
            state,
        },
        ppid,
    ))
}

fn process_entry<T>(read: io::Result<T>) -> io::Result<Option<T>> {
    match read {
        Ok(value) => Ok(Some(value)),
        // procfs may return ESRCH on read if the task vanished after open.
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_stat(pid: i32) -> Result<Option<(ProcessIdentity, i32)>, String> {
    process_entry(fs::read_to_string(format!("/proc/{pid}/stat")))
        .map_err(|error| format!("read /proc/{pid}/stat: {error}"))?
        .map(|stat| parse_process_identity(pid, &stat))
        .transpose()
}

pub fn process_identity(pid: i32) -> Result<Option<ProcessIdentity>, String> {
    Ok(read_stat(pid)?.map(|(identity, _)| identity))
}

pub fn identity_status(identity: ProcessIdentity) -> IdentityStatus {
    match process_identity(identity.pid) {
        Ok(None) => IdentityStatus::Gone,
        Ok(Some(current)) if current.starttime != identity.starttime => IdentityStatus::Reused,
        Ok(Some(current)) if matches!(current.state, 'Z' | 'X') => IdentityStatus::Gone,
        Ok(Some(_)) => IdentityStatus::Alive,
        Err(error) => IdentityStatus::Unknown(error),
    }
}

pub(crate) fn scan_processes() -> Result<Vec<(ProcessIdentity, i32)>, String> {
    let entries = fs::read_dir("/proc").map_err(|error| format!("scan /proc: {error}"))?;
    let mut processes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read /proc entry: {error}"))?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if let Some(entry) = read_stat(pid)? {
            processes.push(entry);
        }
    }
    Ok(processes)
}

pub fn processes_in_group(pgrp: i32, session: i32) -> Result<Vec<ProcessIdentity>, String> {
    if pgrp <= 1 || session <= 1 {
        return Err(format!("invalid process group/session {pgrp}/{session}"));
    }
    Ok(scan_processes()?
        .into_iter()
        .filter_map(|(identity, _)| {
            (identity.pgrp == pgrp
                && identity.session == session
                && !matches!(identity.state, 'Z' | 'X'))
            .then_some(identity)
        })
        .collect())
}

pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => {
            let Some(i) = s.rfind(')') else { return false };
            let rest = &s[i + 2..];
            matches!(
                rest.chars().next(),
                Some('R') | Some('S') | Some('D') | Some('T') | Some('t')
            )
        }
        Err(_) => false,
    }
}

/// 全部后代(按 /proc ppid 树广度优先收集;必须在目标存活时调用 ——
/// 死后整棵子树 reparent 到 1,按 ppid 再也找不到)。覆盖 b2 shared_worker
/// 这类双 setsid 自成进程组的直接子进程,以及 worker 再派生的孙进程。
pub fn descendants_of_checked(root: ProcessIdentity) -> Result<Vec<ProcessIdentity>, String> {
    let processes = scan_processes()?;
    if !processes
        .iter()
        .any(|(identity, _)| identity.pid == root.pid && identity.starttime == root.starttime)
    {
        return Err(format!(
            "leader PID {} identity changed during descendant scan",
            root.pid
        ));
    }
    let mut edges: Vec<(ProcessIdentity, i32)> = processes;
    let mut out = Vec::new();
    let mut frontier = vec![root.pid];
    loop {
        let next: Vec<ProcessIdentity> = edges
            .iter()
            .filter(|(_, ppid)| frontier.contains(ppid))
            .map(|(identity, _)| *identity)
            .filter(|identity| {
                !out.iter()
                    .any(|seen: &ProcessIdentity| seen.pid == identity.pid)
            })
            .collect();
        if next.is_empty() {
            break;
        }
        frontier = next.iter().map(|identity| identity.pid).collect();
        out.extend(next);
        edges.retain(|(identity, _)| !out.iter().any(|seen| seen.pid == identity.pid));
    }
    match process_identity(root.pid)? {
        Some(current) if current.starttime == root.starttime => Ok(out),
        _ => Err(format!(
            "leader PID {} identity changed during descendant scan",
            root.pid
        )),
    }
}

pub fn cpu_ticks(id: ProcessIdentity) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", id.pid)).ok()?;
    let mut fields = stat.get(stat.rfind(')')? + 2..)?.split_whitespace();
    if matches!(fields.next()?, "Z" | "X") {
        return None;
    }
    let user: u64 = fields.nth(10)?.parse().ok()?;
    let sys: u64 = fields.next()?.parse().ok()?;
    let start: u64 = fields.nth(6)?.parse().ok()?;
    (start == id.starttime).then(|| user.saturating_add(sys))
}

/// 页大小(KB),进程内只探测一次。
///
/// 原先硬编码 4 —— x86_64 上正确,但把"该平台恰好是 4KB"当成了普遍事实。
/// 非 4KB 平台(如 arm64 上的 64KB 页)会让 RSS 显示缩小 16 倍。
fn page_size_kb() -> i64 {
    use std::sync::OnceLock;
    static PAGE_KB: OnceLock<i64> = OnceLock::new();
    *PAGE_KB.get_or_init(|| {
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 {
            (v / 1024).max(1)
        } else {
            4
        }
    })
}

pub fn rss_kb(pid: i32) -> i64 {
    let Some(s) = fs::read_to_string(format!("/proc/{pid}/statm")).ok() else {
        return 0;
    };
    let Some(rss_pages) = s.split_whitespace().nth(1) else {
        return 0;
    };
    rss_pages.parse::<i64>().unwrap_or(0) * page_size_kb()
}

#[cfg(test)]
pub fn group_of(pid: i32) -> i32 {
    let Some(s) = fs::read_to_string(format!("/proc/{pid}/stat")).ok() else {
        return 0;
    };
    let Some(i) = s.rfind(')') else { return 0 };
    let fields: Vec<&str> = s[i + 2..].split_whitespace().collect();
    if fields.len() < 4 {
        return 0;
    }
    fields[2].parse().unwrap_or(0) // pgrp
}

/// 进程真实启动时刻(墙钟):stat 的 starttime(分钟级精度够用)。
pub fn started_wall(pid: i32) -> Option<std::time::SystemTime> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let i = stat.rfind(')')?;
    let fields: Vec<&str> = stat[i + 2..].split_whitespace().collect();
    let start_ticks: u64 = fields.get(19)?.parse().ok()?;
    let uptime: f64 = fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let boot =
        std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs_f64(uptime))?;
    Some(boot + std::time::Duration::from_secs_f64(start_ticks as f64 / 100.0))
}

pub fn rediscover_all_in_venv(port: u16, exclude: i32, venv_dir: &str) -> Result<Vec<i32>, String> {
    use std::os::unix::fs::MetadataExt;

    let owner_path = std::path::Path::new(venv_dir)
        .parent()
        .ok_or_else(|| "kernel venv has no parent directory".to_string())?;
    let expected_uid = fs::metadata(venv_dir)
        .or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                fs::metadata(owner_path)
            } else {
                Err(error)
            }
        })
        .map_err(|error| format!("inspect kernel venv owner: {error}"))?
        .uid();
    let entries = fs::read_dir("/proc").map_err(|error| format!("scan /proc: {error}"))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read /proc entry: {error}"))?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid == exclude || pid == std::process::id() as i32 {
            continue;
        }
        let raw = match fs::read_to_string(format!("/proc/{pid}/cmdline")) {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("read /proc/{pid}/cmdline: {error}")),
        };
        if raw.is_empty() {
            continue;
        }
        let argv: Vec<&str> = raw.trim_end_matches('\0').split('\0').collect();
        if !is_kernel_argv_in_venv(&argv, port, venv_dir) {
            continue;
        }
        let process_uid = match fs::metadata(format!("/proc/{pid}")) {
            Ok(metadata) => metadata.uid(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("inspect /proc/{pid} owner: {error}")),
        };
        if process_uid == expected_uid {
            out.push(pid);
        }
    }
    Ok(out)
}

fn is_kernel_argv_in_venv(argv: &[&str], port: u16, venv_dir: &str) -> bool {
    let Some(program) = argv.first() else {
        return false;
    };
    let program = std::path::Path::new(program);
    let venv = std::path::Path::new(venv_dir);
    let in_venv_bin = program
        .parent()
        .is_some_and(|parent| parent == venv.join("bin"));
    let python_program = program
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("python3"));
    in_venv_bin && python_program && is_kernel_argv(argv, port)
}

fn is_kernel_argv(argv: &[&str], port: u16) -> bool {
    let mut ok_base = false;
    let mut ok_mod = false;
    let mut ok_app = false;
    let mut ok_port = false;
    for (i, a) in argv.iter().enumerate() {
        if std::path::Path::new(a)
            .file_name()
            .map(|f| f.to_string_lossy().starts_with("python3"))
            .unwrap_or(false)
        {
            ok_base = true;
        }
        match *a {
            "-m" => ok_mod = argv.get(i + 1) == Some(&"qwenpaw"),
            "app" => ok_app = true,
            "--port" => {
                ok_port = argv.get(i + 1).and_then(|p| p.parse::<u16>().ok()) == Some(port);
            }
            _ => {}
        }
    }
    ok_base && ok_mod && ok_app && ok_port
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_opened_before_child_exit_is_a_vanished_process() {
        use std::process::{Command, Stdio};

        let mut child = Command::new("cat").stdin(Stdio::piped()).spawn().unwrap();
        let pid = child.id() as i32;
        let stat = fs::File::open(format!("/proc/{pid}/stat")).unwrap();
        assert!(child.wait().unwrap().success()); // wait closes the child's stdin first
        let read = io::read_to_string(stat);
        assert_eq!(read.as_ref().unwrap_err().raw_os_error(), Some(libc::ESRCH));
        assert_eq!(process_entry(read).unwrap(), None);
        assert_eq!(process_identity(pid).unwrap(), None);
    }

    #[test]
    fn process_entry_does_not_hide_permission_or_io_errors() {
        for errno in [libc::EACCES, libc::EPERM, libc::EIO] {
            let read = Err::<String, _>(io::Error::from_raw_os_error(errno));
            assert_eq!(process_entry(read).unwrap_err().raw_os_error(), Some(errno));
        }
    }

    #[test]
    fn process_identity_and_group_scans_preserve_starttime_session_and_group() {
        let pid = std::process::id() as i32;
        let identity = process_identity(pid).unwrap().unwrap();
        assert_eq!(identity.pgrp, group_of(pid));
        assert_eq!(identity_status(identity), IdentityStatus::Alive);
        assert!(processes_in_group(identity.pgrp, identity.session)
            .unwrap()
            .iter()
            .any(|member| member.pid == pid && member.starttime == identity.starttime));
        descendants_of_checked(identity).unwrap();

        let mut stale = identity;
        stale.starttime = stale.starttime.saturating_add(1);
        assert_eq!(identity_status(stale), IdentityStatus::Reused);
    }

    #[test]
    fn uninstall_rediscovery_requires_the_configured_venv_interpreter() {
        let valid = [
            "/srv/home/venv/bin/python3.real",
            "-m",
            "qwenpaw",
            "app",
            "--port",
            "18088",
        ];
        assert!(is_kernel_argv_in_venv(&valid, 18088, "/srv/home/venv"));
        assert!(!is_kernel_argv_in_venv(&valid, 18088, "/srv/other/venv"));
        assert!(!is_kernel_argv_in_venv(&valid, 18089, "/srv/home/venv"));
        let external = [
            "/usr/bin/python3",
            "-m",
            "qwenpaw",
            "app",
            "--port",
            "18088",
        ];
        assert!(!is_kernel_argv_in_venv(&external, 18088, "/srv/home/venv"));
    }
}
