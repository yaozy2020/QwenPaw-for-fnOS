#!/usr/bin/env python3
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(
    os.environ.get(
        "QWENPAW_TEST_SUP_BIN",
        ROOT / "supervisor/target/release/qwenpaw-supervisor",
    )
)
TOKEN = "isolated_test_token_12345678901234567890"

TEST_RUNTIME_BIN = Path(
    os.environ.get(
        "QWENPAW_TEST_RUNTIME_BIN",
        ROOT / "supervisor/target/release/qwenpaw-test-runtime",
    )
)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def read_process(pid):
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
        fields = raw[raw.rfind(")") + 2:].split()
        executable = os.readlink(f"/proc/{pid}/exe")
        if executable.endswith(" (deleted)"):
            executable = executable[:-10]
        identity = (pid, int(fields[19]), executable)
        cmdline = Path(f"/proc/{pid}/cmdline").read_bytes()
        return identity, fields[0], int(fields[1]), cmdline
    except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError, ValueError, OSError):
        return None


def process_is_alive(identity):
    current = read_process(identity[0])
    return current is not None and current[0] == identity and current[1] not in ("Z", "X")


def signal_owned_process(identity, signum):
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise AssertionError("safe process signaling requires pidfd_open and pidfd_send_signal")
    current = read_process(identity[0])
    if current is None or current[1] in ("Z", "X"):
        return False
    assert current[0] == identity, f"refusing to signal a process whose identity changed: pid={identity[0]}"
    try:
        pidfd = os.pidfd_open(identity[0])
    except ProcessLookupError:
        return False
    try:
        current = read_process(identity[0])
        if current is None or current[1] in ("Z", "X"):
            return False
        assert current[0] == identity, f"refusing to signal a reused pid: pid={identity[0]}"
        signal.pidfd_send_signal(pidfd, signum)
        return True
    finally:
        os.close(pidfd)


def wait_for_process_exit(identity, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not process_is_alive(identity):
            return
        time.sleep(0.05)
    raise AssertionError(f"owned process remained alive: pid={identity[0]}")


def process_group(pid):
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
        return int(raw[raw.rfind(")") + 2:].split()[2])
    except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError, ValueError, OSError):
        return None


def process_tree(root_pid):
    processes = {}
    try:
        pids = [int(entry.name) for entry in Path("/proc").iterdir() if entry.name.isdigit()]
    except OSError:
        return ()
    for pid in pids:
        info = read_process(pid)
        if info is not None:
            processes[pid] = info
    selected = {root_pid}
    while True:
        children = {pid for pid, info in processes.items() if info[2] in selected}
        new_children = children - selected
        if not new_children:
            break
        selected.update(new_children)
    return tuple(sorted(processes[pid][0] for pid in selected if pid in processes))


def filesystem_snapshot(root):
    snapshot = []
    for path in sorted(root.rglob("*")):
        info = path.lstat()
        common = (info.st_ino, info.st_mode, info.st_mtime_ns)
        if stat.S_ISLNK(info.st_mode):
            value = ("symlink", os.readlink(path), *common)
        elif stat.S_ISDIR(info.st_mode):
            value = ("directory", *common)
        elif stat.S_ISSOCK(info.st_mode):
            value = ("socket", *common)
        else:
            value = ("file", info.st_size, path.read_bytes(), *common)
        snapshot.append((str(path.relative_to(root)), value))
    return tuple(snapshot)


