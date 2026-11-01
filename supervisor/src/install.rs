//! 内核安装状态机 —— 模式照搬 dsh server.go(单槽状态 + Seq 去重 + 终态唯一写入者)。
//! 纪律(S4/S6/S8):不编造百分比,只报"已获取 N 项"+ 尾行;终态唯一写入者是
//! 安装线程;失败清理半成品;Seq 是前端"结果只提示一次"的去重键。
//!
//! 安全(P0,勿回退):版本/镜像一律走 argv 直传 pip,绝不拼接 bash -c。
//! 历史版本曾把用户可控的版本号拼进 Shell 脚本构成命令注入 —— JSON 转义只能
//! 保护传输格式,防不了 Shell 解释;镜像白名单也替代不了版本号校验,两者都要。

use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Downloading,
    Installing,
    Done,
    Failed,
}

/// 安装渠道:latest / beta / 精确版本。精确版本经 PEP 440 字符白名单校验后
/// 以 `qwenpaw=={v}` 单个 argv 传给 pip;latest/beta 是枚举而非字符串拼装。
#[derive(Clone, Debug)]
pub enum InstallChannel {
    Latest,
    Beta,
    Exact(String),
}

impl InstallChannel {
    /// 控制台/桥送来的 version 字段 → 渠道;非法值返回 None(调用方拒绝请求)。
    pub fn parse(v: &str) -> Option<InstallChannel> {
        match v {
            "latest" => Some(InstallChannel::Latest),
            "beta" | "pre" => Some(InstallChannel::Beta),
            other if is_safe_exact_version(other) => {
                Some(InstallChannel::Exact(other.to_string()))
            }
            _ => None,
        }
    }
}

/// 精确版本白名单:仅 PEP 440 合法字符(字母数字 . _ + -),不得以 . 或 - 开头
/// (后者会被 pip 当作选项)。长度上限 64。凭此拒绝 requirement 语法、URL、
/// 本地路径、git+https、附加参数与一切 Shell 元字符 —— 不依赖 pip 解析报错,
/// 也不自行实现不完整的 PEP 440 语义校验。
fn is_safe_exact_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
        && !v.starts_with('.')
        && !v.starts_with('-')
}

/// pip 参数构造(纯函数,单测覆盖):始终 argv,镜像为独立 --index-url 参数。
pub fn pip_args(channel: &InstallChannel, mirror: &str) -> Vec<String> {
    let mut args: Vec<String> = ["install", "--upgrade"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    match channel {
        InstallChannel::Latest => args.push("qwenpaw".into()),
        InstallChannel::Beta => {
            args.push("--pre".into());
            args.push("qwenpaw".into());
        }
        InstallChannel::Exact(v) => args.push(format!("qwenpaw=={v}")),
    }
    if !mirror.is_empty() {
        args.push("--index-url".into());
        args.push(mirror.into());
    }
    args
}

/// 镜像白名单:http(s) + URL 安全字符;不合法返回空(pip 落官方源)。
/// 桥端 set_mirror 有同一份白名单,这里兜底直连监督者的调用方。
pub fn sanitize_mirror(m: &str) -> String {
    if (m.starts_with("http://") || m.starts_with("https://"))
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || ":/._~-".contains(c))
    {
        m.to_string()
    } else {
        String::new()
    }
}

