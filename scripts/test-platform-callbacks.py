#!/usr/bin/env python3
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
SUPERVISOR_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_SUP_BIN", ROOT / "supervisor/target/release/qwenpaw-supervisor"
)).resolve()
RUNTIME_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_RUNTIME_BIN", ROOT / "supervisor/target/release/qwenpaw-test-runtime"
)).resolve()
CALLBACKS = {
    "install": (ROOT / "cmd/install_callback", "install"),
    "upgrade": (ROOT / "cmd/upgrade_callback", "upgrade"),
    "uninstall": (ROOT / "cmd/uninstall_callback", "uninstall"),
}
TERMINAL_PHASES = {"done", "failed"}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def free_port(excluded=()):
    while True:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        if port not in excluded:
            return port


def read_process(pid):
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
        fields = raw[raw.rfind(")") + 2:].split()
        executable = Path(f"/proc/{pid}/exe").resolve(strict=True)
        metadata = executable.stat()
        argv = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        environ = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        return {
            "pid": int(pid),
            "starttime": int(fields[19]),
            "state": fields[0],
            "ppid": int(fields[1]),
            "exe": executable,
            "exe_id": (metadata.st_dev, metadata.st_ino),
            "argv": tuple(arg for arg in argv if arg),
            "environ": frozenset(item for item in environ if item),
        }
    except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError, ValueError, OSError):
        return None


def alive(identity):
    current = read_process(identity["pid"])
    return current is not None and identity_matches(current, identity) and current["state"] not in {"Z", "X"}


def identity_matches(current, expected):
    return all(current[key] == expected[key] for key in ("pid", "starttime", "exe_id"))


def env_contains(info, key, value):
    return os.fsencode(f"{key}={value}") in info["environ"]


def wait_for(predicate, timeout, message):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (FileNotFoundError, ConnectionError, OSError, ValueError) as error:
            last_error = error
        time.sleep(0.025)
    detail = f": {last_error}" if last_error else ""
    raise AssertionError(f"timed out waiting for {message}{detail}")


def pidfd_signal(identity, signum, fixture):
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
            "safe process cleanup requires pidfd_open and pidfd_send_signal")
    current = read_process(identity["pid"])
    if current is None or current["state"] in {"Z", "X"}:
        return False
    require(identity_matches(current, identity),
            f"refusing to signal changed process identity pid={identity['pid']}")
    require(env_contains(current, "TRIM_PKGVAR", fixture.var),
            f"refusing to signal process outside the temporary TRIM root pid={identity['pid']}")
    pidfd = os.pidfd_open(identity["pid"])
    try:
        current = read_process(identity["pid"])
        if current is None or current["state"] in {"Z", "X"}:
            return False
        require(identity_matches(current, identity),
                f"process identity changed before pidfd signal pid={identity['pid']}")
        require(env_contains(current, "TRIM_PKGVAR", fixture.var),
                f"process left the temporary TRIM root before pidfd signal pid={identity['pid']}")
        signal.pidfd_send_signal(pidfd, signum)
        return True
    finally:
        os.close(pidfd)