def wait_for(predicate, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError("timed out waiting for isolated supervisor")


class Fixture:
    def __init__(self, root, name, binary_sha256, runtime_sha256):
        self.root = root / name
        for directory in ("home/venv/bin", "var", "share", "etc", "app/ui", "app/bin"):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        self.port = free_port()
        self.sup_port = free_port()
        self.python = self.root / "home/venv/bin/python3"
        shutil.copy2(TEST_RUNTIME_BIN, self.python)
        copied_runtime_sha256 = hashlib.sha256(self.python.read_bytes()).hexdigest()
        assert copied_runtime_sha256 == runtime_sha256, f"release helper changed during copy: {copied_runtime_sha256} != {runtime_sha256}"
        (self.root / "home/venv/installed").write_text("1.0.0\n")
        (self.root / "var/qwenpaw-version").write_text("1.0.0\n")
        (self.root / "var/service_port").write_text(f"{self.port}\n")
        (self.root / "var/qwenpaw-log-level").write_text("info\n")
        (self.root / "var/supervisor-token").write_text(TOKEN)
        self.ui = self.root / "app/ui/config"
        self.ui.write_text(json.dumps({".url": {
            "qwenpaw.main": {"port": str(self.port), "noDisplay": False},
            "qwenpaw.console": {
                "url": "/app/qwenpaw/", "gatewayPrefix": "/app/qwenpaw",
                "gatewaySocket": "app.sock", "type": "iframe",
            },
        }}))
        shutil.copy2(BIN, self.root / "app/bin/qwenpaw-supervisor")
        self.binary = self.root / "app/bin/qwenpaw-supervisor"
        copied_sha256 = hashlib.sha256(self.binary.read_bytes()).hexdigest()
        assert copied_sha256 == binary_sha256, f"release binary changed during copy: {copied_sha256} != {binary_sha256}"
        self.env = dict(os.environ, TRIM_PKGHOME=str(self.root / "home"), TRIM_PKGVAR=str(self.root / "var"), TRIM_PKGETC=str(self.root / "etc"), TRIM_DATA_SHARE_PATHS=str(self.root / "share"), TRIM_APPDEST=str(self.root / "app"), TRIM_SERVICE_PORT=str(self.port), SUPERVISOR_PORT=str(self.sup_port), SUPERVISOR_AUTOSTART="0", QWENPAW_WEB_PROXY_PORT="off", QWENPAW_PYTHON_BIN=str(self.python), QWENPAW_TEST_CONTROL=str(self.root), QWENPAW_SUP_URL=f"http://127.0.0.1:{self.sup_port}")
        self.env.pop("QWENPAW_CRASH_POINT", None)
        self.env.pop("QWENPAW_ENABLE_CRASH_POINT", None)
        self.process = None
        self.process_identity = None
        self.last_exit_code = None
        self.owned_descendants = []
        self.log = None

    def start(self):
        assert self.process is None
        self.log = (self.root / "process.log").open("ab")
        self.process = subprocess.Popen([str(self.binary)], env=self.env, stdout=self.log, stderr=self.log)
        def capture_identity():
            if self.process.poll() is not None:
                raise AssertionError((self.root / "process.log").read_text())
            info = read_process(self.process.pid)
            if info is None or Path(info[0][2]) != self.binary:
                return False
            self.process_identity = info[0]
            if not info[3]:
                return False
            expected_argv0 = os.fsencode(str(self.binary)) + bytes((0,))
            assert info[3].startswith(expected_argv0), info[3]
            return True
        wait_for(capture_identity)
        self.last_exit_code = None
        wait_for(lambda: self.ready())

    def ready(self):
        if self.process.poll() is not None:
            raise AssertionError((self.root / "process.log").read_text())
        try:
            return self.request("/status")[0] == 200
        except (OSError, urllib.error.URLError):
            return False

    def request(self, path, body=None, method=None, token=TOKEN, include_token=True):
        if method is None:
            method = "POST" if body is not None else "GET"
        data = None if body is None else json.dumps(body).encode()
        headers = {"Content-Type": "application/json"}
        if include_token:
            headers["X-Supervisor-Token"] = token
        req = urllib.request.Request(self.env["QWENPAW_SUP_URL"] + path, data=data, headers=headers, method=method)
        try:
            response = urllib.request.urlopen(req, timeout=75)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            payload = response.read()
            try:
                result = json.loads(payload) if payload else None
            except (json.JSONDecodeError, UnicodeDecodeError):
                result = None
            return response.status, result

    def remember_owned_descendants(self):
        if self.process is None:
            return
        known = {identity[0]: identity for identity in self.owned_descendants}
        for identity in process_tree(self.process.pid):
            if identity[0] != self.process.pid:
                known[identity[0]] = identity
        self.owned_descendants = list(known.values())

    def stop(self, expected_exit_code=0):
        if self.process is None:
            return
        self.remember_owned_descendants()
        if self.process.poll() is None:
            assert self.process_identity is not None
            signal_owned_process(self.process_identity, signal.SIGTERM)
            try:
                self.process.wait(timeout=29)
            except subprocess.TimeoutExpired:
                signal_owned_process(self.process_identity, signal.SIGKILL)
                self.process.wait()
                raise AssertionError("graceful shutdown exceeded fnOS stop budget")
        self.last_exit_code = self.process.returncode
        if expected_exit_code is not None:
            assert self.last_exit_code == expected_exit_code, f"supervisor exit code {self.last_exit_code}, expected {expected_exit_code}"
        self.process = None
        self.process_identity = None
        if self.log:
            self.log.close()
            self.log = None

    def kill_for_crash(self):
        assert self.process is not None and self.process.poll() is None
        assert self.process_identity is not None
        self.remember_owned_descendants()
        signal_owned_process(self.process_identity, signal.SIGKILL)
        self.process.wait(timeout=5)
        self.last_exit_code = self.process.returncode
        assert self.last_exit_code == -signal.SIGKILL, f"SIGKILL exit code was {self.last_exit_code}"
        self.process = None
        self.process_identity = None
        if self.log:
            self.log.close()
            self.log = None

    def cleanup_owned_descendants(self):
        for identity in self.owned_descendants:
            if process_is_alive(identity):
                signal_owned_process(identity, signal.SIGKILL)
                wait_for_process_exit(identity)


def api_routes():
    return [
        ("GET", "/status", None),
        ("POST", "/start", {}),
        ("POST", "/stop", {}),
        ("POST", "/restart", {}),
        ("GET", "/events", None),
        ("GET", "/logs", None),
        ("POST", "/logs/clear", {}),
        ("POST", "/kernel/install", {"version": "2.0.0"}),
        ("POST", "/kernel/uninstall", {}),
        ("GET", "/kernel/install-status", None),
        ("GET", "/kernel/install-log", None),
        ("GET", "/kernel/versions", None),
        ("POST", "/kernel/pin", {"version": "3.1.4"}),
        ("POST", "/kernel/reset-auth", None),
        ("POST", "/port", {"port": free_port()}),
        ("POST", "/settings", {"port": free_port(), "log_level": "debug"}),
    ]


def side_effect_snapshot(fixture):
    assert fixture.process is not None and fixture.process_identity is not None
    return filesystem_snapshot(fixture.root), process_tree(fixture.process.pid)


def assert_no_side_effects(fixture, before, label):
    after = side_effect_snapshot(fixture)
    assert after == before, f"{label} changed isolated files or supervisor process tree"


def test_authentication_and_methods(fixture):
    fixture.start()
    code, result = fixture.request("/start", {})
    assert code == 200 and result["success"], result
    wait_for(lambda: fixture.request("/status")[1]["running"])
    auth_file = fixture.root / ".qwenpaw.secret/auth.json"
    auth_file.parent.mkdir(parents=True)
    auth_file.write_text("auth-must-remain-unchanged")
    routes = api_routes()

    for method, path, body in routes:
        for label, options in (
            ("missing token", {"include_token": False}),
            ("wrong token", {"token": "wrong-isolated-test-token"}),
        ):
            before = side_effect_snapshot(fixture)
            code, result = fixture.request(path, body, method=method, **options)
            assert code == 401 and result and result.get("success") is False, (path, label, code, result)
            assert_no_side_effects(fixture, before, f"{path} {label}")

        wrong_method = "POST" if method == "GET" else "GET"
        before = side_effect_snapshot(fixture)
        wrong_body = {} if wrong_method == "POST" else None
        code, _ = fixture.request(path, wrong_body, method=wrong_method)
        assert code == 405, (path, wrong_method, code)
        assert_no_side_effects(fixture, before, f"{path} {wrong_method}")

    assert auth_file.read_text() == "auth-must-remain-unchanged"
    assert fixture.request("/status")[1]["running"]
    print(f"PASS: {len(routes)} routes reject missing/wrong tokens and wrong methods with zero fixture side effects")


def test_settings(fixture):
    fixture.start()
    assert fixture.request("/start", {})[0] == 200
    wait_for(lambda: fixture.request("/status")[1]["running"])
    drift = json.loads(fixture.ui.read_text())
    drift[".url"]["qwenpaw.main"]["port"] = "18888"
    fixture.ui.write_text(json.dumps(drift))
    before = fixture.ui.read_bytes(), fixture.ui.stat().st_mtime_ns, fixture.ui.stat().st_ino
    assert fixture.request("/status")[0] == 200
    assert before == (fixture.ui.read_bytes(), fixture.ui.stat().st_mtime_ns, fixture.ui.stat().st_ino)
    target = free_port()
    code, result = fixture.request("/settings", {"port": target, "log_level": "debug"})
    assert code == 200 and result["success"], result
    assert fixture.request("/status")[1]["port"] == target
    assert (fixture.root / "var/qwenpaw-log-level").read_text().strip() == "debug"
    assert json.loads(fixture.ui.read_text())[".url"]["qwenpaw.console"] == {
        "url": "/app/qwenpaw/", "gatewayPrefix": "/app/qwenpaw",
        "gatewaySocket": "app.sock", "type": "iframe",
    }
    fail_port = free_port()
    (fixture.root / "fail-port").write_text(str(fail_port))
    files = [fixture.root / "var/service_port", fixture.root / "var/qwenpaw-log-level", fixture.ui]
    original = [path.read_bytes() for path in files]
    code, result = fixture.request("/settings", {"port": fail_port, "log_level": "trace"})
    assert code == 500 and not result["success"], result
    assert [path.read_bytes() for path in files] == original
    assert fixture.request("/status")[1]["port"] == target
    callback_port = free_port()
    callback = subprocess.run(["bash", str(ROOT / "cmd/config_callback")], env=dict(fixture.env, wizard_port=str(callback_port), wizard_log_level="warning"), capture_output=True, text=True, timeout=90)
    assert callback.returncode == 0, callback.stdout + callback.stderr
    assert fixture.process.poll() is None
    assert fixture.request("/status")[1]["port"] == callback_port
    assert (fixture.root / "var/qwenpaw-log-level").read_text().strip() == "warning"
    gateway = json.loads(fixture.ui.read_text())[".url"]["qwenpaw.console"]
    assert gateway["url"] == "/app/qwenpaw/" and gateway["gatewaySocket"] == "app.sock"
    assert fixture.request("/status")[1]["port"] == callback_port
    print("PASS: real HTTP configuration, exact rollback, callback → supervisor → kernel, Gateway console config, read-only status")


def test_recovery_gate(fixture):
    (fixture.root / "var/settings-transaction.json").write_text("corrupt journal")
    fixture.env["SUPERVISOR_AUTOSTART"] = "1"
    fixture.start()
    status = fixture.request("/status")[1]
    assert not status["running"] and status["install"]["recovery_required"], status
    assert status["install"]["recovery_error"]
    for path, body in [("/start", {}), ("/kernel/install", {"version": "2.0.0"}), ("/settings", {"port": free_port()}), ("/port", {"port": free_port()}), ("/kernel/uninstall", {})]:
        code, result = fixture.request(path, body)
        assert code == 409 and not result["success"], (path, code, result)
    assert (fixture.root / "var/settings-transaction.json").read_text() == "corrupt journal"
    assert not (fixture.root / "pip-started").exists()
    print("PASS: corrupt recovery journal blocks autostart and mutations without blocking diagnostics")


def test_shutdown(fixture):
    fixture.start()
    (fixture.root / "slow-pip").touch()
    code, result = fixture.request("/kernel/install", {"version": "2.0.0"})
    assert code == 200 and result["success"], result
    wait_for(lambda: (fixture.root / "pip-started").exists())
    fixture.stop(expected_exit_code=0)
    assert fixture.last_exit_code == 0
    assert (fixture.root / "home/venv/installed").read_text().strip() == "1.0.0"
    assert (fixture.root / "var/qwenpaw-version").read_text().strip() == "1.0.0"
    transaction_records = sorted(fixture.root.joinpath("home").glob(".venv.transaction.*.json"))
    staging_dirs = sorted(fixture.root.joinpath("home").glob(".venv.staging.*"))
    if transaction_records or staging_dirs:
        line_sep = chr(10)
        record_dump = line_sep.join(
            f"{path}: {path.read_text(errors='replace')}" for path in transaction_records
        )
        process_log = (fixture.root / "process.log").read_text(errors="replace")
        pip_log_path = fixture.root / "var/pip.log"
        pip_log = pip_log_path.read_text(errors="replace") if pip_log_path.exists() else "<missing>"
        raise AssertionError(
            "shutdown left install recovery evidence; "
            f'fixture={fixture.root} supervisor_status="unavailable after exit"{line_sep}'
            f"records={record_dump or '<none>'}{line_sep}"
            f"staging={[str(path) for path in staging_dirs]}{line_sep}"
            f"process.log={process_log}{line_sep}"
            f"pip.log={pip_log}"
        )
    fixture.start()
    status = fixture.request("/status")[1]
    assert not status["running"] and not status["install"]["recovery_required"], status
    print("PASS: SIGTERM exits supervisor 0, cancels pip, rolls back, and does not resurrect the kernel")


def test_process_tree_shutdown(fixture):
    sentinel = None
    sentinel_identity = None
    fixture.start()
    (fixture.root / "spawn-process-tree").touch()
    code, result = fixture.request("/start", {})
    assert code == 200 and result["success"], result
    kernel_pid = int((fixture.root / "var/kernel.pid").read_text().strip())
    descendants_file = fixture.root / "target-descendants"
    wait_for(descendants_file.is_file)
    descendant_pids = [int(line) for line in descendants_file.read_text().splitlines()]
    wait_for(lambda: all(
        read_process(pid) is not None and read_process(pid)[1] not in ("Z", "X")
        for pid in descendant_pids
    ))
    target_tree = process_tree(kernel_pid)
    target_identities = {identity[0]: identity for identity in target_tree}
    assert kernel_pid in target_identities
    assert set(descendant_pids).issubset(target_identities), (descendant_pids, sorted(target_identities))
    groups = {process_group(pid) for pid in descendant_pids}
    assert None not in groups and kernel_pid not in groups and len(groups) == len(descendant_pids), (groups, descendant_pids)
    fixture.owned_descendants = [identity for pid, identity in target_identities.items() if pid != kernel_pid]

    sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)
    wait_for(lambda: read_process(sentinel.pid) is not None)
    sentinel_identity = read_process(sentinel.pid)[0]
    assert process_is_alive(sentinel_identity)
    try:
        fixture.stop(expected_exit_code=0)
        assert fixture.last_exit_code == 0
        generation = json.loads((fixture.root / "var/shutdown-generation.json").read_text())
        assert generation["state"] == "clean", f"shutdown generation was not clean: {generation}"
        wait_for(lambda: all(not process_is_alive(identity) for identity in target_tree))
        assert all(not process_is_alive(identity) for identity in target_tree), target_tree
        assert process_is_alive(sentinel_identity), "unrelated sentinel must survive supervisor shutdown"
        print(f"PASS: supervisor exit 0; {len(target_tree) - 1} target descendants exited; unrelated sentinel survived")
    finally:
        fixture.stop()
        fixture.cleanup_owned_descendants()
        if sentinel is not None and sentinel.poll() is None:
            assert sentinel_identity is not None
            signal_owned_process(sentinel_identity, signal.SIGTERM)
        if sentinel is not None:
            sentinel.wait(timeout=5)


