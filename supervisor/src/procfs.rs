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

pub fn process_identity(pid: i32) -> Result<Option<ProcessIdentity>, String> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => parse_process_identity(pid, &stat).map(|(identity, _)| Some(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read /proc/{pid}/stat: {error}")),
    }
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

fn scan_processes() -> Result<Vec<(ProcessIdentity, i32)>, String> {
    let entries = fs::read_dir("/proc").map_err(|error| format!("scan /proc: {error}"))?;
    let mut processes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read /proc entry: {error}"))?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => processes.push(parse_process_identity(pid, &stat)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("read /proc/{pid}/stat while scanning: {error}")),
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

pub fn cpu_ticks(pid: i32) -> (u64, u64) {
    let Some(s) = fs::read_to_string(format!("/proc/{pid}/stat")).ok() else {
        return (0, 0);
    };
    let Some(i) = s.rfind(')') else { return (0, 0) };
    let fields: Vec<&str> = s[i + 2..].split_whitespace().collect();
    if fields.len() < 14 {
        return (0, 0);
    }
    let utime = fields[11].parse().unwrap_or(0);
    let stime = fields[12].parse().unwrap_or(0);
    (utime, stime)
}

/// 页大小(KB),进程内只探测一次。
///
/// 原先硬编码 4 —— x86_64 上正确,但把"该平台恰好是 4KB"当成了普遍事实。
/// 非 4KB 平台(如 arm64 上的 64KB 页)会让 RSS 显示缩小 16 倍。
fn page_size_kb() -> i64 {
    use std::sync::OnceLock;
    static PAGE_KB: OnceLock<i64> = OnceLock::new();
    *PAGE_KB.get_or_init(|| {
        // getconf 是最直接的权威来源;失败时退回 sysconf(SC_PAGESIZE)
        if let Ok(out) = std::process::Command::new("getconf")
            .arg("PAGESIZE")
            .output()
        {
            if out.status.success() {
                if let Ok(v) = String::from_utf8_lossy(&out.stdout).trim().parse::<i64>() {
                    if v > 0 {
                        return (v / 1024).max(1);
                    }
                }
            }
        }
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 {
            (v as i64 / 1024).max(1)
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

/// 扫 /proc:cmdline 特征匹配内核(basename 含 python3 且 -m qwenpaw app --port N)。
/// 返回全部匹配 pid——数量 >1 说明存在多实例(历史孤儿),调用方负责清扫。
pub fn rediscover_all(port: u16, exclude: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return out;
    };
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid == exclude || pid == std::process::id() as i32 {
            continue;
        }
        let Ok(raw) = fs::read_to_string(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        if raw.is_empty() {
            continue; // 僵尸:cmdline 为空,天然不命中
        }
        let argv: Vec<&str> = raw.trim_end_matches('\0').split('\0').collect();
        if is_kernel_argv(&argv, port) {
            out.push(pid);
        }
    }
    out
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
}
