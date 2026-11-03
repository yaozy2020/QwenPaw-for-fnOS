//! /proc 基元(同步,无状态)—— 模式照搬 dsh backend(MIT)。
//! 血泪:僵尸判定看 stat 最后一个 ')' 之后的状态字符,不看目录存在;
//! comm 可含空格/括号,必须从最后一个 ')' 之后切字段。

use std::fs;

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
pub fn descendants_of(pid: i32) -> Vec<i32> {
    let mut edges: Vec<(i32, i32)> = Vec::new(); // (child, ppid)
    let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
    for e in entries.flatten() {
        let Ok(c) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        if c == pid {
            continue;
        }
        let Ok(s) = fs::read_to_string(format!("/proc/{c}/stat")) else { continue };
        let Some(i) = s.rfind(')') else { continue };
        let ppid = s[i + 2..]
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
        edges.push((c, ppid));
    }
    let mut out: Vec<i32> = Vec::new();
    let mut frontier: Vec<i32> = vec![pid];
    loop {
        let next: Vec<i32> = edges
            .iter()
            .filter(|(_, p)| frontier.contains(p))
            .map(|(c, _)| *c)
            .filter(|c| !out.contains(c))
            .collect();
        if next.is_empty() {
            break;
        }
        out.extend(next.clone());
        frontier = next;
    }
    out
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
        if let Ok(out) = std::process::Command::new("getconf").arg("PAGESIZE").output() {
            if out.status.success() {
                if let Ok(v) = String::from_utf8_lossy(&out.stdout).trim().parse::<i64>() {
                    if v > 0 {
                        return (v / 1024).max(1);
                    }
                }
            }
        }
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 { (v as i64 / 1024).max(1) } else { 4 }
    })
}

pub fn rss_kb(pid: i32) -> i64 {
    let Some(s) = fs::read_to_string(format!("/proc/{pid}/statm")).ok() else {
        return 0;
    };
    let Some(rss_pages) = s.split_whitespace().nth(1) else { return 0 };
    rss_pages.parse::<i64>().unwrap_or(0) * page_size_kb()
}

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
    let boot = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs_f64(uptime))?;
    Some(boot + std::time::Duration::from_secs_f64(start_ticks as f64 / 100.0))
}

/// 扫 /proc:cmdline 特征匹配内核(basename 含 python3 且 -m qwenpaw app --port N)。
/// 返回全部匹配 pid——数量 >1 说明存在多实例(历史孤儿),调用方负责清扫。
pub fn rediscover_all(port: u16, exclude: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else { return out };
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        if pid == exclude || pid == std::process::id() as i32 {
            continue;
        }
        let Ok(raw) = fs::read_to_string(format!("/proc/{pid}/cmdline")) else { continue };
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