def test_settings_crash_recovery(fixture):
    def original_kernel_ready():
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{fixture.port}/api/version", timeout=1) as response:
                return json.load(response)["version"] == "1.0.0"
        except (OSError, urllib.error.URLError):
            return False

    fixture.start()
    assert fixture.request("/start", {})[0] == 200
    wait_for(original_kernel_ready)
    files = [fixture.root / "var/service_port", fixture.root / "var/qwenpaw-log-level", fixture.ui]
    original = [path.read_bytes() for path in files]
    (fixture.root / "pause-kernel").touch()
    worker = subprocess.Popen([
        "curl", "--noproxy", "*", "--silent", "--show-error", "--max-time", "60",
        "--header", f"X-Supervisor-Token: {TOKEN}", "--header", "Content-Type: application/json",
        "--data", json.dumps({"port": free_port(), "log_level": "debug"}),
        fixture.env["QWENPAW_SUP_URL"] + "/settings",
    ], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    wait_for(lambda: read_process(worker.pid) is not None)
    worker_identity = read_process(worker.pid)[0]
    try:
        wait_for(lambda: (fixture.root / "kernel-paused").exists()
                 and (fixture.root / "var/kernel.pid").exists())
        assert (fixture.root / "var/settings-transaction.json").is_file()
        fixture.kill_for_crash()
        worker.communicate(timeout=5)
        assert worker.returncode != 0
    finally:
        if worker.poll() is None:
            signal_owned_process(worker_identity, signal.SIGKILL)
            worker.communicate()
    fixture.stop()
    (fixture.root / "pause-kernel").unlink()
    fixture.start()
    status = fixture.request("/status")[1]
    assert [path.read_bytes() for path in files] == original
    assert status["port"] == fixture.port and not status["running"], status
    assert not status["install"]["recovery_required"], status
    assert not (fixture.root / "var/settings-transaction.json").exists()
    assert fixture.request("/start", {})[0] == 200
    wait_for(original_kernel_ready)
    print("PASS: SIGKILL during settings restart restores exact files and in-memory configuration before API startup")


def test_disconnected_settings_finishes_rollback(fixture):
    import struct

    fixture.start()
    assert fixture.request("/start", {})[0] == 200
    wait_for(lambda: fixture.request("/status")[1]["running"])
    files = [fixture.root / "var/service_port", fixture.root / "var/qwenpaw-log-level", fixture.ui]
    original = [path.read_bytes() for path in files]
    pause = fixture.root / "pause-kernel"
    pause.touch()
    journal = fixture.root / "var/settings-transaction.json"
    body = json.dumps({"port": free_port(), "log_level": "debug"}).encode()
    with socket.create_connection(("127.0.0.1", fixture.sup_port), timeout=5) as stream:
        stream.sendall((
            f"POST /settings HTTP/1.1\r\nHost: localhost\r\nX-Supervisor-Token: {TOKEN}\r\n"
            f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n"
        ).encode() + body)
        wait_for(lambda: journal.exists() and (fixture.root / "kernel-paused").exists())
        assert fixture.request("/mirror", {"mirror": ""})[0] == 409
        stream.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
    # Let the old request's connection task observe RST before checking its gate.
    time.sleep(0.1)
    assert fixture.request("/mirror", {"mirror": ""})[0] == 409, "disconnect released the operation lock"
    pause.unlink()
    wait_for(lambda: not journal.exists(), timeout=25)
    assert [path.read_bytes() for path in files] == original
    wait_for(lambda: fixture.request("/mirror", {"mirror": ""})[0] == 200)
    assert not fixture.request("/kernel/install-status")[1]["recovery_required"]
    print("PASS: disconnected configuration retains the operation lock through rollback and restores files")


def test_legacy_uninstall_pin_recovery(fixture):
    pin = fixture.root / "var/qwenpaw-version"
    backup = fixture.root / "var/.qwenpaw-version.uninstall.12.34"
    pin.rename(backup)
    fixture.start()
    assert fixture.python.is_file()
    assert pin.read_text() == "1.0.0\n"
    assert not backup.exists()
    assert not fixture.request("/kernel/install-status")[1]["recovery_required"]
    print("PASS: legacy uninstall interrupted before venv move restores the exact version pin")


def test_committed_uninstall_recovery(fixture):
    var = fixture.root / "var"
    home = fixture.root / "home"
    (home / "venv").rename(home / ".venv.uninstall.12.34")
    (var / "qwenpaw-version").rename(var / ".qwenpaw-version.uninstall.12.34")
    (var / "kernel-uninstall.json").write_text(json.dumps({
        "schema": 1, "id": "12.34", "committed": True, "had_runtime": True, "had_pin": True,
    }))
    fixture.start()
    assert not (home / "venv").exists()
    assert not (home / ".venv.uninstall.12.34").exists()
    assert not (var / "qwenpaw-version").exists()
    assert not (var / ".qwenpaw-version.uninstall.12.34").exists()
    assert not (var / "kernel-uninstall.json").exists()
    assert not fixture.request("/kernel/install-status")[1]["recovery_required"]
    print("PASS: committed uninstall startup finishes cleanup without restoring removed runtime")


def test_auth_reset_recovery(fixture):
    secret = fixture.root / "share/.qwenpaw.secret"
    secret.mkdir()
    auth = secret / "auth.json"
    backup = secret / "auth.json.bak-reset"
    journal = fixture.root / "var/auth-reset.json"
    auth.write_text("new-auth")
    backup.write_text("old-auth")
    fixture.start()
    assert fixture.request("/kernel/install-status")[1]["recovery_required"]
    assert fixture.request("/mirror", {"mirror": ""})[0] == 409
    assert auth.read_text() == "new-auth" and backup.read_text() == "old-auth"
    fixture.stop()
    journal.write_text(json.dumps({"schema": 1, "committed": False}))
    fixture.start()
    assert auth.read_text() == "old-auth"
    assert not backup.exists() and not journal.exists()
    assert not fixture.request("/kernel/install-status")[1]["recovery_required"]
    fixture.stop()
    auth.rename(backup)
    auth.write_text("committed-new-auth")
    journal.write_text(json.dumps({"schema": 1, "committed": True}))
    fixture.start()
    assert auth.read_text() == "committed-new-auth"
    assert not backup.exists() and not journal.exists()
    assert not fixture.request("/kernel/install-status")[1]["recovery_required"]
    print("PASS: auth reset blocks ambiguous legacy state, restores prepared state, and keeps committed credentials")


def test_proxy_settings_recovery(fixture):
    var = fixture.root / "var"
    port = free_port()
    journal = var / "web-proxy-transaction.json"
    journal.write_text(json.dumps({
        "schema": 1,
        "port": {"bytes": list(f"{port}\n".encode()), "mode": 0o600},
        "public": {"bytes": list(b"0\n"), "mode": 0o600},
    }))
    (var / "web_proxy_port").write_text(f"{free_port()}\n")
    (var / "web_proxy_public").write_text("1\n")
    fixture.env.pop("QWENPAW_WEB_PROXY_PORT", None)
    fixture.env.pop("QWENPAW_WEB_PROXY_PUBLIC", None)
    fixture.start()
    result = fixture.request("/web-proxy/port")[1]
    assert result["port"] == port and result["public"] is False, result
    assert not journal.exists()
    assert (var / "web_proxy_public").read_text() == "0\n"
    fixture.stop()
    journal.write_text("broken")
    fixture.start()
    assert fixture.request("/kernel/install-status")[1]["recovery_required"]
    assert fixture.request("/web-proxy/port")[0] == 404
    assert journal.read_text() == "broken"
    print("PASS: proxy port and access recover together; corrupt state disables proxy while management remains available")


def main():
    if not BIN.is_file():
        raise SystemExit(f"test supervisor binary not found: {BIN}")
    if not TEST_RUNTIME_BIN.is_file():
        raise SystemExit(f"test runtime helper not found: {TEST_RUNTIME_BIN}")
    binary_sha256 = hashlib.sha256(BIN.read_bytes()).hexdigest()
    runtime_sha256 = hashlib.sha256(TEST_RUNTIME_BIN.read_bytes()).hexdigest()
    print(f"TEST_SUPERVISOR_SHA256={binary_sha256}", flush=True)
    print(f"TEST_RUNTIME_HELPER_SHA256={runtime_sha256}", flush=True)
    print(f"TEST_BINARY_SHA256={binary_sha256}", flush=True)
    with tempfile.TemporaryDirectory(prefix="qwenpaw-api-") as directory:
        fixtures = []
        try:
            cases = [
                ("settings", test_settings),
                ("auth-methods", test_authentication_and_methods),
                ("settings-crash", test_settings_crash_recovery),
                ("settings-disconnect", test_disconnected_settings_finishes_rollback),
                ("legacy-uninstall", test_legacy_uninstall_pin_recovery),
                ("uninstall-committed", test_committed_uninstall_recovery),
                ("auth-reset-recovery", test_auth_reset_recovery),
                ("proxy-recovery", test_proxy_settings_recovery),
                ("gate", test_recovery_gate),
                ("shutdown", test_shutdown),
                ("process-tree", test_process_tree_shutdown),
            ]
            for name, check in cases:
                fixture = Fixture(Path(directory), name, binary_sha256, runtime_sha256)
                fixtures.append(fixture)
                check(fixture)
                fixture.stop()
            current_sha256 = hashlib.sha256(BIN.read_bytes()).hexdigest()
            assert current_sha256 == binary_sha256, f"release binary changed during test: {current_sha256} != {binary_sha256}"
            current_runtime_sha256 = hashlib.sha256(TEST_RUNTIME_BIN.read_bytes()).hexdigest()
            assert current_runtime_sha256 == runtime_sha256, f"release helper changed during test: {current_runtime_sha256} != {runtime_sha256}"
        finally:
            for fixture in fixtures:
                try:
                    fixture.stop()
                finally:
                    fixture.cleanup_owned_descendants()


if __name__ == "__main__":
    main()
