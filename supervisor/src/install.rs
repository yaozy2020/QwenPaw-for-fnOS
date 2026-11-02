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
    Starting,
    Done,
    Failed,
    /// 任务在生效前被停止(停止/卸载路径的 cancel)。与 Failed 区分的意义:
    /// Failed 会触发自愈重启(main.rs on_change),Cancelled 绝不 —— 取消只
    /// 发生在关停流程中,自动拉起内核会对抗关停(P1-04)。
    Cancelled,
}

/// 线上 phase 字符串(与旧桥/控制台兼容):取消也归入 failed —— 控制台只认
/// done/failed 终态,"已被停止"的细节走 message/error 字段,不扩协议。
pub fn phase_wire(p: &Phase) -> &'static str {
    match p {
        Phase::Idle => "idle",
        Phase::Downloading => "downloading",
        Phase::Installing => "installing",
        Phase::Starting => "starting",
        Phase::Done => "done",
        Phase::Failed | Phase::Cancelled => "failed",
    }
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
            other if is_safe_exact_version(other) => Some(InstallChannel::Exact(other.to_string())),
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

pub fn truncate_utf8(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_string();
    }
    let end = raw
        .char_indices()
        .map(|(idx, _)| idx)
        .take_while(|&idx| idx <= max_bytes)
        .last()
        .unwrap_or(0);
    format!("{}…", &raw[..end])
}

/// 单行落盘(超限排水)。stdout/stderr 两个线程各自持有 append 句柄调用。
fn append_line(log: &mut Option<std::fs::File>, total: &Arc<Mutex<u64>>, raw: &str) {
    if let Some(f) = log.as_mut() {
        let mut t = total.lock().unwrap();
        if *t >= LOG_TOTAL_CAP {
            return;
        }
        let owned = truncate_utf8(raw, LOG_LINE_CAP);
        if f.write_all(owned.as_bytes()).is_ok() && f.write_all(b"\n").is_ok() {
            *t += owned.len() as u64 + 1;
        }
    }
}

/// 刷新看门狗活动时间戳(自任务启动起的毫秒数)
fn touch_activity(last: &AtomicU64, start: &std::time::Instant) {
    last.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
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

struct VenvTransaction {
    active: std::path::PathBuf,
    staged: std::path::PathBuf,
    backup: std::path::PathBuf,
}

impl VenvTransaction {
    fn prepare(active: &Path, seq: u64) -> std::io::Result<Self> {
        let python = std::env::var("QWENPAW_PYTHON_BIN")
            .unwrap_or_else(|_| "/var/apps/python312/target/bin/python3".into());
        Self::prepare_with_python(active, seq, Path::new(&python))
    }

    fn prepare_with_python(active: &Path, seq: u64, python: &Path) -> std::io::Result<Self> {
        let parent = active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;
        let suffix = format!("{}.{}", std::process::id(), seq);
        let staged = parent.join(format!(".venv.staging.{suffix}"));
        let backup = parent.join(format!(".venv.rollback.{suffix}"));
        let prepared = if active.exists() {
            copy_tree(active, &staged)
        } else {
            std::process::Command::new(python)
                .arg("-m")
                .arg("venv")
                .arg(&staged)
                .status()
                .and_then(|status| {
                    if status.success() {
                        Ok(())
                    } else {
                        Err(std::io::Error::other(format!("创建 venv 失败: {status}")))
                    }
                })
        };
        if let Err(error) = prepared {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(error);
        }
        Ok(Self { active: active.to_path_buf(), staged, backup })
    }

    fn python(&self) -> std::path::PathBuf {
        self.staged.join("bin/python3")
    }

    fn activate(&mut self) -> std::io::Result<()> {
        let had_active = self.active.exists();
        if had_active {
            std::fs::rename(&self.active, &self.backup)?;
        }
        if let Err(error) = std::fs::rename(&self.staged, &self.active) {
            if had_active {
                let _ = std::fs::rename(&self.backup, &self.active);
            }
            return Err(error);
        }
        Ok(())
    }

    fn rollback(&mut self) -> std::io::Result<()> {
        std::fs::rename(&self.active, &self.staged)?;
        if self.backup.exists() {
            if let Err(error) = std::fs::rename(&self.backup, &self.active) {
                let _ = std::fs::rename(&self.staged, &self.active);
                return Err(error);
            }
        }
        std::fs::remove_dir_all(&self.staged)
    }

    fn commit(self) -> std::io::Result<()> {
        if self.backup.exists() {
            std::fs::remove_dir_all(&self.backup)
        } else {
            Ok(())
        }
    }
}

impl Drop for VenvTransaction {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.staged);
    }
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return std::os::unix::fs::symlink(std::fs::read_link(source)?, destination);
    }
    if metadata.is_dir() {
        std::fs::create_dir(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(source, destination)?;
    }
    std::fs::set_permissions(destination, metadata.permissions())
}