/// 0600 权限的原子写:临时文件 → fsync → rename。半写内容不会出现在目标路径。
fn atomic_write_0600(path: &str, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = format!("{}.tmp", path);
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// 单行落盘(超限排水)。stdout/stderr 两个线程各自持有 append 句柄调用。
fn append_line(log: &mut Option<std::fs::File>, total: &Arc<Mutex<u64>>, raw: &str) {
    if let Some(f) = log.as_mut() {
        let mut t = total.lock().unwrap();
        if *t >= LOG_TOTAL_CAP {
            return;
        }
        let mut owned = raw.to_string();
        if owned.len() > LOG_LINE_CAP {
            owned.truncate(LOG_LINE_CAP);
            owned.push('…');
        }
        if f.write_all(owned.as_bytes()).is_ok() && f.write_all(b"\n").is_ok() {
            *t += owned.len() as u64 + 1;
        }
    }
}

/// 刷新看门狗活动时间戳(自任务启动起的毫秒数)
fn touch_activity(last: &AtomicU64, start: &std::time::Instant) {
    last.store(
        start.elapsed().as_millis() as u64,
        Ordering::Relaxed,
    );
}

#[derive(Clone, Debug)]
pub struct InstallSnapshot {
    pub phase: Phase,
    pub fetched: u64,
    pub message: String,
    pub error: String,
    pub seq: u64,
    pub running: bool,
}

#[derive(Clone)]
struct Inner {
    phase: Phase,
    fetched: u64,
    message: String,
    error: String,
    seq: u64,
    running: bool,
    pid: u32,
    cancelled: bool,
}

#[derive(Clone)]
pub struct InstallState {
    inner: Arc<Mutex<Inner>>,
    on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
    on_success: Arc<dyn Fn() + Send + Sync>,
}

/// 单行日志上限:pip 行内含长 URL/异常栈,超长截断防日志膨胀与前端卡顿
const LOG_LINE_CAP: usize = 4096;
/// 单次任务日志总量上限(超过后仅排水不再落盘)
const LOG_TOTAL_CAP: u64 = 1024 * 1024;
/// 看门狗:无输出超过 15 分钟视为镜像失联/任务卡死。慢镜像单文件下载间隙
/// 远小于此(pip 自身 socket 超时 15s + 重试),真挂起会长期静默。
/// 若无看门狗,running 永久为 true,后续安装被 busy 永久拒绝(手机端表现为一直转圈)。
const NO_OUTPUT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// 看门狗:任务总时长上限(40MB 内核 + 依赖在慢链路下的宽裕上限)
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

impl InstallState {
    pub fn new(
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_success: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                phase: Phase::Idle,
                fetched: 0,
                message: String::new(),
                error: String::new(),
                seq: 0,
                running: false,
                pid: 0,
                cancelled: false,
            })),
            on_change,
            on_success,
        }
    }

    pub fn snapshot(&self) -> InstallSnapshot {
        let g = self.inner.lock().unwrap();
        InstallSnapshot {
            phase: g.phase.clone(),
            fetched: g.fetched,
            message: g.message.clone(),
            error: g.error.clone(),
            seq: g.seq,
            running: g.running,
        }
    }

    fn emit(&self, snap: InstallSnapshot) {
        (self.on_change)(snap);
    }

    fn set(&self, f: impl FnOnce(&mut Inner)) {
        let snap = {
            let mut g = self.inner.lock().unwrap();
            f(&mut g);
            InstallSnapshot {
                phase: g.phase.clone(),
                fetched: g.fetched,
                message: g.message.clone(),
                error: g.error.clone(),
                seq: g.seq,
                running: g.running,
            }
        };
        self.emit(snap);
    }

    /// 取消进行中的安装:进程组 TERM → 宽限 → 组 KILL。
    /// 只在任务运行时执行;实际状态转换仍由安装线程在 wait 返回后完成。
    /// PID 复用窗口极小且组信号对无关组是 ESRCH 空操作,可接受(勿凭 PID 乱杀)。
    pub fn cancel(&self) {
        let (running, pid) = {
            let g = self.inner.lock().unwrap();
            (g.running, g.pid)
        };
        if !running || pid == 0 {
            return;
        }
        let pgid = pid as i32;
        unsafe { libc::kill(-pgid, libc::SIGTERM) };
        for _ in 0..10 {
            if !crate::procfs::process_alive(pgid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
        self.set(|g| g.cancelled = true);
    }

    /// 运行 pip 安装(独立进程组 + 专用阻塞线程;终态唯一写入者是安装线程)。
    /// 返回是否成功启动(重复触发返回 false,对齐 dsh 的 busy 语义)。
    ///
    /// `pin_path` 是版本钉;成功(且元数据校验通过)后原子写入实际安装版本。
    pub fn try_start(
        &self,
        venv_dir: String,
        channel: InstallChannel,
        mirror: String,
        pip_log: String,
        pin_path: String,
    ) -> bool {
        {
            let mut g = self.inner.lock().unwrap();
            if g.running {
                return false;
            }
            g.seq += 1;
            g.running = true;
            g.phase = Phase::Downloading;
            g.fetched = 0;
            g.message = "启动安装".into();
            g.error = String::new();
            g.pid = 0;
            g.cancelled = false;
        }
        self.emit(self.snapshot());

        let this = self.clone();
        let on_success = std::sync::Arc::clone(&self.on_success);
        std::thread::spawn(move || {
            // 直接用虚拟环境内的 python,不激活环境、不经过 Shell(激活只影响
            // PATH/提示符;`venv/bin/python3 -m pip` 与激活后等价)
            let python = Path::new(&venv_dir).join("bin/python3");
            let mut cmd = std::process::Command::new(&python);
            cmd.arg("-m").arg("pip");
            for a in pip_args(&channel, &mirror) {
                cmd.arg(a);
            }
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            use std::os::unix::process::CommandExt;
            cmd.process_group(0); // 独立进程组:失败/取消可整组清扫
            // 每次任务截断重开,不跨任务追加(避免 pip 日志无限膨胀)
            let log = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&pip_log)
                .ok();
            let total = Arc::new(Mutex::new(0u64));

            let spawned = cmd.spawn();
            match spawned {
                Ok(mut child) => {
                    let pid = child.id();
                    this.set(|g| g.pid = pid);
                    // 安装 PID 落盘:监督者重启后凭此清理孤儿 pip(身份核对见 main.rs)
                    let pid_file = Path::new(&pip_log)
                        .parent()
                        .map(|p| p.join("install.pid"))
                        .unwrap_or_default();
                    let _ = std::fs::write(&pid_file, format!("{}\n", pid));

                    // 看门狗共享状态:任一输出线程每落一行就刷新活动时间戳
                    let started = Arc::new(std::time::Instant::now());
                    let last_activity = Arc::new(AtomicU64::new(0));

                    // stdout/stderr 各自独立排水线程(持 append 句柄逐行落盘)。
                    // 主线程不再阻塞读日志,改为看门狗轮询 —— 否则无输出超时无法实现
                    let mut readers: Vec<std::thread::JoinHandle<()>> = Vec::new();

                    if let Some(err) = child.stderr.take() {
                        let log2 = log.as_ref().and_then(|f| f.try_clone().ok());
                        let total2 = Arc::clone(&total);
                        let act2 = Arc::clone(&last_activity);
                        let start2 = Arc::clone(&started);
                        readers.push(std::thread::spawn(move || {
                            let mut logf = log2;
                            let r = BufReader::new(err);
                            for line in r.split(b'\n') {
                                match line {
                                    Ok(b) => {
                                        touch_activity(&act2, &start2);
                                        let s = String::from_utf8_lossy(&b);
                                        append_line(&mut logf, &total2, s.trim_end_matches(['\r']));
                                    }
                                    Err(_) => break,
                                }
                            }
                        }));
                    }

                    if let Some(out) = child.stdout.take() {
                        let this_r = this.clone();
                        let log1 = log.as_ref().and_then(|f| f.try_clone().ok());
                        let total1 = Arc::clone(&total);
                        let act1 = Arc::clone(&last_activity);
                        let start1 = Arc::clone(&started);
                        readers.push(std::thread::spawn(move || {
                            let mut logf = log1;
                            let mut reader = BufReader::new(out);
                            let mut buf: Vec<u8> = Vec::new();
                            loop {
                                buf.clear();
                                // read_until + lossy:pip 吐非法 UTF-8 时逐行降级而不是
                                // Err 中断(曾导致日志从此截断、进度消息冻结)
                                match reader.read_until(b'\n', &mut buf) {
                                    Ok(0) => break,
                                    Ok(_) => {
                                        touch_activity(&act1, &start1);
                                        let owned = String::from_utf8_lossy(&buf);
                                        let l = owned.trim_end();
                                        append_line(&mut logf, &total1, l);
                                        if l.starts_with("Collecting ") {
                                            let pkg = l
                                                .strip_prefix("Collecting ")
                                                .unwrap_or("")
                                                .split(' ')
                                                .next()
                                                .unwrap_or("")
                                                .to_string();
                                            this_r.set(|g| {
                                                g.fetched += 1;
                                                g.message = format!("获取 {pkg}");
                                            });
                                        } else if l.starts_with("Downloading ") {
                                            this_r.set(|g| g.message = "下载中".into());
                                        } else if l.starts_with("Successfully installed ") {
                                            // 进入真实安装阶段(此前 Installing 变体从未
                                            // 被构造,前端只能凭 message 猜)
                                            this_r.set(|g| {
                                                g.phase = Phase::Installing;
                                                g.message = "安装依赖包".into();
                                            });
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }));
                    }

                    // 看门狗主循环:进程退出/无输出超时/总时长超限三出口。
                    // 超时整组终止(组 TERM → 宽限 → 组 KILL),防 busy 永久锁死
                    let mut exit_ok = false;
                    let mut wait_err: Option<String> = None;
                    let mut timeout_reason = String::new();
                    loop {
                        match child.try_wait() {
                            Ok(Some(st)) => {
                                exit_ok = st.success();
                                break;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                wait_err = Some(e.to_string());
                                break;
                            }
                        }
                        let elapsed = started.elapsed();
                        let idle_ms =
                            elapsed.as_millis() as u64 - last_activity.load(Ordering::Relaxed);
                        if idle_ms > NO_OUTPUT_TIMEOUT.as_millis() as u64 {
                            timeout_reason = format!(
                                "安装超时: 无输出超过 {} 分钟,已终止(下载源可能失联,请换源重试)",
                                NO_OUTPUT_TIMEOUT.as_secs() / 60
                            );
                            break;
                        }
                        if elapsed > TOTAL_TIMEOUT {
                            timeout_reason = format!(
                                "安装超时: 总时长超过 {} 分钟,已终止",
                                TOTAL_TIMEOUT.as_secs() / 60
                            );
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    if !timeout_reason.is_empty() {
                        unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
                        for _ in 0..10 {
                            if child.try_wait().map(|s| s.is_some()).unwrap_or(true) {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(500));
                        }
                        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
                        let _ = child.wait();
                    }
                    // 等两个排水线程随管道 EOF 退出,避免它们写终态后的日志文件句柄
                    for h in readers {
                        let _ = h.join();
                    }
                    let cancelled = {
                        let g = this.inner.lock().unwrap();
                        g.cancelled
                    };
                    let _ = std::fs::remove_file(&pid_file);
                    this.set(|g| g.pid = 0);

                    // 终态前必须核实:venv 元数据里实际装到的版本
                    let mut ok = exit_ok && !cancelled;
                    let mut reason = if cancelled {
                        "安装已被停止".to_string()
                    } else if !timeout_reason.is_empty() {
                        timeout_reason
                    } else if let Some(e) = wait_err {
                        format!("pip 进程等待异常: {e}")
                    } else if !exit_ok {
                        "pip 安装失败,详见日志".to_string()
                    } else {
                        String::new()
                    };
                    let mut actual = String::new();
                    if ok {
                        actual = metadata_version(&python);
                        if actual.is_empty() {
                            ok = false;
                            reason = "安装命令成功但无法读取内核元数据版本,视为失败".into();
                        } else if let InstallChannel::Exact(want) = &channel {
                            if !version_matches(&actual, want) {
                                ok = false;
                                reason =
                                    format!("安装版本({actual})与请求版本({want})不一致,视为失败");
                                actual = String::new();
                            }
                        }
                    }

                    this.set(|g| {
                        g.running = false;
                        if ok {
                            g.phase = Phase::Done;
                            g.message = if actual.is_empty() {
                                "安装完成".into()
                            } else {
                                format!("安装完成 v{actual}")
                            };
                        } else {
                            g.phase = Phase::Failed;
                            g.error = reason;
                        }
                    });
                    if ok {
                        // 版本钉只记录"经元数据核实的实际版本"(原子写);
                        // pip 成功但元数据不符的路径已归入失败,绝不污染可用版本钉
                        if !actual.is_empty() {
                            if let Err(e) = atomic_write_0600(&pin_path, &format!("{actual}\n")) {
                                eprintln!("[Supervisor] write version pin: {e}");
                            }
                        }
                        // 安装成功自动拉起内核(装完即用,无需手动重启)
                        (on_success)();
                    }
                }
                Err(e) => {
                    this.set(|g| {
                        g.running = false;
                        g.phase = Phase::Failed;
                        g.error = e.to_string();
                    });
                }
            }
        });
        true
    }
}

/// 用 venv 内 python 读已安装内核版本(importlib.metadata)
fn metadata_version(python: &Path) -> String {
    std::process::Command::new(python)
        .args(["-c", "import importlib.metadata as m; print(m.version('qwenpaw'))"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// 实际版本 vs 请求版本:PEP 440 允许前导 v 且大小写不敏感,其余按字面比较。
/// 不相等即失败 —— 宁可报错也不静默装错版本。
fn version_matches(actual: &str, want: &str) -> bool {
    let norm = |s: &str| s.trim().trim_start_matches(['v', 'V']).to_ascii_lowercase();
    norm(actual) == norm(want)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_version_rejects_shell_metacharacters() {
        for bad in [
            "1.0;rm -rf /",
            "1.0\n2.0",
            "1.0`id`",
            "1.0$(id)",
            "1.0|cat /etc/passwd",
            "1.0>~/x",
            "1.0&background",
            "1.0'quote",
            "1.0\"dq",
            "1.0\\x",
            "a b",
            "qwenpaw>=1.0",
            "qwenpaw~=1.0",
            "git+https://x.test/p.git",
            "https://x.test/pkg.whl",
            "/local/path.whl",
            "./relative.whl",
            "-pre",
            "--index-url",
            "",
        ] {
            assert!(
                InstallChannel::parse(bad).is_none(),
                "必须拒绝: {bad:?}"
            );
        }
        // 白名单字符集本身也要卡死元字符
        for bad in ["1.0;", "1.0|", "$(x)", "`x`", "a/b", "a\\b", "a b", "a~b"] {
            assert!(!is_safe_exact_version(bad), "必须拒绝: {bad:?}");
        }
    }

    #[test]
    fn channels_accept_legal_values() {
        assert!(matches!(
            InstallChannel::parse("latest"),
            Some(InstallChannel::Latest)
        ));
        assert!(matches!(
            InstallChannel::parse("beta"),
            Some(InstallChannel::Beta)
        ));
        assert!(matches!(
            InstallChannel::parse("pre"),
            Some(InstallChannel::Beta)
        ));
        assert!(matches!(
            InstallChannel::parse("2.3.0b2"),
            Some(InstallChannel::Exact(_))
        ));
        assert!(matches!(
            InstallChannel::parse("2.2.2.post1"),
            Some(InstallChannel::Exact(_))
        ));
        // PEP 440 本地版本段(+cpu)合法
        assert!(matches!(
            InstallChannel::parse("2.3.0b2+cpu"),
            Some(InstallChannel::Exact(_))
        ));
        assert!(matches!(
            InstallChannel::parse("1.0-1"),
            Some(InstallChannel::Exact(_))
        ));
    }

    #[test]
    fn pip_args_shape() {
        let a = pip_args(&InstallChannel::Latest, "");
        assert_eq!(a, vec!["install", "--upgrade", "qwenpaw"]);

        let a = pip_args(&InstallChannel::Beta, "");
        assert_eq!(a, vec!["install", "--upgrade", "--pre", "qwenpaw"]);

        let a = pip_args(&InstallChannel::Exact("2.3.0b2".into()), "");
        assert_eq!(a, vec!["install", "--upgrade", "qwenpaw==2.3.0b2"]);

        let a = pip_args(
            &InstallChannel::Latest,
            "https://pypi.tuna.tsinghua.edu.cn/simple",
        );
        assert_eq!(
            a,
            vec![
                "install",
                "--upgrade",
                "qwenpaw",
                "--index-url",
                "https://pypi.tuna.tsinghua.edu.cn/simple"
            ]
        );
    }

    #[test]
    fn mirror_sanitized() {
        assert_eq!(
            sanitize_mirror("https://pypi.tuna.tsinghua.edu.cn/simple"),
            "https://pypi.tuna.tsinghua.edu.cn/simple"
        );
        assert_eq!(sanitize_mirror("https://x.test/a;rm"), "");
        assert_eq!(sanitize_mirror("ftp://x.test"), "");
        assert_eq!(sanitize_mirror("x$(id)"), "");
    }

    #[test]
    fn version_pin_matches() {
        assert!(version_matches("2.3.0b2", "2.3.0b2"));
        assert!(version_matches("2.3.0B2", "2.3.0b2"));
        assert!(version_matches("2.3.0", "v2.3.0"));
        assert!(!version_matches("2.3.0", "2.3.1"));
        assert!(!version_matches("2.3.0b1", "2.3.0b2"));
    }
}
