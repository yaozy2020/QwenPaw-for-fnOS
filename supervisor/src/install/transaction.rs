use super::{
    atomic_write_0600, io::relocate_console_script_shebangs, run_with_timeout, InstallState,
};
use std::{io::Write, path::Path, sync::atomic::Ordering};

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
    /// 提交决定已持久化,版本钉与备份清理由提交或启动恢复完成
    Committed,
    /// 已回滚
    RolledBack,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// 健康检查通过时记录的实际解析版本。Latest/Beta 渠道 target_version
    /// 为空,Committed 恢复重建钉依赖它；无法重建时保留未决事务。
    #[serde(default)]
    pub resolved_version: Option<String>,
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

pub(super) fn check_crash_point(point: CrashPoint) {
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
    // 故障注入是测试设施,只应在 debug 构建与显式开启时生效。
    // 若随生产二进制发行,任何能设置环境变量的人(同用户/子进程继承)
    // 都能让监督者在安装中途 SIGKILL 自己 —— 那是可被外部触发的自毁开关。
    // 生产需显式 QWENPAW_ENABLE_CRASH_POINT=1 才启用,默认关闭。
    let crash_env_allowed = cfg!(debug_assertions)
        || std::env::var("QWENPAW_ENABLE_CRASH_POINT")
            .map(|v| v == "1")
            .unwrap_or(false);
    let matches_env = crash_env_allowed
        && std::env::var("QWENPAW_CRASH_POINT")
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
    std::fs::File::open(dir)?.sync_all()
}

