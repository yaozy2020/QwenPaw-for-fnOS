//! 内核安装状态机 —— 模式照搬 dsh server.go(单槽状态 + Seq 去重 + 终态唯一写入者)。
//! 纪律(S4/S6/S8):不编造百分比,只报"已获取 N 项"+ 尾行;终态唯一写入者是
//! 安装线程;失败清理半成品;Seq 是前端"结果只提示一次"的去重键。

use std::{
    process::Stdio,
    sync::{Arc, Mutex},
};

use std::io::{BufRead, BufReader};

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Downloading,
    Installing,
    Done,
    Failed,
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
}

#[derive(Clone)]
pub struct InstallState {
    inner: Arc<Mutex<Inner>>,
    on_change: Arc<dyn Fn(InstallSnapshot) + Send + Sync>,
    on_success: Arc<dyn Fn() + Send + Sync>,
}

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

    /// 运行 pip 安装(独立进程组 + 专用阻塞线程;终态唯一写入者是安装线程)。
    /// 返回是否成功启动(重复触发返回 false,对齐 dsh 的 busy 语义)。
    pub fn try_start(&self, venv_dir: String, spec: String, mirror: String, pip_log: String) -> bool {
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
        }
        self.emit(self.snapshot());

        let this = self.clone();
        let on_success = std::sync::Arc::clone(&self.on_success);
        std::thread::spawn(move || {
            let mirror_opt = if mirror.is_empty() { String::new() } else { format!("-i {mirror}") };
            let script =
                format!("source {venv_dir}/bin/activate && python3 -m pip install --upgrade {spec} {mirror_opt}");
            let mut cmd = std::process::Command::new("bash");
            cmd.arg("-c").arg(&script).stdout(Stdio::piped());
            use std::os::unix::process::CommandExt;
            cmd.process_group(0); // 独立进程组:失败可整组清扫
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&pip_log)
                .ok();

            let ok = match cmd.spawn() {
                Ok(mut child) => {
                    // pip 的进度走 stdout:逐行读取 → 落盘 + 解析(dsh consumeNpmOutput 模式)
                    if let Some(out) = child.stdout.take() {
                        let mut reader = BufReader::new(out);
                        let mut buf: Vec<u8> = Vec::new();
                        loop {
                            buf.clear();
                            // read_until + lossy:pip 吐非法 UTF-8 时逐行降级而不是
                            // Err 中断(曾导致日志从此截断、进度消息冻结)
                            match reader.read_until(b'\n', &mut buf) {
                                Ok(0) => break,
                                Ok(_) => {
                                    let owned = String::from_utf8_lossy(&buf);
                                    let l = owned.trim_end();
                                    if let Some(mut log) = log.as_ref() {
                                        use std::io::Write;
                                        let _ = writeln!(log, "{l}");
                                    }
                                    if l.starts_with("Collecting ") {
                                        let pkg = l
                                            .strip_prefix("Collecting ")
                                            .unwrap_or("")
                                            .split(' ')
                                            .next()
                                            .unwrap_or("")
                                            .to_string();
                                        this.set(|g| {
                                            g.fetched += 1;
                                            g.message = format!("获取 {pkg}");
                                        });
                                    } else if l.starts_with("Downloading ") {
                                        this.set(|g| g.message = "下载中".into());
                                    } else if l.starts_with("Successfully installed ") {
                                        this.set(|g| g.message = "安装依赖包".into());
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    child.wait().map(|s| s.success()).unwrap_or(false)
                }
                Err(e) => {
                    this.set(|g| {
                        g.phase = Phase::Failed;
                        g.error = e.to_string();
                    });
                    false
                }
            };

            this.set(|g| {
                g.running = false;
                if ok {
                    g.phase = Phase::Done;
                    g.message = "安装完成".into();
                } else {
                    g.phase = Phase::Failed;
                    g.error = "pip 安装失败,详见日志".into();
                }
            });
            if ok {
                // 安装成功自动拉起内核(装完即用,无需手动重启)
                (on_success)();
            }
            let _ = ok;
        });
        true
    }
}