class Fixture:
    def __init__(self, parent, name, source_hashes, fail_pip=False):
        self.root = parent / name
        self.home = self.root / "home"
        self.var = self.root / "var"
        self.etc = self.root / "etc"
        self.share = self.root / "share"
        self.user_data = self.share / ".qwenpaw"
        self.user_secret = self.share / ".qwenpaw.secret"
        self.app = self.root / "target"
        self.binary = self.app / "app/bin/qwenpaw-supervisor"
        self.runtime_source = self.root / "qwenpaw-test-runtime"
        for path in (self.home, self.var, self.etc, self.user_data, self.user_secret,
                     self.app / "app/bin", self.app / "ui"):
            path.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SUPERVISOR_SOURCE, self.binary)
        shutil.copy2(RUNTIME_SOURCE, self.runtime_source)
        os.chmod(self.binary, 0o755)
        os.chmod(self.runtime_source, 0o755)
        require(sha256(self.binary) == source_hashes["supervisor"],
                f"copied supervisor SHA256 mismatch in {name}")
        require(sha256(self.runtime_source) == source_hashes["runtime"],
                f"copied test runtime SHA256 mismatch in {name}")
        self.supervisor_port = free_port()
        self.service_port = free_port((self.supervisor_port,))
        self.ui_config = self.app / "ui/config"
        self.ui_config.write_text(json.dumps({
            ".url": {"qwenpaw.main": {"title": "QwenPaw", "port": str(self.service_port), "url": "/"}},
            "sentinel": "keep-outside-app-data",
        }))
        (self.var / "service_port").write_text(f"{self.service_port}\n")
        (self.var / "qwenpaw-log-level").write_text("info\n")
        self.control = self.root / "control"
        self.control.mkdir()
        self.sentinel = self.root / "outer-sentinel"
        self.sentinel.write_text("must survive uninstall remove\n")
        self.env = os.environ.copy()
        for key in (
            "QWENPAW_CRASH_POINT", "QWENPAW_ENABLE_CRASH_POINT", "QWENPAW_RACE_READY_FILE",
            "QWENPAW_RACE_GO_FILE", "QWENPAW_TEST_RUNTIME_INSTALL_VERSION",
        ):
            self.env.pop(key, None)
        self.env.update({
            "TRIM_APPDEST": str(self.app),
            "TRIM_PKGVAR": str(self.var),
            "TRIM_PKGHOME": str(self.home),
            "TRIM_PKGETC": str(self.etc),
            "TRIM_DATA_SHARE_PATHS": str(self.share),
            "TRIM_TEMP_LOGFILE": str(self.root / "platform.log"),
            "TRIM_SERVICE_PORT": str(self.service_port),
            "SUPERVISOR_PORT": str(self.supervisor_port),
            "SUPERVISOR_AUTOSTART": "1",
            "QWENPAW_PYTHON_BIN": str(self.runtime_source),
            "QWENPAW_TEST_CONTROL": str(self.control),
            "wizard_port": str(self.service_port),
            "wizard_log_level": "info",
            "wizard_pypi_mirror": "official",
        })
        if fail_pip:
            self.env["QWENPAW_TEST_FAIL_PIP"] = "1"
            (self.control / "fail-pip").write_text("yes\n")
        else:
            self.env.pop("QWENPAW_TEST_FAIL_PIP", None)
        self.manager_identities = []
        self.runtime_identities = []
        self.closed = False

    @property
    def venv(self):
        return self.home / "venv"

    @property
    def pin(self):
        return self.var / "qwenpaw-version"

    def create_runtime(self, version, *, bad_import=False, fail_pip=False):
        binary = self.venv / "bin/python3"
        binary.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(self.runtime_source, binary.with_name("python3.real"))
        os.chmod(binary.with_name("python3.real"), 0o755)
        binary.write_text("#!/bin/bash\n"
                          "if [ \"${1:-}\" = -m ] && [ \"${2:-}\" = pip ]; then\n"
                          "  if [ \"${QWENPAW_TEST_FAIL_PIP:-0}\" = 1 ]; then\n"
                          "    printf 'failed\\n' >> \"$QWENPAW_TEST_CONTROL/pip-attempts\"\n"
                          "    exit 42\n"
                          "  fi\n"
                          "fi\n"
                          "if [ \"${1:-}\" = -c ] && [[ \"${2:-}\" == *'import qwenpaw'* ]] \\\n"
                          "    && [ \"$(stat -Lc '%i' -- \"$0\")\" = \"${QWENPAW_TEST_BAD_PYTHON_INODE:-}\" ]; then\n"
                          "  exit 43\n"
                          "fi\n"
                          "exec \"$0.real\" \"$@\"\n")
        os.chmod(binary, 0o755)
        (self.venv / "installed").write_text(f"{version}\n")
        if bad_import:
            metadata = binary.stat()
            self.env["QWENPAW_TEST_BAD_PYTHON_INODE"] = str(metadata.st_ino)
        else:
            self.env.pop("QWENPAW_TEST_BAD_PYTHON_INODE", None)
        if fail_pip:
            self.env["QWENPAW_TEST_FAIL_PIP"] = "1"
            (self.control / "fail-pip").write_text("yes\n")
        else:
            self.env.pop("QWENPAW_TEST_FAIL_PIP", None)

    def callback(self, name, **overrides):
        path, _ = CALLBACKS[name]
        env = self.env.copy()
        env.update({key: str(value) for key, value in overrides.items()})
        output_path = self.root / f"callback-{name}-{time.monotonic_ns()}.log"
        with output_path.open("wb") as output:
            process = subprocess.Popen([str(path)], env=env, stdout=output,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            identity = None
            identity_deadline = time.monotonic() + 2
            while process.poll() is None and identity is None and time.monotonic() < identity_deadline:
                identity = self.callback_identity(process.pid, path)
                if identity is None:
                    time.sleep(0.025)
            if process.poll() is None and identity is None:
                raise AssertionError(f"cannot verify {name} callback process identity; refusing unsafe cleanup")

            def signal_callback(signum):
                current = self.callback_identity(process.pid, path)
                if current is None and identity is not None and alive(identity):
                    current = identity
                require(current is not None,
                        f"cannot verify {name} callback identity; refusing to send signal {signum}")
                pidfd_signal(current, signum, self)

            try:
                return_code = process.wait(timeout=90)
            except subprocess.TimeoutExpired:
                signal_callback(signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    signal_callback(signal.SIGKILL)
                    process.wait(timeout=3)
                raise AssertionError(f"{name} callback timed out; output: {output_path.read_text(errors='replace')}")
        output = output_path.read_text(errors="replace")
        self.track_owned_processes()
        return return_code, output

    def callback_identity(self, pid, callback):
        info = read_process(pid)
        if info is None or info["state"] in {"Z", "X"}:
            return None
        callback_argv = os.fsencode(str(callback))
        binary = self.binary.stat()
        is_callback_shell = info["exe"].name in {"bash", "sh", "dash"} and any(
            callback_argv in arg for arg in info["argv"])
        is_supervisor = info["exe_id"] == (binary.st_dev, binary.st_ino)
        if not is_callback_shell and not is_supervisor:
            return None
        if not env_contains(info, "TRIM_PKGVAR", self.var):
            return None
        return info

    def command(self, args, timeout=45):
        result = subprocess.run(args, env=self.env, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, text=True, timeout=timeout)
        return result.returncode, result.stdout

    def manager_identity(self):
        def current():
            pid = int((self.var / "supervisor.pid").read_text().strip())
            starttime = int((self.var / "supervisor.pid.starttime").read_text().strip())
            info = read_process(pid)
            if info is None or info["state"] in {"Z", "X"}:
                return None
            expected = {"pid": pid, "starttime": starttime,
                        "exe_id": (self.binary.stat().st_dev, self.binary.stat().st_ino)}
            if not identity_matches(info, expected):
                return None
            for key, value in (("TRIM_PKGVAR", self.var), ("TRIM_PKGHOME", self.home),
                               ("TRIM_APPDEST", self.app)):
                if not env_contains(info, key, value):
                    return None
            return info

        info = wait_for(current, 5, "registered supervisor process identity")
        if not any(identity_matches(item, info) for item in self.manager_identities):
            self.manager_identities.append(info)
        return info

    def start_manager(self, expect_kernel=False):
        code, output = self.command(["bash", str(ROOT / "cmd/main"), "start"])
        require(code == 0, f"native manager failed to start (rc={code}): {output}")
        self.manager_identity()
        require((self.var / "supervisor-control.sock").is_socket(),
                "native manager did not create the private control UDS")
        if expect_kernel:
            self.wait_kernel()

    def wait_kernel(self):
        pid_path = self.venv / ".qwenpaw-test-runtime.pid"

        def current_runtime():
            fields = pid_path.read_text().split()
            pid, ticks = map(int, fields)
            info = read_process(pid)
            if info is None or info["state"] in {"Z", "X"} or info["starttime"] != ticks:
                return None
            executable = next((candidate for candidate in (
                self.venv / "bin/python3.real", self.venv / "bin/python3"
            ) if candidate.exists()), None)
            if executable is None:
                return None
            metadata = executable.stat()
            expected = {"pid": pid, "starttime": ticks,
                        "exe_id": (metadata.st_dev, metadata.st_ino)}
            if not identity_matches(info, expected):
                return None
            for key, value in (("TRIM_PKGVAR", self.var), ("TRIM_PKGHOME", self.home),
                               ("TRIM_APPDEST", self.app)):
                if not env_contains(info, key, value):
                    return None
            if not any(identity_matches(item, expected) for item in self.runtime_identities):
                self.runtime_identities.append(info)
            with urllib.request.urlopen(
                    f"http://127.0.0.1:{self.service_port}/api/version", timeout=1) as response:
                payload = json.loads(response.read())
            require(payload.get("version") == (self.venv / "installed").read_text().strip(),
                    f"test runtime health returned an unexpected version: {payload}")
            return info

        wait_for(current_runtime, 15, "healthy test runtime process identity")

    def control_status(self):
        path = self.var / "supervisor-control.sock"
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(2)
            client.connect(str(path))
            client.sendall(b'{"operation":"install-status"}\n')
            response = bytearray()
            while not response.endswith(b"\n"):
                chunk = client.recv(4096)
                if not chunk:
                    break
                response.extend(chunk)
        require(response.endswith(b"\n"), "UDS install-status response was truncated")
        payload = json.loads(response)
        require(payload.get("status") == 200 and payload.get("success") is True,
                f"UDS install-status failed: {payload}")
        return payload

    def wait_task(self, previous_seq=0):
        def terminal():
            status = self.control_status()
            if status["seq"] <= previous_seq:
                return None
            if status["phase"] in TERMINAL_PHASES and not status["upgrading"]:
                return status
            return None
        return wait_for(terminal, 45, "native install task terminal state")

    def track_owned_processes(self):
        runtime_exe_ids = set()
        for candidate in (self.venv / "bin/python3.real", self.venv / "bin/python3"):
            try:
                info = candidate.stat()
                runtime_exe_ids.add((info.st_dev, info.st_ino))
            except FileNotFoundError:
                pass
        try:
            pids = [int(path.name) for path in Path("/proc").iterdir() if path.name.isdigit()]
        except OSError:
            return
        for pid in pids:
            info = read_process(pid)
            if info is None or info["state"] in {"Z", "X"}:
                continue
            if not env_contains(info, "TRIM_PKGVAR", self.var):
                continue
            if (info["exe_id"] == (self.binary.stat().st_dev, self.binary.stat().st_ino)
                    and env_contains(info, "TRIM_APPDEST", self.app)):
                if not any(identity_matches(item, info) for item in self.manager_identities):
                    self.manager_identities.append(info)
            elif info["exe_id"] in runtime_exe_ids and env_contains(info, "TRIM_PKGHOME", self.home):
                if not any(identity_matches(item, info) for item in self.runtime_identities):
                    self.runtime_identities.append(info)

    def stop_manager(self):
        if not (self.var / "supervisor.pid").exists():
            return
        self.track_owned_processes()
        code, output = self.command(["bash", str(ROOT / "cmd/main"), "stop"], timeout=35)
        require(code == 0, f"native manager stop failed (rc={code}): {output}")
        wait_for(lambda: not any(alive(identity) for identity in self.manager_identities), 8,
                 "owned supervisor exit")
        wait_for(lambda: not any(alive(identity) for identity in self.runtime_identities), 8,
                 "owned runtime exit")

    def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            self.track_owned_processes()
            if (self.var / "supervisor.pid").exists():
                result = subprocess.run(["bash", str(ROOT / "cmd/main"), "stop"], env=self.env,
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                        text=True, timeout=35)
                if result.returncode != 0:
                    print(f"cleanup: native stop rc={result.returncode}: {result.stdout}")
        except (OSError, subprocess.SubprocessError) as error:
            print(f"cleanup: native stop failed: {error}")
        self.track_owned_processes()
        for identity in (*self.runtime_identities, *self.manager_identities):
            if alive(identity):
                pidfd_signal(identity, signal.SIGTERM, self)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and any(
                alive(identity) for identity in (*self.runtime_identities, *self.manager_identities)):
            time.sleep(0.025)
        for identity in (*self.runtime_identities, *self.manager_identities):
            if alive(identity):
                pidfd_signal(identity, signal.SIGKILL, self)
        for identity in (*self.runtime_identities, *self.manager_identities):
            require(not alive(identity), f"owned process survived cleanup: pid={identity['pid']}")

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        self.close()


def validate_inputs():
    for label, path in (("release supervisor", SUPERVISOR_SOURCE),
                        ("release qwenpaw-test-runtime", RUNTIME_SOURCE)):
        require(path.is_file() and os.access(path, os.X_OK),
                f"missing executable {label}: {path}; no fallback binary is allowed")
    hashes = {"supervisor": sha256(SUPERVISOR_SOURCE), "runtime": sha256(RUNTIME_SOURCE)}
    print(f"release supervisor source: {SUPERVISOR_SOURCE}\nSHA256: {hashes['supervisor']}")
    print(f"release test runtime source: {RUNTIME_SOURCE}\nSHA256: {hashes['runtime']}")
    for name, (path, action) in CALLBACKS.items():
        require(path.is_file() and os.access(path, os.X_OK),
                f"missing executable {name} callback: {path}")
        source = path.read_text(encoding="utf-8")
        executable = "\n".join(line.partition("#")[0] for line in source.splitlines())
        require(re.search(rf'exec\s+"\$candidate"\s+platform\s+{re.escape(action)}(?:\s|$)', executable),
                f"{path.relative_to(ROOT)} is not a thin native platform {action} exec callback")
        require("lib.sh" not in executable and "supervisor_ctl.sh" not in executable,
                f"{path.relative_to(ROOT)} still sources retired Shell libraries")
        forbidden = re.compile(r"\b(?:curl|python3?|pip3?|rm|chown|chmod|kill|sed|awk|grep|find|mkdir|mv|sync|sleep)\b")
        require(not forbidden.search(executable),
                f"{path.relative_to(ROOT)} contains lifecycle business or external tooling")
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
            "safe process cleanup requires pidfd_open and pidfd_send_signal")
    return hashes


def run_case(name, function, failures):
    try:
        function()
        print(f"[  OK ] {name}")
    except Exception as error:
        failures.append((name, error))
        print(f"[FAIL ] {name}: {error}")


def case_empty_shell(parent, hashes):
    with Fixture(parent, "empty-shell", hashes) as fixture:
        code, output = fixture.callback("install")
        require(code == 0, f"install callback failed without a pin (rc={code}): {output}")
        require(not fixture.venv.exists(), "empty shell created a venv")
        require(not fixture.pin.exists(), "empty shell created a version pin")
        require(not (fixture.control / "pip-started").exists(), "empty shell invoked pip")
        require(not (fixture.control / "init-count").exists(), "empty shell invoked init")


def case_healthy_upgrade(parent, hashes):
    with Fixture(parent, "healthy-upgrade", hashes) as fixture:
        fixture.create_runtime("2.4.1")
        fixture.pin.write_text("2.4.1\n")
        fixture.user_data.joinpath("config.json").write_text("existing config\n")
        original_inode = (fixture.venv / "bin/python3").stat().st_ino
        code, output = fixture.command(["bash", str(ROOT / "cmd/main"), "start"])
        require(code == 0, f"healthy supervisor failed to start: {output}")
        fixture.manager_identity()
        fixture.wait_kernel()
        prior = fixture.control_status()
        code, output = fixture.callback("upgrade")
        require(code == 0, f"healthy upgrade callback failed (rc={code}): {output}")
        after = fixture.control_status()
        require((fixture.venv / "installed").read_text() == "2.4.1\n",
                "healthy upgrade changed the QwenPaw version")
        require(fixture.pin.read_text() == "2.4.1\n", "healthy upgrade changed the version pin")
        require((fixture.venv / "bin/python3").stat().st_ino == original_inode,
                "healthy upgrade replaced the active runtime")
        require(after["seq"] == prior["seq"], "healthy upgrade started an install task")
        require(not (fixture.control / "pip-started").exists(), "healthy upgrade invoked pip")
        require(not (fixture.control / "init-count").exists(), "healthy upgrade invoked init")


def case_exact_repair(parent, hashes):
    with Fixture(parent, "exact-repair", hashes) as fixture:
        fixture.create_runtime("1.7.0", bad_import=True)
        fixture.pin.write_text("2.8.4\n")
        before_inode = (fixture.venv / "bin/python3").stat().st_ino
        fixture.start_manager(expect_kernel=False)
        prior = fixture.control_status()
        code, output = fixture.callback("upgrade")
        require(code == 0, f"pinned repair callback failed (rc={code}): {output}")
        task = fixture.wait_task(prior["seq"])
        require(task["phase"] == "done", f"pinned repair task did not succeed: {task}")
        require((fixture.venv / "installed").read_text() == "2.8.4\n",
                "repair did not install the exact pin")
        require(fixture.pin.read_text() == "2.8.4\n", "repair changed the exact version pin")
        require((fixture.venv / "bin/python3").stat().st_ino != before_inode,
                "repair did not activate a fresh staged venv")
        require(not list(fixture.home.glob(".venv.staging.*")), "repair left a staging directory")
        require(not list(fixture.home.glob(".venv.rollback.*")), "repair left a rollback directory")
        require((fixture.control / "pip-started").exists(), "repair task did not invoke pip")
        require((fixture.control / "init-count").read_text().strip() == "1",
                "fresh runtime did not initialize exactly once")
        fixture.wait_kernel()


def case_missing_pin(parent, hashes):
    with Fixture(parent, "missing-pin", hashes) as fixture:
        fixture.create_runtime("1.7.0", bad_import=True)
        fixture.start_manager(expect_kernel=False)
        before = (fixture.venv / "installed").read_bytes()
        code, output = fixture.callback("upgrade")
        require(code != 0, "upgrade without a pin unexpectedly succeeded")
        require(not (fixture.control / "pip-started").exists(),
                f"missing-pin upgrade invoked pip: {output}")
        require((fixture.venv / "installed").read_bytes() == before,
                "missing-pin upgrade modified the active runtime")
        require(not fixture.pin.exists(), "missing-pin upgrade synthesized a pin")
        require(fixture.control_status()["seq"] == 0, "missing-pin upgrade submitted a task")


def case_existing_data_skips_init(parent, hashes, marker):
    with Fixture(parent, f"existing-{marker}", hashes) as fixture:
        if marker == "config":
            fixture.user_data.joinpath("config.json").write_text("existing config\n")
        else:
            fixture.user_data.joinpath("workspaces").mkdir()
        fixture.pin.write_text("2.6.0\n")
        fixture.start_manager(expect_kernel=False)
        prior = fixture.control_status()
        code, output = fixture.callback("upgrade")
        require(code == 0, f"upgrade with existing {marker} failed (rc={code}): {output}")
        task = fixture.wait_task(prior["seq"])
        require(task["phase"] == "done", f"upgrade task failed for existing {marker}: {task}")
        require(not (fixture.control / "init-count").exists(),
                f"existing {marker} unexpectedly ran init")
        require(fixture.venv.joinpath("installed").read_text() == "2.6.0\n",
                "existing-data recovery did not honor the exact pin")


def case_uninstall(parent, hashes, choice):
    with Fixture(parent, f"uninstall-{choice}", hashes) as fixture:
        fixture.create_runtime("2.2.0")
        fixture.pin.write_text("2.2.0\n")
        (fixture.home / "workspace-sentinel").write_text("home data\n")
        (fixture.etc / "settings").write_text("etc data\n")
        (fixture.var / "sentinel").write_text("var data\n")
        fixture.user_data.joinpath("memory.db").write_text("memory\n")
        fixture.user_secret.joinpath("auth.json").write_text("auth\n")
        code, output = fixture.callback("uninstall", wizard_keep_data=choice)
        require(code == 0, f"uninstall {choice} failed (rc={code}): {output}")
        require(fixture.sentinel.read_text() == "must survive uninstall remove\n",
                f"uninstall {choice} touched the outer sentinel")
        require(fixture.share.is_dir(), f"uninstall {choice} removed the fnOS shared root")
        if choice == "remove":
            for path in (fixture.home, fixture.etc, fixture.var, fixture.user_data, fixture.user_secret):
                require(not path.exists(), f"remove left deletion target: {path}")
        else:
            require(not fixture.venv.exists(), f"uninstall {choice} did not remove only the venv")
            require((fixture.home / "workspace-sentinel").exists(),
                    f"uninstall {choice} removed home data")
            require((fixture.etc / "settings").exists(), f"uninstall {choice} removed etc data")
            require((fixture.var / "sentinel").exists(), f"uninstall {choice} removed var data")
            require(fixture.user_data.joinpath("memory.db").exists(),
                    f"uninstall {choice} removed user data")
            require(fixture.user_secret.joinpath("auth.json").exists(),
                    f"uninstall {choice} removed authentication data")


def case_uds_failure(parent, hashes):
    with Fixture(parent, "uds-failure", hashes) as fixture:
        fixture.create_runtime("2.1.0")
        fixture.pin.write_text("2.1.0\n")
        before = (fixture.venv / "installed").read_bytes()
        (fixture.control / "remove-control-socket").write_text("yes\n")
        code, output = fixture.callback("upgrade")
        require((fixture.control / "control-socket-removed").exists(),
                "test runtime did not remove the temporary supervisor UDS")
        require(code != 0, f"upgrade callback reported success with unavailable UDS: {output}")
        require((fixture.venv / "installed").read_bytes() == before,
                "UDS failure fell back to an offline runtime mutation")
        require(not (fixture.control / "pip-started").exists(),
                f"UDS failure fell back to pip: {output}")


def case_task_failure(parent, hashes):
    with Fixture(parent, "task-failure", hashes) as fixture:
        fixture.create_runtime("1.4.0", bad_import=True, fail_pip=True)
        fixture.pin.write_text("2.5.0\n")
        active_inode = (fixture.venv / "bin/python3").stat().st_ino
        before_pin = fixture.pin.read_bytes()
        fixture.start_manager(expect_kernel=False)
        prior = fixture.control_status()
        code, output = fixture.callback("upgrade")
        require(code != 0, f"failed install task was reported successful: {output}")
        task = fixture.wait_task(prior["seq"])
        require(task["phase"] == "failed", f"native install task did not report failure: {task}")
        require((fixture.control / "pip-attempts").exists(), "failure injection did not reach pip")
        require((fixture.venv / "installed").read_text() == "1.4.0\n",
                "failed task did not restore the prior runtime")
        require(fixture.pin.read_bytes() == before_pin, "failed task changed the prior pin")
        require((fixture.venv / "bin/python3").stat().st_ino == active_inode,
                "failed task did not restore the original active venv")
        require(not list(fixture.home.glob(".venv.staging.*")), "failed task left a staging directory")


def main():
    try:
        hashes = validate_inputs()
    except Exception as error:
        print(f"[FAIL ] platform callback input validation: {error}")
        return 1
    with tempfile.TemporaryDirectory(prefix="qwenpaw-platform-callbacks-") as temporary:
        parent = Path(temporary)
        manifest = parent / "binary-sources.sha256"
        manifest.write_text(f"{hashes['supervisor']}  {SUPERVISOR_SOURCE}\n"
                            f"{hashes['runtime']}  {RUNTIME_SOURCE}\n")
        failures = []
        run_case("empty shell install does not create a venv or run pip",
                 lambda: case_empty_shell(parent, hashes), failures)
        run_case("healthy upgrade preserves version, init count and pip count",
                 lambda: case_healthy_upgrade(parent, hashes), failures)
        run_case("bad import repair honors exact pin and commits fresh staging",
                 lambda: case_exact_repair(parent, hashes), failures)
        run_case("upgrade without a pin fails without invoking pip",
                 lambda: case_missing_pin(parent, hashes), failures)
        for marker in ("config", "workspaces"):
            run_case(f"existing {marker} prevents reinitialization",
                     lambda marker=marker: case_existing_data_skips_init(parent, hashes, marker), failures)
        for choice in ("keep", "remove", "unknown"):
            run_case(f"uninstall {choice} preserves only its permitted deletion scope",
                     lambda choice=choice: case_uninstall(parent, hashes, choice), failures)
        run_case("unavailable private UDS fails without offline fallback",
                 lambda: case_uds_failure(parent, hashes), failures)
        run_case("failed native install task returns nonzero and rolls back",
                 lambda: case_task_failure(parent, hashes), failures)
        if failures:
            print(f"test-platform-callbacks: {len(failures)} scenario(s) failed")
            return 1
    print("test-platform-callbacks: ALL CHECKS PASSED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