/// 由记录反推它所属的事务文件路径。
/// tx_id 即文件名的 pid.seq 部分,保证恢复阶段不必额外保存路径字段。
pub(super) fn record_tx_path(record: &VenvTxRecord, parent: &Path) -> std::path::PathBuf {
    parent.join(format!(".venv.transaction.{}.json", record.tx_id))
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TxRecordFailurePoint {
    BeforeRename,
    DirectorySyncAfterRename,
    ConfirmationRead,
}

#[cfg(test)]
static TX_RECORD_FAILURES: std::sync::Mutex<Vec<(std::path::PathBuf, TxRecordFailurePoint)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(super) fn inject_tx_record_failure(tx_file: &Path, point: TxRecordFailurePoint) {
    TX_RECORD_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((tx_file.to_path_buf(), point));
}

#[cfg(test)]
fn take_tx_record_failure(tx_file: &Path, point: TxRecordFailurePoint) -> bool {
    let mut failures = TX_RECORD_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(index) = failures
        .iter()
        .position(|(path, failure_point)| path == tx_file && *failure_point == point)
    else {
        return false;
    };
    failures.remove(index);
    true
}

pub(super) fn write_tx_record(tx_file: &Path, record: &VenvTxRecord) -> std::io::Result<()> {
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
    #[cfg(test)]
    if take_tx_record_failure(tx_file, TxRecordFailurePoint::BeforeRename) {
        return Err(std::io::Error::other(
            "injected transaction record failure before rename",
        ));
    }
    std::fs::rename(&tmp, tx_file)?;
    if let Some(p) = tx_file.parent() {
        sync_tx_record_parent(tx_file, p)?;
    }
    Ok(())
}

fn sync_tx_record_parent(_tx_file: &Path, parent: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if take_tx_record_failure(_tx_file, TxRecordFailurePoint::DirectorySyncAfterRename) {
        return Err(std::io::Error::other(
            "injected transaction directory sync failure after rename",
        ));
    }
    sync_dir(parent)
}

fn read_tx_record_for_confirmation(tx_file: &Path) -> std::io::Result<VenvTxRecord> {
    #[cfg(test)]
    if take_tx_record_failure(tx_file, TxRecordFailurePoint::ConfirmationRead) {
        return Err(std::io::Error::other(
            "injected transaction confirmation read failure",
        ));
    }
    let content = std::fs::read_to_string(tx_file)?;
    serde_json::from_str(&content)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub struct VenvTransaction {
    pub(super) record: VenvTxRecord,
    pub(super) tx_file: std::path::PathBuf,
    pub(super) committed_or_rolled_back: bool,
}

impl VenvTransaction {
    #[allow(dead_code)]
    pub fn prepare(
        active: &Path,
        seq: u64,
        target_version: &str,
        pin_path: &Path,
    ) -> std::io::Result<Self> {
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
        Self::prepare_using(active, seq, target_version, pin_path, python, |cmd| {
            run_with_timeout(cmd, std::time::Duration::from_secs(180), "创建 venv")
        })
    }

    pub(crate) fn prepare_with_python_cancellable(
        active: &Path,
        seq: u64,
        target_version: &str,
        pin_path: &Path,
        python: &Path,
        state: &InstallState,
        pid_file: &Path,
    ) -> std::io::Result<Self> {
        Self::prepare_using(active, seq, target_version, pin_path, python, |cmd| {
            state.run_cancellable_command(
                cmd,
                std::time::Duration::from_secs(180),
                "创建 venv",
                pid_file,
            )
        })
    }

    fn prepare_using<F>(
        active: &Path,
        seq: u64,
        target_version: &str,
        pin_path: &Path,
        python: &Path,
        run_command: F,
    ) -> std::io::Result<Self>
    where
        F: FnOnce(std::process::Command) -> std::io::Result<std::process::Output>,
    {
        let pending = pending_transactions(active)?;
        if !pending.is_empty() {
            return Err(std::io::Error::other(format!(
                "存在未决安装事务: {}",
                pending.join("; ")
            )));
        }
        let parent = active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;
        let suffix = format!("{}.{}", std::process::id(), seq);
        let staged = parent.join(format!(".venv.staging.{suffix}"));
        let backup = parent.join(format!(".venv.rollback.{suffix}"));
        let tx_file = parent.join(format!(".venv.transaction.{suffix}.json"));
        let had_active = active.exists() && active.join("bin/python3").exists();

        let mut cmd = std::process::Command::new(python);
        cmd.arg("-m").arg("venv").arg(&staged);
        let prepared = run_command(cmd).and_then(|output| {
            if output.status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "创建 venv 失败: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )))
            }
        });
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
            resolved_version: None,
        };

        if let Err(error) = write_tx_record(&tx_file, &record) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(error);
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
        relocate_console_script_shebangs(&self.record.staged, &self.record.active)?;
        if self.record.had_active {
            std::fs::rename(&self.record.active, &self.record.backup)?;
            sync_dir(parent)?;
            self.record.phase = TxPhase::BackedUp;
            write_tx_record(&self.tx_file, &self.record)?;
            check_crash_point(CrashPoint::AfterActiveRenamedToBackup);
        } else {
            remove_tree(&self.record.active)?;
        }
        std::fs::rename(&self.record.staged, &self.record.active)?;
        sync_dir(parent)?;
        self.record.phase = TxPhase::Activated;
        write_tx_record(&self.tx_file, &self.record)?;
        check_crash_point(CrashPoint::AfterStagedActivated);
        Ok(())
    }

    pub fn rollback(&mut self) -> std::io::Result<()> {
        let parent = self.record.active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;
        rollback_record(&self.record, parent, &self.record.pin_path, None)?;
        self.record.phase = TxPhase::RolledBack;
        write_tx_record(&self.tx_file, &self.record)?;
        remove_file(&self.tx_file)?;
        sync_dir(parent)?;
        self.committed_or_rolled_back = true;
        Ok(())
    }

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

    pub fn commit(&mut self, pin_content: Option<&str>) -> std::io::Result<()> {
        let parent = self.record.active.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "venv 缺少父目录")
        })?;
        let previous_record = self.record.clone();
        self.record.phase = TxPhase::Committed;
        if let Err(write_error) = write_tx_record(&self.tx_file, &self.record) {
            let committed_record = self.record.clone();
            match read_tx_record_for_confirmation(&self.tx_file) {
                Ok(persisted) if persisted == previous_record => {
                    self.record = previous_record;
                }
                Ok(persisted) if persisted == committed_record => {}
                Ok(_) => {
                    return Err(std::io::Error::other(format!(
                        "事务记录写入失败且提交状态无法确认,保留阻断状态: {write_error}"
                    )))
                }
                Err(confirmation_error) => {
                    return Err(std::io::Error::other(format!(
                        "事务记录写入失败且无法读取确认提交状态,保留阻断状态: {write_error}; {confirmation_error}"
                    )))
                }
            }
            return Err(write_error);
        }
        if let Some(content) = pin_content {
            atomic_write_0600(&self.record.pin_path.to_string_lossy(), content)?;
        }
        check_crash_point(CrashPoint::AfterPinWritten);
        remove_tree(&self.record.staged)?;
        remove_tree(&self.record.backup)?;
        sync_dir(parent)?;
        check_crash_point(CrashPoint::AfterCommit);
        remove_file(&self.tx_file)?;
        sync_dir(parent)?;
        self.committed_or_rolled_back = true;
        Ok(())
    }
}

