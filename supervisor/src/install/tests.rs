#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_health_stream_observes_cancellation() {
        use std::io::{Read, Write};
        use std::sync::atomic::AtomicBool;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let (connected_tx, connected_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_write_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            connected_tx.send(()).unwrap();
            while !server_stop.load(Ordering::SeqCst) {
                if stream.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let probe_cancelled = Arc::clone(&cancelled);
        let probe = std::thread::spawn(move || {
            let ready = wait_for_version_cancellable(port, "2.0.0", 30, &|| {
                probe_cancelled.load(Ordering::SeqCst)
            });
            done_tx.send(ready).unwrap();
        });
        connected_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        cancelled.store(true, Ordering::SeqCst);
        let result = done_rx.recv_timeout(Duration::from_millis(300));
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        probe.join().unwrap();
        assert_eq!(result, Ok(false), "持续有响应的连接也必须响应取消");
    }

    #[test]
    fn regression_missing_backup_must_not_claim_rollback_succeeded() {
        let dir = fake_venv("missing-backup");
        let mut tx = VenvTransaction::prepare_with_python(
            &dir.join("venv"),
            84,
            "2.0.0",
            &dir.join("pin"),
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        tx.activate().unwrap();
        std::fs::remove_dir_all(&tx.record.backup).unwrap();
        let result = tx.rollback();
        let retained = tx.tx_file.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "旧备份丢失后不能把新版 active 当成已恢复的旧版"
        );
        assert!(retained);
    }

    #[test]
    fn regression_directory_sync_propagates_failure() {
        let dir = fake_venv("sync-dir-failure");
        let result = sync_dir(&dir.join("missing"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err(), "目录 fsync 失败不得伪装成持久化成功");
    }

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

    fn version_server() -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut request = [0; 1024];
                    let _ = stream.read(&mut request);
                    let body = r#"{"version":"2.0.0"}"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        (port, server)
    }

    fn start_fake_install(st: &InstallState, dir: &Path, port: u16, pin: &Path) -> bool {
        st.set_base_python(dir.join("venv/base-python"));
        st.try_start(
            dir.join("venv").to_string_lossy().into_owned(),
            port,
            InstallChannel::Latest,
            String::new(),
            dir.join("pip.log").to_string_lossy().into_owned(),
            pin.to_string_lossy().into_owned(),
        )
    }

    #[test]
    fn shutdown_cancels_slow_venv_creation_and_reaps_registered_process() {
        use std::os::unix::fs::PermissionsExt;

        let dir = fake_venv("shutdown-slow-venv");
        let root = dir.clone();
        let base_python = dir.join("venv/base-python");
        std::fs::write(
            &base_python,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ntouch '{}'\nsleep 30 &\nwait\n",
                root.join("venv-argv").display(),
                root.join("venv-started").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&base_python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let state = InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0)));
        assert!(start_fake_install(&state, &dir, 0, &dir.join("pin")));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !dir.join("venv-started").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            dir.join("venv-started").exists(),
            "slow venv command did not start"
        );
        let venv_argv = std::fs::read_to_string(dir.join("venv-argv")).unwrap();
        assert!(venv_argv.starts_with("-m\nvenv\n"));
        assert!(venv_argv.lines().nth(2).is_some_and(|path| {
            path.starts_with(&format!("{}/.venv.staging.", dir.display()))
        }));
        let install_pid = dir.join("install.pid");
        let pid = std::fs::read_to_string(&install_pid)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse::<i32>()
            .unwrap();

        let started = std::time::Instant::now();
        assert!(
            state.shutdown(Duration::from_secs(4)),
            "shutdown did not settle worker"
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(state.snapshot().phase, Phase::Cancelled);
        assert!(
            !crate::procfs::process_alive(pid),
            "venv child survived cancellation"
        );
        assert!(
            !install_pid.exists(),
            "install.pid must be removed after reap"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("venv/installed")).unwrap(),
            "old"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_latch_still_rolls_back_a_slow_pip_transaction() {
        use std::os::unix::fs::PermissionsExt;

        let dir = fake_venv("shutdown-slow-pip-latch");
        let root = dir.clone();
        let python = dir.join("venv/bin/python3");
        std::fs::write(
            &python,
            format!(
                "#!/bin/bash\nroot='{}'\nif [ \"$1\" = \"-m\" ] && [ \"$2\" = \"pip\" ]; then touch \"$root/pip-started\"; sleep 30; fi\nif [ \"$1\" = \"-m\" ]; then printf new > \"$(dirname \"$0\")/../installed\"; exit 0; fi\nprintf '2.0.0\\n'\n",
                root.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let state = InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0)));
        let pin = dir.join("pin");
        assert!(start_fake_install(&state, &dir, 0, &pin));

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !dir.join("pip-started").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            dir.join("pip-started").exists(),
            "slow pip command did not start"
        );
        state.block_recovery("监督者正在关闭".into());
        assert!(state.shutdown(Duration::from_secs(4)));

        let snapshot = state.snapshot();
        assert_eq!(snapshot.phase, Phase::Cancelled, "{}", snapshot.error);
        assert!(!snapshot.recovery_required, "{}", snapshot.error);
        assert_eq!(
            std::fs::read_to_string(dir.join("venv/installed")).unwrap(),
            "old"
        );
        assert!(pending_transactions(&dir.join("venv")).unwrap().is_empty());
        assert!(!dir.join("install.pid").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regression_persist_failures_stop_kernel_before_rollback() {
        for (name, pin_is_directory) in [("health", true), ("commit", false)] {
            let dir = fake_venv(&format!("persist-order-{name}"));
            let pin = if pin_is_directory {
                let pin = dir.join("pin");
                std::fs::create_dir(&pin).unwrap();
                pin
            } else {
                dir.join("missing/pin")
            };
            let seen = Arc::new(Mutex::new(Vec::new()));
            let seen_stop = Arc::clone(&seen);
            let active = dir.join("venv");
            let st = InstallState::with_stop(
                Arc::new(|_| {}),
                Arc::new(|| Ok(1)),
                Arc::new(move || {
                    seen_stop
                        .lock()
                        .unwrap()
                        .push(std::fs::read_to_string(active.join("installed")).unwrap());
                }),
            );
            let (port, server) = version_server();
            assert!(start_fake_install(&st, &dir, port, &pin));
            let snap = wait_for_terminal(&st);
            server.join().unwrap();
            let observed = seen.lock().unwrap().clone();
            let _ = std::fs::remove_dir_all(&dir);
            assert_eq!(snap.phase, Phase::Failed, "{name}: {}", snap.error);
            assert_eq!(observed, vec!["new"], "{name}: stop 必须先于 active 回滚");
        }
    }

    #[test]
    fn regression_cancel_after_metadata_never_activates_or_starts() {
        let dir = fake_venv("cancel-metadata");
        let python = dir.join("venv/bin/python3");
        std::fs::write(&python, "#!/bin/bash\nroot=\"$(dirname \"$0\")/../..\"\nif [ \"$1\" = \"-m\" ]; then printf new > \"$(dirname \"$0\")/../installed\"; exit 0; fi\ntouch \"$root/metadata-entered\"\nwhile [ ! -f \"$root/release\" ]; do sleep 0.01; done\nprintf '2.0.0\\n'\n").unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let called = Arc::clone(&calls);
        let st = InstallState::new(
            Arc::new(|_| {}),
            Arc::new(move || {
                called.fetch_add(1, Ordering::SeqCst);
                Err("must not start".into())
            }),
        );
        assert!(start_fake_install(&st, &dir, 0, &dir.join("pin")));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !dir.join("metadata-entered").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(dir.join("metadata-entered").exists());
        st.cancel();
        std::fs::write(dir.join("release"), "").unwrap();
        let snap = wait_for_terminal(&st);
        let installed = std::fs::read_to_string(dir.join("venv/installed")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(snap.phase, Phase::Cancelled, "{}", snap.error);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(installed, "old");
    }

    #[test]
    fn regression_pending_transaction_rejects_new_install() {
        let dir = fake_venv("pending-gate");
        let tx_file = dir.join(".venv.transaction.9999.99.json");
        std::fs::write(&tx_file, "broken record").unwrap();
        let (oc, os) = noop_callbacks();
        let st = InstallState::new(oc, os);
        let started = start_fake_install(&st, &dir, 0, &dir.join("pin"));
        if started {
            st.cancel();
            let _ = wait_for_terminal(&st);
        }
        let retained = tx_file.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!started, "未解决的事务必须阻止跨代安装");
        assert!(retained);
    }

    #[test]
    fn regression_transaction_prepare_refuses_an_unresolved_generation() {
        let dir = fake_venv("direct-generation-gate");
        let first = VenvTransaction::prepare_with_python(
            &dir.join("venv"),
            82,
            "2.0.0",
            &dir.join("pin"),
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        let second = VenvTransaction::prepare_with_python(
            &dir.join("venv"),
            83,
            "2.0.0",
            &dir.join("pin"),
            Path::new(&dir.join("venv/base-python")),
        );
        let rejected = second.is_err();
        assert!(first.record.staged.exists());
        assert!(first.tx_file.exists());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(rejected, "事务构造本身也不得越过旧代未决事务");
    }

    #[test]
    fn regression_recovery_keeps_record_when_staging_cleanup_fails() {
        let dir = fake_venv("recovery-cleanup-fail");
        let mut tx = VenvTransaction::prepare_with_python(
            &dir.join("venv"),
            80,
            "2.0.0",
            &dir.join("pin"),
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        std::fs::remove_dir_all(&tx.record.staged).unwrap();
        std::fs::write(&tx.record.staged, "not a directory").unwrap();
        tx.committed_or_rolled_back = true;
        let tx_file = tx.tx_file.clone();
        let report =
            recover_pending_transactions(&dir.join("venv"), &dir.join("pin"), None, None).unwrap();
        assert!(!report.is_ready());
        assert_eq!(report.recovered, 0);
        assert!(!report.pending.is_empty());
        let retained = tx_file.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(retained, "清理失败不得消费事务记录或报完全恢复");
    }

    #[test]
    fn lifecycle_restores_before_terminal_and_holds_running_slot() {
        let dir = fake_venv("restore-order");
        let order = Arc::new(Mutex::new(Vec::new()));
        let owner: Arc<Mutex<Option<InstallState>>> = Arc::new(Mutex::new(None));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let observed = Arc::clone(&order);
        let stopped = Arc::clone(&order);
        let restored = Arc::clone(&order);
        let active_stop = dir.join("venv");
        let active_restore = dir.join("venv");
        let state_restore = Arc::clone(&owner);
        let st = InstallState::with_lifecycle(
            Arc::new(move |snap| {
                if matches!(snap.phase, Phase::Failed | Phase::Cancelled | Phase::Done) {
                    assert!(!snap.running);
                    observed.lock().unwrap().push("terminal");
                }
            }),
            Arc::new(|| Err("injected spawn failure".into())),
            Arc::new(move || {
                assert_eq!(
                    std::fs::read_to_string(active_stop.join("installed")).unwrap(),
                    "new"
                );
                stopped.lock().unwrap().push("stop");
            }),
            Arc::new(move || {
                assert_eq!(
                    std::fs::read_to_string(active_restore.join("installed")).unwrap(),
                    "old"
                );
                assert!(
                    state_restore
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .snapshot()
                        .running
                );
                restored.lock().unwrap().push("restore");
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                Ok(2)
            }),
        );
        *owner.lock().unwrap() = Some(st.clone());
        assert!(start_fake_install(&st, &dir, 0, &dir.join("pin")));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(st.snapshot().running);
        assert!(!start_fake_install(&st, &dir, 0, &dir.join("pin")));
        assert_eq!(*order.lock().unwrap(), vec!["stop", "restore"]);
        release_tx.send(()).unwrap();
        assert_eq!(wait_for_terminal(&st).phase, Phase::Failed);
        assert_eq!(*order.lock().unwrap(), vec!["stop", "restore", "terminal"]);
        assert!(st.shutdown(Duration::from_secs(1)));
        *owner.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_during_restore_is_latched_and_waits_for_safe_cleanup() {
        let dir = fake_venv("shutdown-restore");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let stops = Arc::new(AtomicU64::new(0));
        let stopped = Arc::clone(&stops);
        let st = InstallState::with_lifecycle(
            Arc::new(|_| {}),
            Arc::new(|| Err("injected failure".into())),
            Arc::new(move || {
                stopped.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(move || {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                Ok(2)
            }),
        );
        assert!(start_fake_install(&st, &dir, 0, &dir.join("pin")));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!st.shutdown(Duration::from_millis(30)));
        assert!(st.snapshot().running);
        release_tx.send(()).unwrap();
        assert!(st.shutdown(Duration::from_secs(2)));
        assert_eq!(st.snapshot().phase, Phase::Cancelled);
        assert_eq!(stops.load(Ordering::SeqCst), 2);
        assert!(!start_fake_install(&st, &dir, 0, &dir.join("pin")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_cancels_health_poll_without_restoring_or_publishing_failed() {
        let dir = fake_venv("shutdown-health");
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let restores = Arc::new(AtomicU64::new(0));
        let restored = Arc::clone(&restores);
        let terminals = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&terminals);
        let st = InstallState::with_lifecycle(
            Arc::new(move |snap| {
                if matches!(snap.phase, Phase::Done | Phase::Failed | Phase::Cancelled) {
                    observed.lock().unwrap().push(snap.phase);
                }
            }),
            Arc::new(move || {
                started_tx.send(()).unwrap();
                Ok(1)
            }),
            Arc::new(|| {}),
            Arc::new(move || {
                restored.fetch_add(1, Ordering::SeqCst);
                Ok(2)
            }),
        );
        assert!(start_fake_install(&st, &dir, port, &dir.join("pin")));
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(st.shutdown(Duration::from_secs(2)));
        assert_eq!(*terminals.lock().unwrap(), vec![Phase::Cancelled]);
        assert_eq!(restores.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("venv/installed")).unwrap(),
            "old"
        );
        assert!(!start_fake_install(&st, &dir, port, &dir.join("pin")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellation_at_starting_notification_never_spawns_kernel() {
        let dir = fake_venv("cancel-before-kernel");
        let owner: Arc<Mutex<Option<InstallState>>> = Arc::new(Mutex::new(None));
        let callback_owner = Arc::clone(&owner);
        let starts = Arc::new(AtomicU64::new(0));
        let started = Arc::clone(&starts);
        let st = InstallState::with_lifecycle(
            Arc::new(move |snap| {
                if snap.phase == Phase::Starting {
                    callback_owner.lock().unwrap().as_ref().unwrap().cancel();
                }
            }),
            Arc::new(move || {
                started.fetch_add(1, Ordering::SeqCst);
                Ok(1)
            }),
            Arc::new(|| {}),
            Arc::new(|| panic!("取消不得恢复服务")),
        );
        *owner.lock().unwrap() = Some(st.clone());
        assert!(start_fake_install(&st, &dir, 0, &dir.join("pin")));
        assert_eq!(wait_for_terminal(&st).phase, Phase::Cancelled);
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert!(st.shutdown(Duration::from_secs(1)));
        *owner.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn process_identity_recovery_block_preserves_transaction_without_restore() {
        let dir = fake_venv("identity-recovery-block");
        let active = dir.join("venv");
        let mut transaction = VenvTransaction::prepare_with_python(
            &active,
            96,
            "2.0.0",
            &dir.join("pin"),
            Path::new(&active.join("base-python")),
        )
        .unwrap();
        let restores = Arc::new(AtomicU64::new(0));
        let restored = Arc::clone(&restores);
        let state = InstallState::with_lifecycle(
            Arc::new(|_| {}),
            Arc::new(|| Ok(0)),
            Arc::new(|| {}),
            Arc::new(move || {
                restored.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }),
        );
        state.block_recovery("unverified install process identity".into());
        let transaction_record = transaction.tx_file.clone();
        let staging = transaction.record.staged.clone();

        state.finish_install(
            Some(&mut transaction),
            &active,
            Err("install process cleanup failed".into()),
        );

        assert!(transaction_record.exists(), "未核实进程时事务记录必须保留");
        assert!(staging.exists(), "未核实进程可能仍使用 staging，禁止删除");
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old"
        );
        assert_eq!(restores.load(Ordering::SeqCst), 0, "必须跳过自动恢复启动");
        assert!(state.snapshot().recovery_required);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_failure_blocks_restart_and_preserves_transaction_and_backup() {
        let dir = fake_venv("blocked-rollback");
        let active = dir.join("venv");
        let st = InstallState::with_lifecycle(
            Arc::new(|_| {}),
            Arc::new(move || {
                std::fs::remove_dir_all(&active).unwrap();
                std::fs::write(&active, "cannot remove as directory").unwrap();
                Err("injected broken active".into())
            }),
            Arc::new(|| {}),
            Arc::new(|| panic!("不确定环境不得启动")),
        );
        assert!(start_fake_install(&st, &dir, 0, &dir.join("pin")));
        let snap = wait_for_terminal(&st);
        assert_eq!(snap.phase, Phase::Failed);
        assert!(snap.recovery_required);
        assert!(snap.recovery_error.is_some());
        assert!(st.recovery_error().is_some());
        assert_eq!(pending_transactions(&dir.join("venv")).unwrap().len(), 1);
        let backup = dir.join(format!(".venv.rollback.{}.1", std::process::id()));
        assert_eq!(
            std::fs::read_to_string(backup.join("installed")).unwrap(),
            "old"
        );
        assert!(!start_fake_install(&st, &dir, 0, &dir.join("pin")));
        assert!(st.shutdown(Duration::from_secs(1)));
        let report =
            recover_pending_transactions(&dir.join("venv"), &dir.join("pin"), None, Some(&|| {}))
                .unwrap();
        assert!(!report.is_ready());
        assert_eq!(report.recovered, 0);
        assert!(backup.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_recovery_record_is_reported_and_preserves_all_candidates() {
        let dir = fake_venv("corrupt-report");
        std::fs::remove_dir_all(dir.join("venv")).unwrap();
        let tx_file = dir.join(".venv.transaction.9999.1.json");
        let backup = dir.join(".venv.rollback.9999.1");
        std::fs::write(&tx_file, "not json").unwrap();
        std::fs::create_dir(&backup).unwrap();
        let report =
            recover_pending_transactions(&dir.join("venv"), &dir.join("pin"), None, None).unwrap();
        assert!(!report.is_ready());
        assert_eq!(report.recovered, 0);
        assert!(report
            .pending
            .iter()
            .any(|p| p.contains(".venv.transaction.9999.1.json")));
        assert!(tx_file.exists());
        assert!(backup.exists());
        let (oc, os) = noop_callbacks();
        let st = InstallState::new(oc, os);
        assert!(!start_fake_install(&st, &dir, 0, &dir.join("pin")));
        assert!(st.snapshot().recovery_required);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_recovery_block_and_idle_shutdown_reject_every_later_start() {
        let dir = fake_venv("manual-block");
        let (oc, os) = noop_callbacks();
        let st = InstallState::new(oc, os);
        st.block_recovery("manual recovery required".into());
        assert_eq!(
            st.recovery_error().as_deref(),
            Some("manual recovery required")
        );
        assert!(!start_fake_install(&st, &dir, 0, &dir.join("pin")));
        let (oc, os) = noop_callbacks();
        let idle = InstallState::new(oc, os);
        assert!(idle.shutdown(Duration::ZERO));
        assert!(!start_fake_install(&idle, &dir, 0, &dir.join("pin")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_one_without_optional_fields_remains_recoverable() {
        let dir = fake_venv("schema-one");
        let tx = VenvTransaction::prepare_with_python(
            &dir.join("venv"),
            81,
            "2.0.0",
            &dir.join("pin"),
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        let mut json = serde_json::to_value(&tx.record).unwrap();
        json.as_object_mut().unwrap().remove("previous_pin");
        json.as_object_mut().unwrap().remove("resolved_version");
        std::fs::write(&tx.tx_file, serde_json::to_vec(&json).unwrap()).unwrap();
        let report =
            recover_pending_transactions(&dir.join("venv"), &dir.join("pin"), None, None).unwrap();
        assert!(report.is_ready(), "{:?}", report.pending);
        assert_eq!(report.recovered, 1);
        let _ = std::fs::remove_dir_all(&dir);
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
            st.inner.lock().unwrap_or_else(|e| e.into_inner()).cancelled,
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
            self.spawned_pids
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(child.id());
            Ok(child)
        }
    }

    fn fake_venv(test_name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("qwenpaw-venv-{test_name}-{}", std::process::id()));
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
        fake_base_python(&dir);
        dir
    }

    pub(super) fn fake_base_python(dir: &Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let python_template = dir.join("venv/bin/python3");
        let python_template = serde_json::to_string(&python_template.to_string_lossy()).unwrap();
        let base_python = dir.join("venv/base-python");
        std::fs::write(
            &base_python,
            format!(
                "#!/usr/bin/python3\nimport pathlib, shutil, sys\nif len(sys.argv) < 4 or sys.argv[1:3] != ['-m', 'venv']:\n    sys.exit(2)\nbin_dir = pathlib.Path(sys.argv[3]) / 'bin'\nbin_dir.mkdir(parents=True)\nshutil.copy2({python_template}, bin_dir / 'python3')\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&base_python, std::fs::Permissions::from_mode(0o755)).unwrap();
        base_python
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
        std::fs::write(
            &python,
            "#!/bin/bash\nmkdir -p \"$3/bin\"\ncp \"$0\" \"$3/bin/python3\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&python).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        std::fs::set_permissions(&python, permissions).unwrap();
        let active = dir.join("venv");
        let pin = dir.join("pin");
        let mut transaction =
            VenvTransaction::prepare_with_python(&active, 1, "2.0.0", &pin, &python).unwrap();
        assert!(transaction.python().exists());
        assert!(!active.exists(), "暂存提交前不得创建活动 venv");
        transaction.activate().unwrap();
        assert!(active.join("bin/python3").exists());
        transaction.commit(None).unwrap();
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            2,
            "提交后仅保留基础 Python 和活动 venv"
        );
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
        st.set_base_python(dir.join("venv/base-python"));
        assert!(st.try_start(
            venv.to_string_lossy().into_owned(),
            crate::api::test_support::free_port(),
            InstallChannel::Latest,
            String::new(),
            dir.join("pip.log").to_string_lossy().into_owned(),
            pin.to_string_lossy().into_owned(),
        ));
        let snapshot = wait_for_terminal(&st);
        assert_eq!(snapshot.phase, Phase::Failed);
        assert_eq!(
            std::fs::read_to_string(venv.join("installed")).unwrap(),
            "old"
        );
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
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let listener = Arc::new(Mutex::new(Some(listener)));
        let on_success_listener = Arc::clone(&listener);
        let on_success: OnSuccessCb = Arc::new(move || {
            let listener = on_success_listener
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .ok_or_else(|| "health listener was already consumed".to_string())?;
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    let body = r#"{"version":"2.0.0"}"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
            });
            Ok(1)
        });
        let st = InstallState::new(Arc::new(|_: InstallSnapshot| {}), on_success);
        st.set_base_python(dir.join("venv/base-python"));
        assert!(st.try_start(
            venv.to_string_lossy().into_owned(),
            port,
            InstallChannel::Latest,
            String::new(),
            dir.join("pip.log").to_string_lossy().into_owned(),
            pin.to_string_lossy().into_owned(),
        ));
        let snapshot = wait_for_terminal(&st);
        assert_eq!(snapshot.phase, Phase::Done, "{}", snapshot.error);
        assert_eq!(
            std::fs::read_to_string(venv.join("installed")).unwrap(),
            "new"
        );
        assert_eq!(std::fs::read_to_string(pin).unwrap(), "2.0.0\n");
        let entries = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(entries, 3, "成功后不得残留暂存与备份目录: {entries}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_preserves_process_group_when_registered_leader_identity_is_stale() {
        use std::os::unix::process::CommandExt;

        let dir = fake_venv("cancel-stale-identity");
        let pid_file = dir.join("install.pid");
        let mut leader = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = leader.id();
        let pgid = pid as i32;
        let mut unrelated = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(pgid)
            .spawn()
            .unwrap();
        let state = InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0)));
        state.register_process(pid, pgid, &pid_file).unwrap();
        state.set(|inner| {
            inner.running = true;
            inner.starttime += 1;
        });

        state.cancel();
        std::thread::sleep(Duration::from_millis(100));
        let leader_alive = crate::procfs::process_alive(pid as i32);
        let unrelated_alive = crate::procfs::process_alive(unrelated.id() as i32);
        let registration_retained = pid_file.exists();
        let recovery_blocked = state.recovery_error().is_some();

        let _ = leader.kill();
        let _ = leader.wait();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(leader_alive, "陈旧 leader 身份不得收到组信号");
        assert!(unrelated_alive, "PGID 内的无关进程必须存活");
        assert!(registration_retained, "不确定身份时必须保留 install.pid");
        assert!(recovery_blocked, "不确定身份必须 fail-closed");
    }

    #[test]
    fn cancel_preserves_process_group_when_registered_executable_identity_mismatches() {
        use std::os::unix::process::CommandExt;

        let dir = fake_venv("cancel-executable-mismatch");
        let pid_file = dir.join("install.pid");
        let mut leader = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = leader.id();
        let pgid = pid as i32;
        let mut group_member = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(pgid)
            .spawn()
            .unwrap();
        let state = InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0)));
        state.register_process(pid, pgid, &pid_file).unwrap();
        state.set(|inner| {
            inner.running = true;
            let mut executable = inner.executable.unwrap();
            executable.ino = executable.ino.wrapping_add(1);
            inner.executable = Some(executable);
        });

        state.cancel();
        std::thread::sleep(Duration::from_millis(100));
        let leader_alive = crate::procfs::process_alive(pid as i32);
        let group_member_alive = crate::procfs::process_alive(group_member.id() as i32);
        let registration_retained = pid_file.exists();
        let recovery_blocked = state.recovery_error().is_some();

        let _ = leader.kill();
        let _ = leader.wait();
        let _ = group_member.kill();
        let _ = group_member.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(leader_alive, "executable 身份不匹配时 leader 必须存活");
        assert!(group_member_alive, "executable 身份不匹配时组成员必须存活");
        assert!(registration_retained, "不确定身份时必须保留 install.pid");
        assert!(recovery_blocked, "executable 身份不匹配必须 fail-closed");
    }

    #[test]
    fn normal_group_cleanup_removes_registration_and_preserves_external_sentinel() {
        use std::os::unix::process::CommandExt;

        let dir = fake_venv("normal-group-cleanup");
        let pid_file = dir.join("install.pid");
        let mut leader = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = leader.id();
        let pgid = pid as i32;
        let mut group_member = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(pgid)
            .spawn()
            .unwrap();
        let mut sentinel = std::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let state = InstallState::new(Arc::new(|_| {}), Arc::new(|| Ok(0)));
        state.register_process(pid, pgid, &pid_file).unwrap();

        super::process::terminate_process_group(&mut leader, state.registered_identity(pid))
            .unwrap();
        state.clear_process(pid, &pid_file).unwrap();
        let group_gone = !crate::procfs::process_alive(pid as i32)
            && !crate::procfs::process_alive(group_member.id() as i32);
        let sentinel_alive = crate::procfs::process_alive(sentinel.id() as i32);
        let registration_removed = !pid_file.exists();

        let _ = leader.wait();
        let _ = group_member.kill();
        let _ = group_member.wait();
        let _ = sentinel.kill();
        let _ = sentinel.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(group_gone, "受管进程组必须被回收");
        assert!(sentinel_alive, "组外哨兵必须存活");
        assert!(registration_removed, "成功清理后必须移除 install.pid");
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

        st.set_base_python(dir.join("venv/base-python"));
        assert!(st.try_start(
            dir.join("venv").to_string_lossy().into_owned(),
            crate::api::test_support::free_port(),
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
        let spawned = pids.lock().unwrap_or_else(|e| e.into_inner()).clone();
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
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered.recovered, 1);
        assert!(recovered.is_ready(), "{:?}", recovered.pending);
        assert!(!staged.exists(), "Prepared 的暂存目录必须被清理");
        assert!(!tx_file.exists(), "事务文件必须被清理");
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old"
        );
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
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered.recovered, 1);
        assert!(recovered.is_ready(), "{:?}", recovered.pending);
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old",
            "必须回滚为旧环境"
        );
        assert!(!backup.exists(), "回滚后 backup 目录不残留");
        assert!(!tx_file.exists(), "事务文件已清理");
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_activated_fresh_install_removes_active_and_leaves_uninstalled() {
        let dir =
            std::env::temp_dir().join(format!("qwenpaw-fresh-rollback-{}", std::process::id()));
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
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered.recovered, 1);
        assert!(recovered.is_ready(), "{:?}", recovered.pending);
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
            resolved_version: Some("2.0.0".into()),
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered.recovered, 1);
        assert!(recovered.is_ready(), "{:?}", recovered.pending);
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old",
            "保守路径必须回滚原环境"
        );
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
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered.recovered, 1);
        assert!(recovered.is_ready(), "{:?}", recovered.pending);
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "new-committed",
            "新环境必须保留"
        );
        assert_eq!(
            std::fs::read_to_string(&pin).unwrap(),
            "2.0.0\n",
            "新版本钉必须保留"
        );
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
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let recovered = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert_eq!(recovered.recovered, 1);
        assert!(recovered.is_ready(), "{:?}", recovered.pending);
        assert!(active.exists(), "活动环境必须被恢复,绝不允许消失");
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old",
            "必须恢复为旧环境内容"
        );
        assert_eq!(
            std::fs::read_to_string(&pin).unwrap(),
            "1.0.0\n",
            "版本钉必须保持旧值"
        );
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
            resolved_version: None,
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
            resolved_version: Some("2.0.0".into()),
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

        let mut transaction = VenvTransaction::prepare_with_python(
            &active,
            9,
            "2.0.0",
            &pin,
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        transaction.activate().unwrap();
        transaction.health_checked().unwrap();
        let result = transaction.commit(Some("2.0.0\n"));
        assert!(result.is_err(), "版本钉写入失败必须向上传播,不能报 Done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn committed_pin_failure_preserves_commit_decision_and_blocks_rollback() {
        let dir = fake_venv("committed-pin-commit-regression");
        let active = dir.join("venv");
        let pin = dir.join("missing-parent/pin");
        let mut transaction = VenvTransaction::prepare_with_python(
            &active,
            10,
            "2.0.0",
            &pin,
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        std::fs::write(transaction.record.staged.join("installed"), "new").unwrap();
        transaction.activate().unwrap();
        transaction.health_checked().unwrap();

        assert!(transaction.commit(Some("2.0.0\n")).is_err());
        assert_eq!(transaction.record.phase, TxPhase::Committed);
        let persisted: VenvTxRecord =
            serde_json::from_str(&std::fs::read_to_string(&transaction.tx_file).unwrap()).unwrap();
        assert_eq!(persisted.phase, TxPhase::Committed);
        assert!(transaction.rollback().is_err());
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "new",
            "rollback must not restore the old environment after commit was persisted"
        );
        assert_eq!(
            std::fs::read_to_string(transaction.record.backup.join("installed")).unwrap(),
            "old"
        );
        assert!(
            transaction.tx_file.exists(),
            "Committed 事务记录必须保留恢复依据"
        );
        let persisted: VenvTxRecord =
            serde_json::from_str(&std::fs::read_to_string(&transaction.tx_file).unwrap()).unwrap();
        assert_eq!(persisted.phase, TxPhase::Committed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_journal_failure_before_rename_allows_safe_rollback() {
        let dir = fake_venv("commit-record-before-rename");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();
        let mut transaction = VenvTransaction::prepare_with_python(
            &active,
            101,
            "2.0.0",
            &pin,
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        std::fs::write(transaction.record.staged.join("installed"), "new").unwrap();
        transaction.activate().unwrap();
        transaction.health_checked().unwrap();
        super::transaction::inject_tx_record_failure(
            &transaction.tx_file,
            super::transaction::TxRecordFailurePoint::BeforeRename,
        );

        assert!(transaction.commit(None).is_err());
        assert_eq!(transaction.record.phase, TxPhase::HealthChecked);
        let persisted: VenvTxRecord =
            serde_json::from_str(&std::fs::read_to_string(&transaction.tx_file).unwrap()).unwrap();
        assert_eq!(persisted.phase, TxPhase::HealthChecked);
        transaction.rollback().unwrap();
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old"
        );
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "1.0.0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_journal_failure_after_rename_recovers_new_env_and_pin() {
        let dir = fake_venv("commit-record-after-rename");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();
        let mut transaction = VenvTransaction::prepare_with_python(
            &active,
            102,
            "2.0.0",
            &pin,
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        std::fs::write(transaction.record.staged.join("installed"), "new").unwrap();
        transaction.activate().unwrap();
        transaction.health_checked().unwrap();
        super::transaction::inject_tx_record_failure(
            &transaction.tx_file,
            super::transaction::TxRecordFailurePoint::DirectorySyncAfterRename,
        );

        assert!(transaction.commit(None).is_err());
        assert_eq!(transaction.record.phase, TxPhase::Committed);
        let persisted: VenvTxRecord =
            serde_json::from_str(&std::fs::read_to_string(&transaction.tx_file).unwrap()).unwrap();
        assert_eq!(persisted.phase, TxPhase::Committed);
        assert!(transaction.rollback().is_err());

        let report = recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert!(report.is_ready(), "{:?}", report.pending);
        assert_eq!(report.recovered, 1);
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "new",
            "Committed 恢复必须保留新环境，不得出现旧 venv + 新 pin"
        );
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), "2.0.0\n");
        assert!(!transaction.record.backup.exists());
        assert!(!transaction.tx_file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_record_confirmation_failure_blocks_rollback() {
        let dir = fake_venv("commit-record-unconfirmed");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        std::fs::write(&pin, "1.0.0\n").unwrap();
        let mut transaction = VenvTransaction::prepare_with_python(
            &active,
            103,
            "2.0.0",
            &pin,
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        std::fs::write(transaction.record.staged.join("installed"), "new").unwrap();
        transaction.activate().unwrap();
        transaction.health_checked().unwrap();
        super::transaction::inject_tx_record_failure(
            &transaction.tx_file,
            super::transaction::TxRecordFailurePoint::DirectorySyncAfterRename,
        );
        super::transaction::inject_tx_record_failure(
            &transaction.tx_file,
            super::transaction::TxRecordFailurePoint::ConfirmationRead,
        );

        assert!(transaction.commit(None).is_err());
        assert_eq!(transaction.record.phase, TxPhase::Committed);
        let persisted: VenvTxRecord =
            serde_json::from_str(&std::fs::read_to_string(&transaction.tx_file).unwrap()).unwrap();
        assert_eq!(persisted.phase, TxPhase::Committed);
        assert!(transaction.rollback().is_err());
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "new",
            "无法确认提交状态时不得回滚可能已提交的环境"
        );
        assert_eq!(
            std::fs::read_to_string(transaction.record.backup.join("installed")).unwrap(),
            "old"
        );
        assert!(transaction.tx_file.exists());
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
            phase: TxPhase::Prepared, // 关键:仍是 Prepared
            active: active.clone(),
            staged: staged.clone(),
            backup: backup.clone(),
            had_active: true,
            target_version: "2.0.0".into(),
            pin_path: pin.clone(),
            previous_pin: None,
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        recover_pending_transactions(&active, &pin, None, None).unwrap();
        assert!(active.exists(), "活动环境必须恢复,不得与备份一起消失");
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old"
        );
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
            resolved_version: None,
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
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let o = Arc::clone(&order);
        let stop = move || o.lock().unwrap_or_else(|e| e.into_inner()).push("stop");

        recover_pending_transactions(&active, &pin, None, Some(&stop)).unwrap();

        let calls = order.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            calls,
            vec!["stop"],
            "需要回滚的事务必须触发内核停止且只停一次"
        );
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old"
        );
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
            resolved_version: None,
        };
        write_tx_record(&tx_file, &record).unwrap();

        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = Arc::clone(&hit);
        let stop = move || h.store(true, std::sync::atomic::Ordering::SeqCst);

        recover_pending_transactions(&active, &pin, None, Some(&stop)).unwrap();
        assert!(
            !hit.load(std::sync::atomic::Ordering::SeqCst),
            "Prepared 阶段旧环境未动,不得停掉正在运行的内核"
        );
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

    // ── 工单11 回归:残缺 venv 不得被当成"有旧环境" ──
    // 断电可能留下"目录在、python 缺失"的半成品。若判 had_active=true,
    // 会 copy_tree 出同样坏的环境,"安装成功"却仍然起不来。
    #[test]
    fn broken_active_venv_is_rebuilt_not_copied() {
        let dir = std::env::temp_dir().join(format!("qwenpaw-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let active = dir.join("venv");
        // 目录存在但没有 bin/python3
        std::fs::create_dir_all(active.join("lib")).unwrap();
        std::fs::write(active.join("half-copied"), "junk").unwrap();

        let pin = dir.join("pin");
        let fake_py = dir.join("fakepy.sh");
        std::fs::write(
            &fake_py,
            "#!/bin/sh\nmkdir -p \"$3/bin\"\ntouch \"$3/bin/python3\"\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_py).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_py, perms).unwrap();

        let tx =
            VenvTransaction::prepare_with_python(&active, 77, "2.0.0", &pin, &fake_py).unwrap();
        // 走"新建"分支:暂存里应有 python3,不应带上残骸
        assert!(tx.python().exists(), "残缺环境必须重建而非复制");
        let tx_dir = tx.record.staged.clone();
        assert!(
            !tx_dir.join("half-copied").exists(),
            "残缺内容不得被复制进新环境"
        );
        drop(tx);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_active_venv_is_backed_up_but_staging_is_fresh() {
        let dir = fake_venv("fresh-staging-existing");
        let active = dir.join("venv");
        std::fs::write(active.join("broken-package-marker"), "damaged").unwrap();
        let python = dir.join("base-python");
        std::fs::write(
            &python,
            "#!/bin/bash\nmkdir -p \"$3/bin\"\ncp \"$0\" \"$3/bin/python3\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut transaction =
            VenvTransaction::prepare_with_python(&active, 92, "2.0.0", &dir.join("pin"), &python)
                .unwrap();
        assert!(transaction.record.had_active);
        assert!(transaction.python().exists());
        assert!(
            !transaction
                .record
                .staged
                .join("broken-package-marker")
                .exists(),
            "坏 active 内容不能复制到 pip staging"
        );
        assert!(
            !transaction.record.staged.join("installed").exists(),
            "staging 必须来自基础 Python,不是旧 venv"
        );

        transaction.activate().unwrap();
        assert_eq!(
            std::fs::read_to_string(transaction.record.backup.join("installed")).unwrap(),
            "old",
            "旧环境仍须留在事务备份中以支持回滚"
        );
        transaction.rollback().unwrap();
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_0600_is_atomic_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("qp-aw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("supervisor.pid");
        let target_s = target.to_string_lossy().to_string();

        atomic_write_0600(&target_s, "1234\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "1234\n");

        // 覆盖写:读者要么看到旧值,要么看到新值,不会看到空文件
        atomic_write_0600(&target_s, "5678\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "5678\n");

        // 临时文件必须已 rename 走,目录里只剩目标文件
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "supervisor.pid")
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");

        // 权限必须是 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "pid 文件权限应为 0600,实际 {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_stale_tmp_files_removes_only_regular_tmp_files() {
        let dir = std::env::temp_dir().join(format!("qp-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.to_str().unwrap().to_string();

        let keep = dir.join("qwenpaw-version");
        std::fs::write(&keep, "2.1.0").unwrap();
        std::fs::write(dir.join("service_port.tmp"), "x").unwrap();
        std::fs::write(dir.join("service_port.tmp.4242"), "x").unwrap();
        std::fs::create_dir_all(dir.join("venv.tmpdir")).unwrap();

        let removed = sweep_stale_tmp_files(&root);
        assert_eq!(removed, 2, "两个 tmp 普通文件都应被清理");
        assert!(keep.exists(), "正式文件不得被清理");
        assert!(!dir.join("service_port.tmp").exists());
        assert!(!dir.join("service_port.tmp.4242").exists());
        assert!(
            dir.join("venv.tmpdir").exists(),
            "目录型残留交给事务恢复处理"
        );
        let _ = std::fs::remove_dir_all(&dir);
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

    #[cfg(unix)]
    #[test]
    fn relocates_exact_python_shebang_and_preserves_script_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("qp-shebang-exact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let staged = dir.join(".venv.staging.1.1");
        let active = dir.join("venv");
        let bin = staged.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink("python3", bin.join("python")).unwrap();
        std::os::unix::fs::symlink("python3", bin.join("python3.11")).unwrap();

        let python3_script = bin.join("qwenpaw-python3");
        let python_alias_script = bin.join("qwenpaw-python-alias");
        std::fs::write(
            &python3_script,
            format!(
                "#!{}\nprint('python3 body')\n",
                bin.join("python3").display()
            ),
        )
        .unwrap();
        std::fs::write(
            &python_alias_script,
            format!(
                "#!{}\nprint('alias body')\n",
                bin.join("python3.11").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&python3_script, std::fs::Permissions::from_mode(0o751)).unwrap();

        super::io::relocate_console_script_shebangs(&staged, &active).unwrap();

        assert_eq!(
            std::fs::read(&python3_script).unwrap(),
            format!(
                "#!{}\nprint('python3 body')\n",
                active.join("bin/python3").display()
            )
            .as_bytes()
        );
        assert_eq!(
            std::fs::read(&python_alias_script).unwrap(),
            format!(
                "#!{}\nprint('alias body')\n",
                active.join("bin/python3").display()
            )
            .as_bytes()
        );
        assert_eq!(
            std::fs::metadata(&python3_script)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751,
            "原执行权限必须保留"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn leaves_nonmatching_shebangs_and_script_bodies_unchanged() {
        let dir = std::env::temp_dir().join(format!("qp-shebang-unrelated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let staged = dir.join(".venv.staging.2.1");
        let active = dir.join("venv");
        let bin = staged.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let with_arguments = bin.join("with-arguments");
        let unrelated = bin.join("unrelated");
        let with_arguments_bytes = format!(
            "#!{} -s\nprint('unchanged')\n",
            bin.join("python3").display()
        )
        .into_bytes();
        let unrelated_bytes =
            format!("#!/usr/bin/python3\n# {}\n", bin.join("python3").display()).into_bytes();
        std::fs::write(&with_arguments, &with_arguments_bytes).unwrap();
        std::fs::write(&unrelated, &unrelated_bytes).unwrap();

        super::io::relocate_console_script_shebangs(&staged, &active).unwrap();

        assert_eq!(std::fs::read(with_arguments).unwrap(), with_arguments_bytes);
        assert_eq!(std::fs::read(unrelated).unwrap(), unrelated_bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinked_scripts_and_directories_without_following_them() {
        let dir = std::env::temp_dir().join(format!("qp-shebang-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let staged = dir.join(".venv.staging.3.1");
        let active = dir.join("venv");
        let bin = staged.join("bin");
        let outside = dir.join("outside");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let external_script = outside.join("external-script");
        let linked_directory_script = outside.join("linked-dir-script");
        let original = format!("#!{}\nprint('outside')\n", bin.join("python3").display());
        std::fs::write(&external_script, &original).unwrap();
        std::fs::write(&linked_directory_script, &original).unwrap();
        std::os::unix::fs::symlink("../outside/external-script", bin.join("script-link")).unwrap();
        std::os::unix::fs::symlink("../outside", bin.join("directory-link")).unwrap();

        super::io::relocate_console_script_shebangs(&staged, &active).unwrap();

        assert_eq!(std::fs::read_to_string(external_script).unwrap(), original);
        assert_eq!(
            std::fs::read_to_string(linked_directory_script).unwrap(),
            original
        );
        assert!(std::fs::symlink_metadata(bin.join("script-link"))
            .unwrap()
            .file_type()
            .is_symlink());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn activation_propagates_relocation_failure_and_preserves_old_active_venv() {
        let dir = fake_venv("relocation-failure");
        let active = dir.join("venv");
        let pin = dir.join("pin");
        let mut transaction = VenvTransaction::prepare_with_python(
            &active,
            91,
            "2.0.0",
            &pin,
            Path::new(&dir.join("venv/base-python")),
        )
        .unwrap();
        std::fs::remove_dir_all(transaction.record.staged.join("bin")).unwrap();
        std::fs::write(transaction.record.staged.join("bin"), "not a directory").unwrap();

        let activation = transaction.activate();
        assert!(activation.is_err(), "重定位错误必须中止激活");
        assert_eq!(transaction.record.phase, TxPhase::Prepared);
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old",
            "失败不能先移动活动旧环境"
        );
        transaction.rollback().unwrap();
        assert_eq!(
            std::fs::read_to_string(active.join("installed")).unwrap(),
            "old",
            "现有回滚路径必须保留旧环境"
        );
        assert!(!transaction.record.staged.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