#[derive(Clone)]
pub struct InstallState {
    inner: Arc<Mutex<Inner>>,
    on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
    on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    /// pip 启动器:生产即 Command::spawn;测试注入可阻塞 spawner,
    /// 精确复现"运行态已建立、PID 未登记"的取消窗口(P1-04)。
    spawner: Arc<dyn PipSpawner>,
}

/// pip 启动器抽象。
trait PipSpawner: Send + Sync {
    fn spawn(&self, cmd: &mut std::process::Command) -> std::io::Result<std::process::Child>;
}

struct RealSpawner;

impl PipSpawner for RealSpawner {
    fn spawn(&self, cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
        cmd.spawn()
    }
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
        on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    ) -> Self {
        Self::with_spawner(Arc::new(RealSpawner), on_change, on_success)
    }

    fn with_spawner(
        spawner: Arc<dyn PipSpawner>,
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
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
            spawner,
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
    /// 实际状态转换仍由安装线程在 wait 返回后完成(终态唯一写入者)。
    /// PID 复用窗口极小且组信号对无关组是 ESRCH 空操作,可接受(勿凭 PID 乱杀)。
    ///
    /// P1-04:任务已建立运行态但 pip 尚未 spawn(pid=0)时同样必须记录取消
    /// 意图 —— 否则停止/卸载调用 cancel 后,安装线程仍会拉起 pip 继续改 venv,
    /// 只能指望下次启动的孤儿清理。安装线程在 spawn 前后各核对一次该标志。
    pub fn cancel(&self) {
        let (running, pid) = {
            let g = self.inner.lock().unwrap();
            (g.running, g.pid)
        };
        if !running {
            return;
        }
        if pid == 0 {
            // 尚无进程可杀:先落取消标志,安装线程稍后自行中止/回收
            self.set(|g| g.cancelled = true);
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

    fn cancelled(&self) -> bool {
        self.inner.lock().unwrap().cancelled
    }

    /// 取消终态:不写版本钉、不自动拉起内核(区别于 Failed 的自愈重启)。
    fn finish_cancelled(&self) {
        self.set(|g| {
            g.running = false;
            g.pid = 0;
            g.phase = Phase::Cancelled;
            g.error = "安装已被停止".into();
        });
    }

    /// spawn 前核对取消标志;已取消则落终态并返回 true(调用方直接结束任务)。
    fn abort_if_cancelled(&self) -> bool {
        if self.cancelled() {
            self.finish_cancelled();
            true
        } else {
            false
        }
    }

    /// 运行 pip 安装(独立进程组 + 专用阻塞线程;终态唯一写入者是安装线程)。
    /// 返回是否成功启动(重复触发返回 false,对齐 dsh 的 busy 语义)。
    ///
    /// `pin_path` 是版本钉;成功(且元数据校验通过)后原子写入实际安装版本。
    pub fn try_start(
        &self,
        venv_dir: String,
        kernel_port: u16,
        channel: InstallChannel,
        mirror: String,
        pip_log: String,
        pin_path: String,
    ) -> bool {
        let task_seq = {
            let mut g = self.inner.lock().unwrap();
            if g.running {
                return false;
            }
            g.seq += 1;
            g.running = true;
            g.phase = Phase::Downloading;
            g.fetched = 0;
            g.message = "准备独立安装环境".into();
            g.error = String::new();
            g.pid = 0;
            g.cancelled = false;
            g.seq
        };
        self.emit(self.snapshot());

        let this = self.clone();
        let on_success = std::sync::Arc::clone(&self.on_success);
        std::thread::spawn(move || {
            if this.abort_if_cancelled() {
                return;
            }
            let mut transaction = match VenvTransaction::prepare(Path::new(&venv_dir), task_seq) {
                Ok(transaction) => transaction,
                Err(error) => {
                    this.set(|g| {
                        g.running = false;
                        g.phase = Phase::Failed;
                        g.error = format!("无法准备独立安装环境: {error}");
                    });
                    return;
                }
            };
            // pip 只修改暂存环境,原 venv 在完整健康检查前保持不变。
            let python = transaction.python();
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

            // spawn 前核对取消标志(P1-04):cancel 在 PID 登记前到达时,绝不拉起 pip
            if this.abort_if_cancelled() {
                return;
            }
            let spawned = this.spawner.spawn(&mut cmd);
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

                    // P1-04:spawn 与登记之间到达的取消在此落地 —— cancel 当时看到
                    // pid=0 只落了标志,这里复核后整组回收(组 TERM → 短宽限 → 组
                    // KILL),再走正常收尾(wait 立即返回,终态 Cancelled)
                    if this.cancelled() {
                        let cpgid = pid as i32;
                        unsafe { libc::kill(-cpgid, libc::SIGTERM) };
                        for _ in 0..8 {
                            if !crate::procfs::process_alive(cpgid) {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(250));
                        }
                        unsafe { libc::kill(-cpgid, libc::SIGKILL) };
                    }

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

                    if cancelled {
                        // 取消终态(P1-04):不写版本钉、不触发自愈重启,
                        // 与 Failed(真实 pip 失败,需要拉回旧内核)分流
                        this.finish_cancelled();
                        return;
                    }

                    // 终态前必须核实:venv 元数据里实际装到的版本
                    let mut ok = exit_ok;
                    let mut reason = if !timeout_reason.is_empty() {
                        timeout_reason
                    } else if let Some(e) = wait_err {
                        format!("pip 进程等待异常: {e}")
                    } else if !exit_ok {
                        let mut msg = "pip 安装失败,详见日志".to_string();
                        if let Ok(content) = std::fs::read_to_string(&pip_log) {
                            if let Some(err_line) = content.lines().rev().find(|l| {
                                l.contains("ERROR:")
                                    || l.contains("403 Forbidden")
                                    || l.contains("from versions: none")
                            }) {
                                msg = format!("pip 安装失败: {}", err_line.trim());
                            }
                        }
                        msg
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

                    if !ok {
                        this.set(|g| {
                            g.running = false;
                            g.phase = Phase::Failed;
                            g.error = reason;
                        });
                        return;
                    }

                    if let Err(error) = transaction.activate() {
                        this.set(|g| {
                            g.running = false;
                            g.phase = Phase::Failed;
                            g.error = format!("切换内核运行环境失败: {error}");
                        });
                        return;
                    }

                    this.set(|g| {
                        g.phase = Phase::Starting;
                        g.message = format!("内核包已安装 v{actual}，等待服务就绪");
                    });
                    let spawn_result = (on_success)();
                    let health_ok =
                        spawn_result.is_ok() && wait_for_version(kernel_port, &actual, 30);
                    if !health_ok {
                        let mut error = match spawn_result {
                            Err(e) => format!("内核启动失败 (spawn_failed): {e}"),
                            Ok(_) => {
                                "内核包已安装，但在宽限期内启动健康检查超时 (health_timeout)".into()
                            }
                        };
                        error = match transaction.rollback() {
                            Ok(()) => format!("{error}；已恢复原内核环境"),
                            Err(e) => format!("{error}；恢复原内核环境失败: {e}"),
                        };
                        this.set(|g| {
                            g.running = false;
                            g.phase = Phase::Failed;
                            g.error = error;
                        });
                        return;
                    }

                    if this.cancelled() {
                        let _ = transaction.rollback();
                        this.finish_cancelled();
                        return;
                    }

                    let pin_result = if actual.is_empty() {
                        Ok(())
                    } else {
                        atomic_write_0600(&pin_path, &format!("{actual}\n"))
                    };
                    match pin_result {
                        Ok(()) => {
                            let cleanup = transaction.commit();
                            this.set(|g| {
                                g.running = false;
                                g.phase = Phase::Done;
                                g.message = match cleanup {
                                    Ok(()) => format!("安装完成 v{actual}，服务已就绪"),
                                    Err(e) => format!("安装完成 v{actual}，服务已就绪；旧环境备份清理失败: {e}"),
                                };
                            });
                        }
                        Err(e) => {
                            let mut error = format!("版本钉写入失败 (pin_write_failed): {e}");
                            error = match transaction.rollback() {
                                Ok(()) => format!("{error}；已恢复原内核环境"),
                                Err(e) => format!("{error}；恢复原内核环境失败: {e}"),
                            };
                            this.set(|g| {
                                g.running = false;
                                g.phase = Phase::Failed;
                                g.error = error;
                            });
                        }
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

fn wait_for_version(port: u16, target_version: &str, attempts: usize) -> bool {
    use std::io::{Read, Write};
    for _ in 0..attempts {
        std::thread::sleep(Duration::from_millis(500));
        let Ok(mut stream) = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(500),
        ) else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let request = format!(
            "GET /api/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        if stream.write_all(request.as_bytes()).is_err() {
            continue;
        }
        let mut buf = Vec::new();
        if stream.read_to_end(&mut buf).is_err() {
            continue;
        }
        let response = String::from_utf8_lossy(&buf);
        if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
            continue;
        }
        let Some(body_idx) = response.find("\r\n\r\n") else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(&response[body_idx + 4..])
        else {
            continue;
        };
        if payload.get("version").and_then(|v| v.as_str()) == Some(target_version) {
            return true;
        }
    }
    false
}

/// 用 venv 内 python 读已安装内核版本(importlib.metadata)
fn metadata_version(python: &Path) -> String {
    std::process::Command::new(python)
        .args([
            "-c",
            "import importlib.metadata as m; print(m.version('qwenpaw'))",
        ])
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
            assert!(InstallChannel::parse(bad).is_none(), "必须拒绝: {bad:?}");
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
    fn utf8_truncation_preserves_character_boundaries() {
        let input = "汉".repeat(1001);
        assert_eq!(
            truncate_utf8(&input, 2000),
            format!("{}…", "汉".repeat(666))
        );
        assert_eq!(truncate_utf8("a😀b", 4), "a…");
        assert_eq!(truncate_utf8("短日志", 2000), "短日志");
    }

    #[test]
    fn health_check_uses_the_configured_port_and_exact_version() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let body = r#"{"version":"9.9.9"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        assert!(wait_for_version(port, "9.9.9", 1));
        server.join().unwrap();

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let body = r#"{"version":"9.9.90"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        assert!(!wait_for_version(port, "9.9.9", 1));
        server.join().unwrap();
    }

    #[test]
    fn version_pin_matches() {
        assert!(version_matches("2.3.0b2", "2.3.0b2"));
        assert!(version_matches("2.3.0B2", "2.3.0b2"));
        assert!(version_matches("2.3.0", "v2.3.0"));
        assert!(!version_matches("2.3.0", "2.3.1"));
        assert!(!version_matches("2.3.0b1", "2.3.0b2"));
    }

    #[test]
    fn cancelled_phase_maps_to_failed_on_wire() {
        // 线上协议不扩新终态:控制台只认 done/failed,"已被停止"走 message/error
        assert_eq!(phase_wire(&Phase::Cancelled), "failed");
        assert_eq!(phase_wire(&Phase::Failed), "failed");
        assert_eq!(phase_wire(&Phase::Done), "done");
        assert_eq!(phase_wire(&Phase::Idle), "idle");
        assert_eq!(phase_wire(&Phase::Downloading), "downloading");
        assert_eq!(phase_wire(&Phase::Installing), "installing");
        assert_eq!(phase_wire(&Phase::Starting), "starting");
    }

    type OnChangeCb = Arc<dyn Fn(InstallSnapshot) + Send + Sync>;
    type OnSuccessCb = Arc<dyn Fn() -> Result<i32, String> + Send + Sync>;
    fn noop_callbacks() -> (OnChangeCb, OnSuccessCb) {
        (Arc::new(|_: InstallSnapshot| {}), Arc::new(|| Ok(0)))
    }

    #[test]
    fn cancel_records_intent_when_pid_not_yet_registered() {
        // P1-04:running=true、pid=0(运行态已建立、pip 未 spawn)时,
        // cancel 必须记录取消意图,而不是静默返回
        let (oc, os) = noop_callbacks();
        let st = InstallState::new(oc, os);
        st.set(|g| {
            g.running = true;
            g.phase = Phase::Downloading;
        });
        st.cancel();
        assert!(
            st.inner.lock().unwrap().cancelled,
            "PID 未登记时取消意图必须被记录"
        );
    }

    #[test]
    fn pre_spawn_cancel_aborts_without_running_pip() {
        // spawn 前核对取消标志:直接落 Cancelled 终态,不拉起任何进程
        let (oc, os) = noop_callbacks();
        let st = InstallState::new(oc, os);
        st.set(|g| {
            g.running = true;
            g.cancelled = true;
        });
        assert!(st.abort_if_cancelled());
        let snap = st.snapshot();
        assert_eq!(snap.phase, Phase::Cancelled);
        assert!(!snap.running);
        // 未取消时不得误触发
        let (oc, os) = noop_callbacks();
        let st2 = InstallState::new(oc, os);
        st2.set(|g| g.running = true);
        assert!(!st2.abort_if_cancelled());
    }

    /// 可阻塞 spawner:进入 spawn 后挂起等待放行,测试在挂起期间调用 cancel(),
    /// 精确复现"运行态已建立、PID 未登记"的窗口(P1-04)
    struct GatedSpawner {
        entered: Arc<AtomicU64>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        spawned_pids: Arc<Mutex<Vec<u32>>>,
    }

    impl PipSpawner for GatedSpawner {
        fn spawn(&self, _cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
            use std::os::unix::process::CommandExt;
            self.entered.store(1, Ordering::SeqCst);
            let _ = self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(20));
            // 真实子进程替身 pip:独立进程组,存活期间可被组信号回收
            let mut c = std::process::Command::new("/bin/sleep");
            c.arg("30").process_group(0);
            let child = c.spawn()?;
            self.spawned_pids.lock().unwrap().push(child.id());
            Ok(child)
        }
    }

    fn fake_venv(test_name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qwenpaw-venv-{test_name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("venv/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let python = bin.join("python3");
        std::fs::write(
            &python,
            "#!/bin/bash\nif [ \"$1\" = \"-m\" ]; then\n  printf new > \"$(dirname \"$0\")/../installed\"\n  exit \"${FAKE_PIP_EXIT:-0}\"\nfi\nprintf '2.0.0\\n'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&python).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&python, permissions).unwrap();
        std::fs::write(dir.join("venv/installed"), "old").unwrap();
        dir
    }

    fn wait_for_terminal(st: &InstallState) -> InstallSnapshot {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while st.snapshot().running && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        st.snapshot()
    }

    #[test]
    fn missing_active_venv_creates_a_fresh_staged_environment() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-fresh-venv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let python = dir.join("base-python");
        std::fs::write(&python, "#!/bin/bash\nmkdir -p \"$3/bin\"\ncp \"$0\" \"$3/bin/python3\"\n").unwrap();
        let mut permissions = std::fs::metadata(&python).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&python, permissions).unwrap();
        let active = dir.join("venv");
        let mut transaction = VenvTransaction::prepare_with_python(&active, 1, &python).unwrap();
        assert!(transaction.python().exists());
        assert!(!active.exists(), "暂存提交前不得创建活动 venv");
        transaction.activate().unwrap();
        assert!(active.join("bin/python3").exists());
        transaction.commit().unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "提交后仅保留基础 Python 和活动 venv");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_health_check_restores_previous_venv_and_pin() {
        let dir = fake_venv("rollback");
        let venv = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();
        let st = InstallState::new(
            Arc::new(|_: InstallSnapshot| {}),
            Arc::new(|| Err("injected start failure".into())),
        );
        assert!(st.try_start(
            venv.to_string_lossy().into_owned(),
            18088,
            InstallChannel::Latest,
            String::new(),
            dir.join("pip.log").to_string_lossy().into_owned(),
            pin.to_string_lossy().into_owned(),
        ));
        let snapshot = wait_for_terminal(&st);
        assert_eq!(snapshot.phase, Phase::Failed);
        assert_eq!(std::fs::read_to_string(venv.join("installed")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(pin).unwrap(), "1.0.0\n");
        let entries = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(entries, 3, "暂存与备份目录必须清理: {entries}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn successful_install_commits_staged_venv_and_pin() {
        let dir = fake_venv("commit");
        let venv = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();
        let on_success: OnSuccessCb = Arc::new(|| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 18089))
                .map_err(|e| e.to_string())?;
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    let body = r#"{"version":"2.0.0"}"#;
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                }
            });
            Ok(1)
        });
        let st = InstallState::new(Arc::new(|_: InstallSnapshot| {}), on_success);
        assert!(st.try_start(
            venv.to_string_lossy().into_owned(),
            18089,
            InstallChannel::Latest,
            String::new(),
            dir.join("pip.log").to_string_lossy().into_owned(),
            pin.to_string_lossy().into_owned(),
        ));
        let snapshot = wait_for_terminal(&st);
        assert_eq!(snapshot.phase, Phase::Done, "{}", snapshot.error);
        assert_eq!(std::fs::read_to_string(venv.join("installed")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(pin).unwrap(), "2.0.0\n");
        let entries = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(entries, 3, "成功后不得残留暂存与备份目录: {entries}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_in_pid_registration_window_kills_pip_and_lands_cancelled() {
        let (tx, rx) = std::sync::mpsc::channel();
        let entered = Arc::new(AtomicU64::new(0));
        let pids: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let success_calls = Arc::new(AtomicU64::new(0));
        let sc = Arc::clone(&success_calls);
        let on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync> = Arc::new(move || {
            sc.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        });
        let st = InstallState::with_spawner(
            Arc::new(GatedSpawner {
                entered: Arc::clone(&entered),
                release: Mutex::new(rx),
                spawned_pids: Arc::clone(&pids),
            }),
            Arc::new(|_: InstallSnapshot| {}),
            on_success,
        );
        let dir = fake_venv("cancel-window");
        let pip_log = dir.join("pip.log").to_string_lossy().to_string();
        let pin = dir.join("pin").to_string_lossy().to_string();

        assert!(st.try_start(
            dir.join("venv").to_string_lossy().into_owned(),
            18088,
            InstallChannel::Latest,
            String::new(),
            pip_log.clone(),
            pin.clone(),
        ));
        // 等安装线程进入 spawn:此刻 running=true、pid=0(竞态窗口)
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while entered.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(entered.load(Ordering::SeqCst), 1, "安装线程未进入 spawn");
        // 窗口内取消:pid 尚未登记,取消意图必须先落标志
        st.cancel();
        tx.send(()).ok(); // 放行 spawn

        // 等终态:必须是 Cancelled(旧实现此处会一路跑成 Done 并触发 on_success)
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while st.snapshot().running && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let snap = st.snapshot();
        assert_eq!(snap.phase, Phase::Cancelled, "终态应为 Cancelled");
        assert!(!snap.running);

        // pip 替身(sleep 30)必须已被整组回收,而不是留成 30s 孤儿
        let spawned = pids.lock().unwrap().clone();
        assert!(!spawned.is_empty(), "替身 pip 未被 spawn");
        for p in &spawned {
            assert!(
                !crate::procfs::process_alive(*p as i32),
                "pip 替身 pid={} 在取消后仍存活",
                p
            );
        }
        // 不得写版本钉、不得触发安装成功自动拉起、install.pid 必须清理
        assert!(!std::path::Path::new(&pin).exists(), "取消任务不得写版本钉");
        assert_eq!(
            success_calls.load(Ordering::SeqCst),
            0,
            "取消不得触发 on_success"
        );
        let install_pid = std::path::Path::new(&pip_log)
            .parent()
            .unwrap()
            .join("install.pid");
        assert!(!install_pid.exists(), "install.pid 未清理");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