pub(super) fn remove_tree(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn rollback_record(
    record: &VenvTxRecord,
    parent: &Path,
    pin_path: &Path,
    _log: Option<&dyn Fn(&str)>,
) -> std::io::Result<()> {
    if record.phase == TxPhase::Committed {
        return Err(std::io::Error::other("已提交事务只能完成恢复清理,不得回滚"));
    }
    if record.had_active
        && !record.backup.exists()
        && (record.phase != TxPhase::Prepared || !record.active.join("bin/python3").exists())
    {
        return Err(std::io::Error::other("旧环境备份缺失,无法确认活动环境归属"));
    }
    remove_tree(&record.staged)?;
    if record.had_active {
        restore_previous_pin(record)?;
        if record.backup.exists() {
            remove_tree(&record.active)?;
            std::fs::rename(&record.backup, &record.active)?;
        }
    } else {
        remove_tree(&record.active)?;
        remove_file(&record.pin_path)?;
        if record.pin_path != pin_path {
            remove_file(pin_path)?;
        }
    }
    sync_dir(parent)?;
    if let Some(parent) = record.pin_path.parent() {
        if parent.exists() {
            sync_dir(parent)?;
        }
    }
    Ok(())
}

pub(super) fn restore_previous_pin(record: &VenvTxRecord) -> std::io::Result<()> {
    match record.previous_pin.as_deref() {
        Some(prev) => atomic_write_0600(&record.pin_path.to_string_lossy(), prev),
        None if record.phase == TxPhase::HealthChecked => remove_file(&record.pin_path),
        None => Ok(()),
    }
}

#[derive(Debug, Default)]
pub struct RecoveryReport {
    pub recovered: usize,
    pub pending: Vec<String>,
}

impl RecoveryReport {
    pub fn is_ready(&self) -> bool {
        self.pending.is_empty()
    }
}

pub(super) fn pending_transactions(venv_dir: &Path) -> std::io::Result<Vec<String>> {
    let Some(parent) = venv_dir.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "venv 缺少父目录",
        ));
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut pending = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".venv.transaction.") && name.ends_with(".json") {
            pending.push(entry.path().to_string_lossy().into_owned());
        }
    }
    pending.sort();
    Ok(pending)
}

