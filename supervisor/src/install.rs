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
pub(crate) fn atomic_write_0600(path: &str, content: &str) -> std::io::Result<()> {
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


#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxPhase {
    /// 暂存环境已就绪,活动环境未动
    Prepared,
    /// 旧环境已改名为 backup,新环境尚未就位(活动路径此刻可能不存在)
    BackedUp,
    /// 新环境已激活到活动路径
    Activated,
    /// 新环境通过健康检查,版本钉尚未改写
    HealthChecked,
    /// 新环境、版本钉、旧备份清理全部完成
    Committed,
    /// 已回滚
    RolledBack,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VenvTxRecord {
    pub schema_version: u32,
    pub tx_id: String,
    pub phase: TxPhase,
    pub active: std::path::PathBuf,
    pub staged: std::path::PathBuf,
    pub backup: std::path::PathBuf,
    pub had_active: bool,
    pub target_version: String,
    pub pin_path: std::path::PathBuf,
    /// 健康检查通过时快照的旧版本钉原文。回滚时据此把钉写回,
    /// 消除“新钉已写、环境已回滚”的不一致窗口。
    #[serde(default)]
    pub previous_pin: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 故障注入中断点(仅用于测试与故障演练,不参与生产逻辑)
#[allow(clippy::enum_variant_names)]
pub enum CrashPoint {
    AfterPrepare,
    AfterActiveRenamedToBackup,
    AfterStagedActivated,
    AfterHealthChecked,
    AfterPinWritten,
    AfterCommit,
    /// 新内核已被拉起并通过健康检查,但事务尚未 commit —— 监督者此刻被杀,
    /// 会留下"进程是新版、磁盘是旧版"的组合,只有恢复阶段主动停进程才能纠正。
    AfterSpawnBeforeCommit,
}

static TEST_CRASH_POINT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[allow(dead_code)]
pub fn set_test_crash_point(cp: Option<CrashPoint>) {
    let val = match cp {
        None => 0,
        Some(CrashPoint::AfterPrepare) => 1,
        Some(CrashPoint::AfterActiveRenamedToBackup) => 2,
        Some(CrashPoint::AfterStagedActivated) => 3,
        Some(CrashPoint::AfterHealthChecked) => 4,
        Some(CrashPoint::AfterPinWritten) => 5,
        Some(CrashPoint::AfterCommit) => 6,
        Some(CrashPoint::AfterSpawnBeforeCommit) => 7,
    };
    TEST_CRASH_POINT.store(val, Ordering::SeqCst);
}

fn check_crash_point(point: CrashPoint) {
    let current_test = TEST_CRASH_POINT.load(Ordering::SeqCst);
    let matches_test = match point {
        CrashPoint::AfterPrepare => current_test == 1,
        CrashPoint::AfterActiveRenamedToBackup => current_test == 2,
        CrashPoint::AfterStagedActivated => current_test == 3,
        CrashPoint::AfterHealthChecked => current_test == 4,
        CrashPoint::AfterPinWritten => current_test == 5,
        CrashPoint::AfterCommit => current_test == 6,
        CrashPoint::AfterSpawnBeforeCommit => current_test == 7,
    };

    let env_name = match point {
        CrashPoint::AfterPrepare => "after_prepare",
        CrashPoint::AfterActiveRenamedToBackup => "after_rename_backup",
        CrashPoint::AfterStagedActivated => "after_activate",
        CrashPoint::AfterHealthChecked => "after_health_checked",
        CrashPoint::AfterPinWritten => "after_pin_written",
        CrashPoint::AfterCommit => "after_commit",
        CrashPoint::AfterSpawnBeforeCommit => "after_spawn_before_commit",
    };
    let matches_env = std::env::var("QWENPAW_CRASH_POINT")
        .map(|v| v.eq_ignore_ascii_case(env_name))
        .unwrap_or(false);

    if matches_test || matches_env {
        #[cfg(unix)]
        unsafe {
            libc::kill(libc::getpid(), libc::SIGKILL);
        }
        #[cfg(not(unix))]
        std::process::exit(137);
    }
}

pub fn sync_dir(dir: &Path) -> std::io::Result<()> {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.sync_all();
    }
    Ok(())
}

/// 由记录反推它所属的事务文件路径。
/// tx_id 即文件名的 pid.seq 部分,保证恢复阶段不必额外保存路径字段。
fn record_tx_path(record: &VenvTxRecord, parent: &Path) -> std::path::PathBuf {
    parent.join(format!(".venv.transaction.{}.json", record.tx_id))
}

fn write_tx_record(tx_file: &Path, record: &VenvTxRecord) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = tx_file.with_extension("tmp");
    let json_str = serde_json::to_string_pretty(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(json_str.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, tx_file)?;
    if let Some(p) = tx_file.parent() {
        let _ = sync_dir(p);
    }
    Ok(())
}
pub struct VenvTransaction {
    record: VenvTxRecord,
    tx_file: std::path::PathBuf,
    committed_or_rolled_back: bool,
}

impl VenvTransaction {
    pub fn prepare(active: &Path, seq: u64, target_version: &str, pin_path: &Path) -> std::io::Result<Self> {
        let python = std::env::var("QWENPAW_PYTHON_BIN")
            .unwrap_or_else(|_| "/var/apps/python312/target/bin/python3".into());
        Self::prepare_with_python(active, seq, target_version, pin_path, Path::new(&python))
    }

    pub fn prepare_with_python(
        active: &Path,
        seq: u64,
        target_version: &str,
        pin_path: &Path,
        python: &Path,
    ) -> std::io::Result<Self> {
        let parent = active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;
        let suffix = format!("{}.{}", std::process::id(), seq);
        let staged = parent.join(format!(".venv.staging.{suffix}"));
        let backup = parent.join(format!(".venv.rollback.{suffix}"));
        let tx_file = parent.join(format!(".venv.transaction.{suffix}.json"));
        let had_active = active.exists();

        let prepared = if had_active {
            copy_tree(active, &staged)
        } else {
            let mut cmd = std::process::Command::new(python);
            cmd.arg("-m").arg("venv").arg(&staged);
            run_with_timeout(cmd, std::time::Duration::from_secs(180), "创建 venv")
                .and_then(|o| {
                    if o.status.success() {
                        Ok(())
                    } else {
                        Err(std::io::Error::other(format!(
                            "创建 venv 失败: {}",
                            String::from_utf8_lossy(&o.stderr).trim()
                        )))
                    }
                })
        };
        if let Err(error) = prepared {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(error);
        }

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix,
            phase: TxPhase::Prepared,
            active: active.to_path_buf(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active,
            target_version: target_version.to_string(),
            pin_path: pin_path.to_path_buf(),
            previous_pin: None,
        };

        if let Err(e) = write_tx_record(&tx_file, &record) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(e);
        }
        check_crash_point(CrashPoint::AfterPrepare);

        Ok(Self {
            record,
            tx_file,
            committed_or_rolled_back: false,
        })
    }

    pub fn python(&self) -> std::path::PathBuf {
        self.record.staged.join("bin/python3")
    }

    pub fn activate(&mut self) -> std::io::Result<()> {
        let parent = self.record.active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;

        if self.record.had_active && self.record.active.exists() {
            std::fs::rename(&self.record.active, &self.record.backup)?;
            let _ = sync_dir(parent);
            // 关键:改名之后、激活之前必须把“已切走旧环境”落盘。
            // 否则此刻崩溃,事务仍是 Prepared,恢复会当作“原环境未动”而删掉唯一可用的备份。
            self.record.phase = TxPhase::BackedUp;
            write_tx_record(&self.tx_file, &self.record)?;
            check_crash_point(CrashPoint::AfterActiveRenamedToBackup);
        }

        if let Err(error) = std::fs::rename(&self.record.staged, &self.record.active) {
            if self.record.had_active && self.record.backup.exists() {
                let _ = std::fs::rename(&self.record.backup, &self.record.active);
                let _ = sync_dir(parent);
            }
            return Err(error);
        }
        let _ = sync_dir(parent);

        self.record.phase = TxPhase::Activated;
        write_tx_record(&self.tx_file, &self.record)?;
        check_crash_point(CrashPoint::AfterStagedActivated);
        Ok(())
    }

    /// 运行时回滚。与崩溃恢复共用 rollback_record,保证两条路径口径一致。
    pub fn rollback(&mut self) -> std::io::Result<()> {
        let parent = self.record.active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;

        rollback_record(&self.record, parent, &self.record.pin_path, None);

        self.record.phase = TxPhase::RolledBack;
        let _ = std::fs::remove_file(&self.tx_file);
        let _ = sync_dir(parent);
        self.committed_or_rolled_back = true;
        Ok(())
    }

    /// 健康检查通过。此时把旧版本钉内容快照进事务记录,
    /// 让“新环境已激活、版本钉尚未生效”的窗口里也能把旧钉原样写回。
    pub fn health_checked(&mut self) -> std::io::Result<()> {
        self.record.previous_pin = match std::fs::read_to_string(&self.record.pin_path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        self.record.phase = TxPhase::HealthChecked;
        write_tx_record(&self.tx_file, &self.record)?;
        check_crash_point(CrashPoint::AfterHealthChecked);
        Ok(())
    }

    /// 在事务约束下改写版本钉:先落盘 Committed,再写钉。
    /// 顺序不能反 —— 写完钉但未落 Committed 时崩溃,恢复会回滚环境却以为钉还是旧的。
    pub fn commit(&mut self, pin_content: Option<&str>) -> std::io::Result<()> {
        let parent = self.record.active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;

        self.record.phase = TxPhase::Committed;
        write_tx_record(&self.tx_file, &self.record)?;

        // Committed 落盘之后才允许改写版本钉。此后崩溃只会留下“新环境+新钉”,
        // 恢复走 Committed 分支保留二者,不再回滚。
        if let Some(content) = pin_content {
            if let Err(e) = atomic_write_0600(&self.record.pin_path.to_string_lossy(), content) {
                // 钉写失败说明新环境无法被后续流程稳定复现,必须回滚,
                // 并把 Committed 退回 HealthChecked,让恢复与运行时口径一致
                self.record.phase = TxPhase::HealthChecked;
                let _ = write_tx_record(&self.tx_file, &self.record);
                return Err(e);
            }
        }
        check_crash_point(CrashPoint::AfterPinWritten);

        if self.record.backup.exists() {
            let _ = std::fs::remove_dir_all(&self.record.backup);
            let _ = sync_dir(parent);
        }
        if self.record.staged.exists() {
            let _ = std::fs::remove_dir_all(&self.record.staged);
            let _ = sync_dir(parent);
        }

        check_crash_point(CrashPoint::AfterCommit);
        let _ = std::fs::remove_file(&self.tx_file);
        let _ = sync_dir(parent);
        self.committed_or_rolled_back = true;
        Ok(())
    }
}

impl Drop for VenvTransaction {
    fn drop(&mut self) {
        if !self.committed_or_rolled_back {
            let _ = std::fs::remove_dir_all(&self.record.staged);
        }
    }
}

/// 回滚一个尚未 Committed 的事务,语义由“磁盘目录现状 + 是否曾有旧环境”共同决定。
/// 崩溃恢复与运行时失败回滚共用此实现,避免两条路径各自演化出不同的恢复口径。
///
/// 关键约束:回滚结束时活动路径必须存在且为旧环境(或首次安装时彻底不存在),
/// 且版本钉必须与活动环境一致。任何情况下都不得删除唯一可用的旧环境。
fn rollback_record(
    record: &VenvTxRecord,
    parent: &Path,
    pin_path: &Path,
    log: Option<&dyn Fn(&str)>,
) {
    if record.had_active {
        // 1. 活动路径当前若存在,它要么是新环境(Activated/HealthChecked),
        //    要么是没被切走的旧环境(backup 不存在)。只有前者可以删。
        if record.backup.exists() {
            if record.active.exists() {
                let _ = std::fs::remove_dir_all(&record.active);
                let _ = sync_dir(parent);
            }
            // 2. 旧环境归位
            if let Err(e) = std::fs::rename(&record.backup, &record.active) {
                if let Some(logf) = log {
                    logf(&format!(
                        "rollback: 旧环境归位失败 {} -> {}: {e}",
                        record.backup.display(),
                        record.active.display()
                    ));
                }
            }
            let _ = sync_dir(parent);
        } else if !record.active.exists() {
            // backup 与新环境都不存在,活动环境也丢了 —— 已是既成损坏,如实记录
            if let Some(logf) = log {
                logf(&format!(
                    "rollback: 事务 {} 未找到可用旧环境(active/backup 均缺失)",
                    record.tx_id
                ));
            }
        }
        // 环境回滚后,版本钉必须回到旧值,否则会留下“旧内核 + 新版本钉”
        restore_previous_pin(record, log);
    } else {
        // 首次安装没有旧环境:回滚后保持“未安装”,不留空版本钉
        if record.active.exists() {
            let _ = std::fs::remove_dir_all(&record.active);
            let _ = sync_dir(parent);
        }
        for pin in [record.pin_path.as_path(), pin_path] {
            if pin.exists() {
                let _ = std::fs::remove_file(pin);
                if let Some(p) = pin.parent() {
                    let _ = sync_dir(p);
                }
            }
        }
    }

    // 3. 无论哪条分支,暂存与备份残留都必须清掉
    if record.staged.exists() {
        let _ = std::fs::remove_dir_all(&record.staged);
        let _ = sync_dir(parent);
    }
    if record.backup.exists() {
        let _ = std::fs::remove_dir_all(&record.backup);
        let _ = sync_dir(parent);
    }
}

/// 把版本钉恢复到事务开始前的状态。
/// - 旧环境存在且记录中留有旧钉原文 → 原样写回(含“原本没有钉”的删除语义)
/// - 从未记录过快照(崩在 health_checked 之前) → 版本钉从未被本事务改写,无需处理
fn restore_previous_pin(record: &VenvTxRecord, log: Option<&dyn Fn(&str)>) {
    let pin_path = record.pin_path.to_string_lossy().to_string();
    match record.previous_pin.as_deref() {
        Some(prev) => {
            if let Err(e) = atomic_write_0600(&pin_path, prev) {
                if let Some(logf) = log {
                    logf(&format!("rollback: 旧版本钉恢复失败 {pin_path}: {e}"));
                }
            }
        }
        None if record.phase == TxPhase::HealthChecked && record.pin_path.exists() => {
            // 快照明确为“无钉”:删掉可能已被写入的新钉
            let _ = std::fs::remove_file(&record.pin_path);
            if let Some(p) = record.pin_path.parent() {
                let _ = sync_dir(p);
            }
        }
        None => {}
    }
}

/// 监督者启动恢复:恢复未完成事务(崩溃/SIGKILL/断电自愈)。
///
/// `stop_kernel` 在"需要回滚的事务"出现时被调用一次,用于停掉可能由该事务
/// 拉起的内核进程。崩溃恢复只回滚磁盘是不够的:健康检查窗口内监督者被杀时,
/// 新内核仍存活,随后收养会得到"进程新版 + 磁盘旧版"的错配 ——
/// 运行中进程的依赖目录已被回滚抽走,只能靠重启才恢复。
pub fn recover_pending_transactions(
    venv_dir: &Path,
    pin_path: &Path,
    log: Option<&dyn Fn(&str)>,
    stop_kernel: Option<&dyn Fn()>,
) -> std::io::Result<usize> {
    let parent = match venv_dir.parent() {
        Some(p) if p.exists() => p,
        _ => return Ok(0),
    };
    let mut recovered_count = 0;
    let read_dir = match std::fs::read_dir(parent) {
        Ok(rd) => rd,
        Err(_) => return Ok(0),
    };

    let mut tx_files = Vec::new();
    let mut orphan_stagings = Vec::new();
    let mut orphan_uninstalls = Vec::new();
    let mut candidate_rollbacks = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(".venv.transaction.") && name_str.ends_with(".json") {
            tx_files.push(entry.path());
        } else if name_str.starts_with(".venv.staging.") {
            orphan_stagings.push(entry.path());
        } else if name_str.starts_with(".venv.uninstall.") {
            orphan_uninstalls.push(entry.path());
        } else if name_str.starts_with(".venv.rollback.") {
            candidate_rollbacks.push(entry.path());
        }
    }

    tx_files.sort();

    // 先把能解析的记录全部收集到内存,供后面的孤儿清理做引用判断。
    // 关键:不能等到清理阶段再从磁盘重读 —— 那时 tx 文件已被本循环消费掉,
    // 读取恒失败,引用检查会静默失效并把在用的 backup 当垃圾删掉。
    let mut parsed_records: Vec<VenvTxRecord> = Vec::new();
    let mut unreadable_tx = false;

    for tx_path in &tx_files {
        let content = match std::fs::read_to_string(tx_path) {
            Ok(c) => c,
            Err(_) => {
                // 读不出来时不敢断言它没引用某个 backup,fail-closed
                unreadable_tx = true;
                let _ = std::fs::remove_file(tx_path);
                continue;
            }
        };
        let record: VenvTxRecord = match serde_json::from_str(&content) {
            Ok(r) => r,
            Err(_) => {
                unreadable_tx = true;
                let _ = std::fs::remove_file(tx_path);
                continue;
            }
        };
        parsed_records.push(record);
    }

    for record in &parsed_records {
        let tx_path = &record_tx_path(record, parent);
        if !tx_path.exists() {
            continue;
        }

        if let Some(logf) = log {
            logf(&format!(
                "recovering venv transaction id={} phase={:?}",
                record.tx_id, record.phase
            ));
        }

        match record.phase {
            // Prepared 有两条可能的历史:
            //  a) 旧环境从未被动过 —— 删暂存即可,active/backup 都不碰;
            //  b) rename(active→backup) 已执行,但 BackedUp 尚未落盘就断电 ——
            //     此刻 active 缺失、backup 存在。必须把 backup 归位,
            //     否则它就是唯一的旧环境副本,删了就彻底丢失。
            // 用磁盘现状而非阶段字段判定,才能覆盖 b 这个更窄的窗口。
            TxPhase::Prepared => {
                if record.had_active && !record.active.exists() && record.backup.exists() {
                    let _ = std::fs::rename(&record.backup, &record.active);
                    let _ = sync_dir(parent);
                    if let Some(logf) = log {
                        logf(&format!(
                            "recover: 事务 {} 崩在 rename 与 BackedUp 落盘之间,已把旧环境备份归位",
                            record.tx_id
                        ));
                    }
                }
                if record.staged.exists() {
                    let _ = std::fs::remove_dir_all(&record.staged);
                    let _ = sync_dir(parent);
                }
                let _ = std::fs::remove_file(tx_path);
                let _ = sync_dir(parent);
            }
            // BackedUp / Activated / HealthChecked：新环境尚未 Commit,一律回滚到旧环境。
            // 三者共用同一回滚语义,差别只在“活动路径当前是谁”,由目录现状判定。
            TxPhase::BackedUp | TxPhase::Activated | TxPhase::HealthChecked => {
                // 先停进程再回滚磁盘:顺序反了会让运行中的内核瞬间失去依赖目录
                if let Some(stop) = stop_kernel {
                    if let Some(logf) = log {
                        logf(&format!(
                            "recovery: 停止由中断安装可能拉起的内核 (tx {})",
                            record.tx_id
                        ));
                    }
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(stop));
                }
                rollback_record(record, parent, pin_path, log);
                let _ = std::fs::remove_file(tx_path);
                let _ = sync_dir(parent);
            }
            TxPhase::Committed => {
                // Committed：保留新环境和版本钉，只清理旧备份
                if record.backup.exists() {
                    let _ = std::fs::remove_dir_all(&record.backup);
                    let _ = sync_dir(parent);
                }
                if record.staged.exists() {
                    let _ = std::fs::remove_dir_all(&record.staged);
                    let _ = sync_dir(parent);
                }
                let _ = std::fs::remove_file(tx_path);
                let _ = sync_dir(parent);
            }
            TxPhase::RolledBack => {
                if record.staged.exists() {
                    let _ = std::fs::remove_dir_all(&record.staged);
                }
                if record.backup.exists() {
                    let _ = std::fs::remove_dir_all(&record.backup);
                }
                let _ = std::fs::remove_file(tx_path);
                let _ = sync_dir(parent);
            }
        }
        recovered_count += 1;
    }

    // 清理历史残留暂存目录与卸载垃圾 (P2 启动自愈治理)
    for staging_path in orphan_stagings {
        if staging_path.exists() {
            let _ = std::fs::remove_dir_all(&staging_path);
            let _ = sync_dir(parent);
        }
    }
    for uninstall_path in orphan_uninstalls {
        if uninstall_path.exists() {
            let _ = std::fs::remove_dir_all(&uninstall_path);
            let _ = sync_dir(parent);
        }
    }
    // 备份目录只在“确实没有任何事务引用它”时才可视为垃圾。
    // 有记录引用的绝不代删 —— 那是唯一的旧环境。
    // 判定依据是本次已解析到内存的记录(而非回读已被消费的 tx 文件),
    // 且 fail-closed:只要存在读不出来的 tx 文件,就整体跳过清理,
    // 宁可留一堆垃圾,也不能删掉可能是唯一副本的备份。
    if unreadable_tx {
        if let Some(logf) = log {
            logf("recover: 存在无法解析的事务文件,跳过孤儿备份清理(fail-closed)");
        }
    } else {
        for rollback_path in candidate_rollbacks {
            if !rollback_path.exists() {
                continue;
            }
            let referenced = parsed_records.iter().any(|r| r.backup == rollback_path);
            if referenced {
                continue;
            }
            let _ = std::fs::remove_dir_all(&rollback_path);
            let _ = sync_dir(parent);
        }
    }

    // 清理 var 目录下的残留临时文件 (如 .qwenpaw-version.uninstall.* 等)
    if let Some(var_dir) = pin_path.parent() {
        if var_dir.exists() {
            if let Ok(rd) = std::fs::read_dir(var_dir) {
                for entry in rd.flatten() {
                    let n = entry.file_name();
                    let s = n.to_string_lossy();
                    if s.starts_with(".qwenpaw-version.uninstall.") || s == ".qwenpaw-version.tmp" {
                        let _ = std::fs::remove_file(entry.path());
                        let _ = sync_dir(var_dir);
                    }
                }
            }
        }
    }

    Ok(recovered_count)
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    let ft = metadata.file_type();
    if ft.is_symlink() {
        return std::os::unix::fs::symlink(std::fs::read_link(source)?, destination);
    }
    if ft.is_dir() {
        std::fs::create_dir(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if ft.is_file() {
        std::fs::copy(source, destination)?;
    } else {
        // FIFO / socket / 设备节点绝不能走 std::fs::copy:
        // 打开 FIFO 会阻塞到有写端为止,一次 venv 里出现 FIFO 就能让安装线程永久挂死,
        // running 恒 true、/kernel/install 永久 busy。直接失败比挂死好。
        return Err(std::io::Error::other(format!(
            "暂存复制遇到非常规文件,拒绝处理: {}",
            source.display()
        )));
    }
    std::fs::set_permissions(destination, metadata.permissions())
}

#[derive(Clone)]
pub struct InstallState {
    inner: Arc<Mutex<Inner>>,
    on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
    on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    /// 停止内核。取消或回滚时必须真正停掉刚拉起的实例,
    /// 否则运行中的进程其磁盘路径会被回滚抽掉。
    on_stop: Arc<dyn Fn() + Send + Sync>,
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
    /// 无停止回调的构造(测试与控制台状态快照用)。生产路径用 with_stop。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
    ) -> Self {
        Self::with_spawner(Arc::new(RealSpawner), on_change, on_success)
    }

    /// 带自定义停止回调的构造:生产路径用它,保证取消/回滚前先停内核。
    pub fn with_stop(
        on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
        on_success: Arc<dyn Fn() -> Result<i32, String> + Send + Sync>,
        on_stop: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let mut me = Self::with_spawner(Arc::new(RealSpawner), on_change, on_success);
        me.on_stop = on_stop;
        me
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
            on_stop: Arc::new(|| {}),
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
        let on_stop = std::sync::Arc::clone(&self.on_stop);
        let guard = self.clone();
        std::thread::spawn(move || {
            // 一次意外 panic(例如 FIFO/设备文件进入 copy_tree)会让 running 永久为 true,
            // /kernel/install 从此永久 busy 直到重启。用 catch_unwind 兜住并落 Failed。
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if this.abort_if_cancelled() {
                return;
            }
            let target_hint = match &channel {
                InstallChannel::Exact(v) => v.as_str(),
                _ => "",
            };
            let mut transaction = match VenvTransaction::prepare(
                Path::new(&venv_dir),
                task_seq,
                target_hint,
                Path::new(&pin_path),
            ) {
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
                    let _ = atomic_write_0600(&pid_file.to_string_lossy(), &format!("{pid}\n"));

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
                        {
                            let stop = Arc::clone(&on_stop);
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stop()));
                        }
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

                    // 新内核此时已拉起并探活成功,但事务仍是 Activated。
                    // 这里被杀会留下"进程新版 + 磁盘旧版",由恢复阶段停进程纠正。
                    check_crash_point(CrashPoint::AfterSpawnBeforeCommit);

                    // 健康检查通过,落盘 HealthChecked(同时快照旧版本钉)。
                    // 落盘失败意味着无法保证之后可回滚,必须中止而不是继续往前走。
                    if let Err(e) = transaction.health_checked() {
                        let mut error = format!("安装状态落盘失败 (tx_persist_failed): {e}");
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
                        // 新内核已在运行(健康检查已通过)。必须先停掉它,
                        // 否则回滚把磁盘上的活动环境换回旧版本,而运行中的进程
                        // 仍指向被抽掉的路径。
                        let stop = Arc::clone(&on_stop);
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stop()));
                        let _ = transaction.rollback();
                        this.finish_cancelled();
                        return;
                    }

                    // 版本钉改写收进事务:先落 Committed 再写钉
                    let pin_content = if actual.is_empty() {
                        None
                    } else {
                        Some(format!("{actual}\n"))
                    };
                    match transaction.commit(pin_content.as_deref()) {
                        Ok(()) => {
                            this.set(|g| {
                                g.running = false;
                                g.phase = Phase::Done;
                                g.message = if actual.is_empty() {
                                    "安装完成，服务已就绪".to_string()
                                } else {
                                    format!("安装完成 v{actual}，服务已就绪")
                                };
                            });
                        }
                        Err(e) => {
                            // commit 失败(含版本钉写入失败)一律回滚并报 Failed,
                            // 绝不向前端报 Done
                            let mut error = format!("安装提交失败 (commit_failed): {e}");
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
            }));

            if let Err(panic) = outcome {
                let detail = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                guard.set(|g| {
                    // 只有仍处于 running 的本次任务才改写,避免覆盖后续任务状态
                    if g.running {
                        g.running = false;
                        g.phase = Phase::Failed;
                        g.error = format!("安装线程异常终止 (panic): {detail}");
                    }
                });
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

/// 带超时地运行一个短命令,超时则杀掉并返回错误。
///
/// `python -m venv` 与 importlib.metadata 查询都不在上面那个 pip 看门狗覆盖范围内:
/// 一旦挂起(venv 创建卡在锁上、解释器启动卡在 NFS 上),安装线程会永久阻塞,
/// running 恒 true、/kernel/install 永久 busy —— 正是看门狗注释声称要消灭的模式。
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    label: &str,
) -> std::io::Result<std::process::Output> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn()?;
    let started = std::time::Instant::now();

    loop {
        match child.try_wait()? {
            Some(_) => return child.wait_with_output(),
            None => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("{label} 超时({}s),已终止", timeout.as_secs()),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// 用 venv 内 python 读已安装内核版本(importlib.metadata)
fn metadata_version(python: &Path) -> String {
    let mut cmd = std::process::Command::new(python);
    cmd.args([
        "-c",
        "import importlib.metadata as m; print(m.version('qwenpaw'))",
    ]);
    run_with_timeout(cmd, std::time::Duration::from_secs(30), "读取内核版本")
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
        let pin = dir.join("pin");
        let mut transaction = VenvTransaction::prepare_with_python(&active, 1, "2.0.0", &pin, &python).unwrap();
        assert!(transaction.python().exists());
        assert!(!active.exists(), "暂存提交前不得创建活动 venv");
        transaction.activate().unwrap();
        assert!(active.join("bin/python3").exists());
        transaction.commit(None).unwrap();
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

    #[test]
    fn recover_prepared_cleans_staging_and_leaves_active_untouched() {
        let dir = fake_venv("rec-prepared");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        // 模拟执行到 AfterPrepare 并崩溃
        let suffix = "9999.1";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("marker"), "staging-data").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Prepared,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered, 1);
        assert!(!staged.exists(), "Prepared 的暂存目录必须被清理");
        assert!(!tx_file.exists(), "事务文件必须被清理");
        assert_eq!(std::fs::read_to_string(active.join("installed")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_activated_rolls_back_to_previous_active_env() {
        let dir = fake_venv("rec-activated");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.2";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        // 模拟已激活状态:老环境被改名为 backup,新环境已占据 active
        std::fs::rename(&active, &backup).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("installed"), "new-halfway").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Activated,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered, 1);
        assert_eq!(std::fs::read_to_string(active.join("installed")).unwrap(), "old", "必须回滚为旧环境");
        assert!(!backup.exists(), "回滚后 backup 目录不残留");
        assert!(!tx_file.exists(), "事务文件已清理");
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_activated_fresh_install_removes_active_and_leaves_uninstalled() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-fresh-rollback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let active = dir.join("venv");
        let pin = dir.join("pin");

        let suffix = "9999.3";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        // 首次安装没有旧环境(had_active: false)，模拟激活后被 kill
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("installed"), "fresh-unhealthy").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Activated,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: false,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered, 1);
        assert!(!active.exists(), "首次安装回滚后不得保留活动环境");
        assert!(!pin.exists(), "首次安装回滚后不得留有版本钉(保持未安装)");
        assert!(!tx_file.exists(), "事务文件已清理");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_health_checked_rolls_back_conservatively() {
        let dir = fake_venv("rec-healthchecked");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.4";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        // 健康检查后、版本钉写入前断电:新环境已就位,版本钉仍是旧值(快照已落盘)
        std::fs::rename(&active, &backup).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("installed"), "new-checked").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::HealthChecked,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: Some("1.0.0\n".into()),
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered, 1);
        assert_eq!(std::fs::read_to_string(active.join("installed")).unwrap(), "old", "保守路径必须回滚原环境");
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n");
        assert!(!backup.exists());
        assert!(!tx_file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_committed_preserves_new_env_and_cleans_backup() {
        let dir = fake_venv("rec-committed");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "2.0.0\n").unwrap();

        let suffix = "9999.5";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        // 版本钉写入后、备份清理前断电:保留新环境和版本钉，只清理旧备份
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("old-data"), "old").unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("installed"), "new-committed").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Committed,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered, 1);
        assert_eq!(std::fs::read_to_string(active.join("installed")).unwrap(), "new-committed", "新环境必须保留");
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "2.0.0\n", "新版本钉必须保留");
        assert!(!backup.exists(), "旧备份必须清理");
        assert!(!tx_file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_staging_directories_are_cleaned_on_recovery() {
        let dir = fake_venv("rec-orphan");
        let active = dir.join("venv");
        let pin = dir.join("pin");

        let orphan1 = dir.join(".venv.staging.orphan-1");
        let orphan2 = dir.join(".venv.staging.orphan-2");
        std::fs::create_dir_all(&orphan1).unwrap();
        std::fs::create_dir_all(&orphan2).unwrap();

        let _ = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert!(!orphan1.exists(), "孤儿 staging-1 必须被清理");
        assert!(!orphan2.exists(), "孤儿 staging-2 必须被清理");
        let _ = std::fs::remove_dir_all(&dir);
    }



    // ── P0 回归:旧环境已改名、新环境尚未激活(BackedUp 阶段)时崩溃 ──
    // 曾经的缺陷:该窗口磁盘记录仍是 Prepared,恢复按"原环境未动"处理,
    // 只删 staging 与事务文件,随后把唯一可用的旧环境备份当孤儿 rollback 删除,
    // 结果活动 venv 消失、版本钉却残留。
    #[test]
    fn recover_backed_up_restores_previous_environment() {
        let dir = fake_venv("rec-backedup");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.6";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        // 旧环境已改名为 backup,活动路径此刻不存在(staging 尚未改名过去)
        std::fs::rename(&active, &backup).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("installed"), "new-staged").unwrap();
        assert!(!active.exists(), "该中断点活动路径本就不存在");

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::BackedUp,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered, 1);
        assert!(active.exists(), "活动环境必须被恢复,绝不允许消失");
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old",
            "必须恢复为旧环境内容"
        );
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n", "版本钉必须保持旧值");
        assert!(!backup.exists(), "备份不得残留");
        assert!(!staged.exists(), "暂存不得残留");
        assert!(!tx_file.exists(), "事务文件必须消费");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── P0 回归:孤儿 rollback 清理不得误删仍被未决事务引用的备份 ──
    #[test]
    fn orphan_rollback_cleanup_skips_backups_referenced_by_live_transaction() {
        let dir = fake_venv("rec-live-ref");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.7";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));
        std::fs::rename(&active, &backup).unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::BackedUp,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert!(active.exists(), "被事务引用的备份必须归位,而不是当孤儿删掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── P1 回归:回滚必须把版本钉写回旧值 ──
    #[test]
    fn rollback_restores_previous_pin_snapshot() {
        let dir = fake_venv("rec-pin-snapshot");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "2.0.0\n").unwrap();

        let suffix = "9999.8";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));
        std::fs::rename(&active, &backup).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("installed"), "new").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::HealthChecked,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: Some("1.0.0\n".into()),
        };
        write_tx_record(&tx_file, &record).unwrap();

        recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&pin).unwrap(),
            "1.0.0\n",
            "环境回滚后版本钉必须同步回到旧值,不允许出现'旧内核+新钉'"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── P1 回归:commit 失败必须返回 Err,不得静默报成功 ──
    #[test]
    fn commit_propagates_pin_write_failure() {
        let dir = fake_venv("commit-pin-fail");
        let active = dir.join("venv");
        // 把 pin 指向一个不存在的目录,令写入必然失败
        let pin = dir.join("no-such-dir").join("pin");

        let mut transaction =
            VenvTransaction::prepare_with_python(&active, 9, "2.0.0", &pin, Path::new("/bin/true"))
                .unwrap();
        transaction.activate().unwrap();
        transaction.health_checked().unwrap();
        let result = transaction.commit(Some("2.0.0\n"));
        assert!(result.is_err(), "版本钉写入失败必须向上传播,不能报 Done");
        let _ = std::fs::remove_dir_all(&dir);
    }


    // ── 验收保留意见回归:崩在 rename 与 BackedUp 落盘之间 ──
    // 该窗口下磁盘记录仍是 Prepared,但 active 已缺失、backup 已存在。
    // 曾经的 Prepared 分支只删 staging,随后孤儿清理把 backup 当垃圾删掉,
    // 结果活动环境与唯一备份同时消失。
    #[test]
    fn recover_prepared_with_missing_active_restores_backup() {
        let dir = fake_venv("rec-narrow-window");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.10";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        // rename 已执行,BackedUp 尚未落盘:active 消失,backup 持有旧环境
        std::fs::rename(&active, &backup).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        assert!(!active.exists() && backup.exists());

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Prepared,   // 关键:仍是 Prepared
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert!(active.exists(), "活动环境必须恢复,不得与备份一起消失");
        assert_eq!(std::fs::read_to_string(active.join("installed")).unwrap(), "old");
        assert!(!backup.exists(), "备份归位后不得残留");
        assert!(!staged.exists(), "暂存必须清理");
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── 验收保留意见回归:引用检查不得失效 ──
    // 主循环先消费 tx 文件,若清理阶段再从磁盘回读来判断引用,读取恒失败,
    // fail-open 会把在用的 backup 当垃圾删掉。
    #[test]
    fn orphan_cleanup_uses_in_memory_records_not_reread_files() {
        let dir = fake_venv("rec-ref-memory");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.11";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        std::fs::rename(&active, &backup).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("installed"), "new").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Activated,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old",
            "备份必须归位;若被当孤儿删除则活动环境会保留新版本"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }


    // ── 工单2 回归:崩溃恢复必须先停进程再回滚磁盘 ──
    #[test]
    fn recovery_stops_kernel_before_rolling_back() {
        let dir = fake_venv("rec-stop-first");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.12";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));

        std::fs::rename(&active, &backup).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        std::fs::write(active.join("installed"), "new-spawned").unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Activated,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let o = Arc::clone(&order);
        let stop = move || o.lock().unwrap().push("stop");

        recover_pending_transactions(&active, &pin, None, Some(&stop)).unwrap();

        let calls = order.lock().unwrap().clone();
        assert_eq!(calls, vec!["stop"], "需要回滚的事务必须触发内核停止且只停一次");
        assert_eq!(std::fs::read_to_string(active.join("installed")).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Prepared(旧环境未动)不得停内核,避免误杀用户正在用的实例
    #[test]
    fn recovery_does_not_stop_kernel_for_prepared_phase() {
        let dir = fake_venv("rec-no-stop");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();

        let suffix = "9999.13";
        let staged = dir.join(format!(".venv.staging.{suffix}"));
        let backup = dir.join(format!(".venv.rollback.{suffix}"));
        let tx_file = dir.join(format!(".venv.transaction.{suffix}.json"));
        std::fs::create_dir_all(&staged).unwrap();

        let record = VenvTxRecord {
            schema_version: 1,
            tx_id: suffix.into(),
            phase: TxPhase::Prepared,
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = Arc::clone(&hit);
        let stop = move || h.store(true, std::sync::atomic::Ordering::SeqCst);

        recover_pending_transactions(&active, &pin, None, Some(&stop)).unwrap();
        assert!(!hit.load(std::sync::atomic::Ordering::SeqCst),
            "Prepared 阶段旧环境未动,不得停掉正在运行的内核");
        let _ = std::fs::remove_dir_all(&dir);
    }


    // ── 工单3:非常规文件不得让 copy_tree 挂死 ──
    // FIFO 经 std::fs::copy 会在 open 处阻塞到有写端为止,
    // 一次 venv 里出现 FIFO 就能让安装线程永久挂死。
    #[cfg(unix)]
    #[test]
    fn copy_tree_rejects_fifo_instead_of_blocking() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        let fifo = src.join("pipe");
        let c_path = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) };
        assert_eq!(rc, 0, "mkfifo 失败");

        let start = std::time::Instant::now();
        let result = copy_tree(&src, &dst);
        let elapsed = start.elapsed();

        assert!(result.is_err(), "遇到 FIFO 必须报错而不是挂死");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "copy_tree 不得阻塞(实际耗时 {:?})",
            elapsed
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── 工单3:带超时的命令执行 ──
    #[test]
    fn run_with_timeout_kills_hung_command() {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30"]);
        let start = std::time::Instant::now();
        let result = run_with_timeout(cmd, std::time::Duration::from_millis(300), "测试命令");
        let elapsed = start.elapsed();

        assert!(result.is_err(), "超时必须返回错误而不是一直等");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "超时后应立即返回(实际 {:?})",
            elapsed
        );
    }

    #[test]
    fn run_with_timeout_returns_output_on_success() {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", "printf hello"]);
        let out = run_with_timeout(cmd, std::time::Duration::from_secs(5), "测试命令").unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
    }

    #[test]
    fn startup_recovery_cleans_uninstall_trash_and_orphan_rollbacks() {
        let dir = fake_venv("rec-trash");
        let active = dir.join("venv");
        let var = dir.join("var");
        std::fs::create_dir_all(&var).unwrap();
        let pin = var.join("qwenpaw-version");

        let trash_venv = dir.join(".venv.uninstall.123.456");
        let orphan_rollback = dir.join(".venv.rollback.789.012");
        let trash_pin = var.join(".qwenpaw-version.uninstall.123.456");
        let tmp_pin = var.join(".qwenpaw-version.tmp");

        std::fs::create_dir_all(&trash_venv).unwrap();
        std::fs::create_dir_all(&orphan_rollback).unwrap();
        std::fs::write(&trash_pin, "old-pin").unwrap();
        std::fs::write(&tmp_pin, "tmp-pin").unwrap();

        let _ = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert!(!trash_venv.exists(), "卸载残留 .venv.uninstall 必须清理");
        assert!(!orphan_rollback.exists(), "孤儿 .venv.rollback 必须清理");
        assert!(!trash_pin.exists(), "版本钉卸载残留必须清理");
        assert!(!tmp_pin.exists(), "版本钉临时文件必须清理");
        let _ = std::fs::remove_dir_all(&dir);
    }

}