pub fn recover_pending_transactions(
    venv_dir: &Path,
    pin_path: &Path,
    log: Option<&dyn Fn(&str)>,
    stop_kernel: Option<&dyn Fn()>,
) -> std::io::Result<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let tx_files = pending_transactions(venv_dir)?;
    let Some(parent) = venv_dir.parent().filter(|p| p.exists()) else {
        return Ok(report);
    };
    let mut records = Vec::new();
    for tx_path in &tx_files {
        let parsed = std::fs::read_to_string(tx_path).and_then(|content| {
            serde_json::from_str::<VenvTxRecord>(&content)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        });
        match parsed {
            Ok(record)
                if record.schema_version == 1
                    && !record.tx_id.is_empty()
                    && record
                        .tx_id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                    && record.active == venv_dir
                    && record.pin_path == pin_path
                    && record.staged == parent.join(format!(".venv.staging.{}", record.tx_id))
                    && record.backup == parent.join(format!(".venv.rollback.{}", record.tx_id))
                    && record_tx_path(&record, parent) == Path::new(tx_path) =>
            {
                records.push(record)
            }
            _ => {
                report
                    .pending
                    .push(format!("{tx_path}: 事务记录无法读取或不兼容"));
            }
        }
    }
    if tx_files.len() > 1 || !report.is_ready() {
        report.pending.extend(tx_files);
        report.pending.sort();
        report.pending.dedup();
        if let Some(log) = log {
            log(&format!(
                "recovery: 保留未决事务: {}",
                report.pending.join("; ")
            ));
        }
        return Ok(report);
    }

    for record in &records {
        let tx_path = record_tx_path(record, parent);
        let result = (|| -> std::io::Result<()> {
            match record.phase {
                TxPhase::Prepared => {
                    if record.had_active && !record.active.exists() && !record.backup.exists() {
                        return Err(std::io::Error::other("旧环境与备份均缺失"));
                    }
                    remove_tree(&record.staged)?;
                    if record.had_active && !record.active.exists() && record.backup.exists() {
                        std::fs::rename(&record.backup, &record.active)?;
                    }
                    if record.backup.exists() {
                        return Err(std::io::Error::other("Prepared 事务仍有未确定归属的备份"));
                    }
                }
                TxPhase::BackedUp | TxPhase::Activated | TxPhase::HealthChecked => {
                    if let Some(stop) = stop_kernel {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(stop))
                            .map_err(|_| std::io::Error::other("停止内核回调异常,未回滚磁盘"))?;
                    }
                    rollback_record(record, parent, pin_path, log)?;
                }
                TxPhase::Committed => {
                    if !record.active.join("bin/python3").exists() {
                        return Err(std::io::Error::other("已提交事务的活动环境不可用"));
                    }
                    let target = record
                        .resolved_version
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| record.target_version.trim());
                    let pin_now = std::fs::read_to_string(&record.pin_path).unwrap_or_default();
                    if target.is_empty() && pin_now.trim().is_empty() {
                        return Err(std::io::Error::other("无法重建已提交事务的版本钉"));
                    }
                    if !target.is_empty() && pin_now.trim() != target {
                        atomic_write_0600(
                            &record.pin_path.to_string_lossy(),
                            &format!("{target}\n"),
                        )?;
                    }
                    remove_tree(&record.staged)?;
                    remove_tree(&record.backup)?;
                }
                TxPhase::RolledBack => {
                    if record.had_active && !record.active.join("bin/python3").exists() {
                        return Err(std::io::Error::other("已回滚事务的活动环境不可用"));
                    }
                    remove_tree(&record.staged)?;
                    if record.backup.exists() {
                        return Err(std::io::Error::other("已回滚事务仍有备份,保留待检查"));
                    }
                }
            }
            sync_dir(parent)?;
            remove_file(&tx_path)?;
            sync_dir(parent)
        })();
        match result {
            Ok(()) if !tx_path.exists() => report.recovered += 1,
            Ok(()) => report
                .pending
                .push(format!("{}: 事务记录清理未完成", tx_path.display())),
            Err(e) => report.pending.push(format!("{}: {e}", tx_path.display())),
        }
    }

    report.pending.extend(pending_transactions(venv_dir)?);
    if report.is_ready() {
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".venv.staging.")
                || name.starts_with(".venv.uninstall.")
                || name.starts_with(".venv.rollback.")
            {
                if let Err(e) = remove_tree(&entry.path()) {
                    report
                        .pending
                        .push(format!("{}: {e}", entry.path().display()));
                }
            }
        }
        if let Some(var_dir) = pin_path.parent().filter(|p| p.exists()) {
            for entry in std::fs::read_dir(var_dir)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".qwenpaw-version.uninstall.") || name == ".qwenpaw-version.tmp"
                {
                    if let Err(e) = remove_file(&entry.path()) {
                        report
                            .pending
                            .push(format!("{}: {e}", entry.path().display()));
                    }
                }
            }
            sync_dir(var_dir)?;
        }
        sync_dir(parent)?;
    }
    if let Some(log) = log {
        for pending in &report.pending {
            log(&format!("recovery pending: {pending}"));
        }
    }
    Ok(report)
}

#[cfg(test)]
pub(super) fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
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
