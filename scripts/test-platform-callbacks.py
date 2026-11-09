#!/usr/bin/env python3
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
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
    def __init__(self, parent, name, source_hashes, fail_pip=False, fnos_layout=False):
        self.root = parent / name
        if fnos_layout:
            volume = self.root / "vol1"
            self.home = volume / "@apphome/qwenpaw"
            self.etc = volume / "@appconf/qwenpaw"
            self.var = volume / "@appdata/qwenpaw"
        else:
            self.home = self.root / "home"
            self.var = self.root / "var"
            self.etc = self.root / "etc"
        self.share = self.root / "share"
        self.user_data = self.share / ".qwenpaw"
        self.user_secret = self.share / ".qwenpaw.secret"
        self.app = self.root / "target"
        self.binary = self.app / "bin/qwenpaw-supervisor"
        self.runtime_source = self.root / "qwenpaw-test-runtime"
        for path in (self.home, self.var, self.etc, self.user_data, self.user_secret,
                     self.app / "bin", self.app / "ui"):
            path.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SUPERVISOR_SOURCE, self.binary)
        shutil.copy2(RUNTIME_SOURCE, self.runtime_source)
        os.chmod(self.binary, 0o755)
        os.chmod(self.runtime_source, 0o755)
        require(sha256(self.binary) == source_hashes["supervisor"],
                f"copied supervisor SHA256 mismatch in {name}")
        require(sha256(self.runtime_source) == source_hashes["runtime"],
                f"copied test runtime SHA256 mismatch in {name}")
        self.candidate_cmd.mkdir(parents=True, exist_ok=True)
        self.stage_candidate_helper()
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
            "QWENPAW_WEB_PROXY_PORT": "off",
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

    @property
    def helper(self):
        return self.var / "qwenpaw-supervisor-helper"

    @property
    def installed_cmd(self):
        return self.root / "cmd"

    @property
    def installed_helper(self):
        return self.installed_cmd / "qwenpaw-supervisor"

    @property
    def candidate_cmd(self):
        return self.root / "candidate/cmd"

    @property
    def candidate_helper(self):
        return self.candidate_cmd / "qwenpaw-supervisor"

    def stage_installed_helper(self):
        self.installed_helper.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SUPERVISOR_SOURCE, self.installed_helper)
        os.chmod(self.installed_helper, 0o755)
        require(sha256(self.installed_helper) == sha256(SUPERVISOR_SOURCE),
                "installed helper fixture differs from the release supervisor")
        return self.installed_helper

    def stage_candidate_helper(self):
        self.candidate_helper.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SUPERVISOR_SOURCE, self.candidate_helper)
        os.chmod(self.candidate_helper, 0o755)
        require(sha256(self.candidate_helper) == sha256(SUPERVISOR_SOURCE),
                "candidate helper fixture differs from the release supervisor")
        return self.candidate_helper

    def entry_path(self, name, directory=None):
        source = CALLBACKS[name][0] if name in CALLBACKS else ROOT / "cmd" / name
        path = Path(directory or self.installed_cmd) / f".isolated-{name}"
        path.parent.mkdir(parents=True, exist_ok=True)
        content = source.read_text(encoding="utf-8")
        for index, candidate in enumerate((
            "/var/apps/qwenpaw/target/bin/qwenpaw-supervisor",
            "/var/apps/qwenpaw/app/bin/qwenpaw-supervisor",
        )):
            content = content.replace(
                candidate,
                str(self.root / "unavailable-targets" / str(index) / "qwenpaw-supervisor"),
            )
        path.write_text(content, encoding="utf-8")
        os.chmod(path, 0o755)
        return path

    def manager_executable_ids(self):
        ids = set()
        for candidate in (self.binary, self.helper, self.installed_helper, self.candidate_helper):
            try:
                metadata = candidate.stat()
            except FileNotFoundError:
                continue
            if stat.S_ISREG(metadata.st_mode):
                ids.add((metadata.st_dev, metadata.st_ino))
        return ids

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

    def callback(self, name, output_parent=None, directory=None, **overrides):
        if name in {"upgrade", "uninstall"} and directory is None:
            require(self.installed_helper.is_file(),
                    f"{name} callback requires an explicitly staged installed sibling helper")
        path = self.entry_path(name, directory=directory)
        env = self.env.copy()
        env.update({key: str(value) for key, value in overrides.items()})
        output_path = (output_parent or self.root) / f"callback-{name}-{time.monotonic_ns()}.log"
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

    def upgrade_init(self, directory=None):
        path = self.entry_path("upgrade_init", directory=directory)
        result = subprocess.run([str(path)], env=self.env, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, text=True, timeout=30)
        return result.returncode, result.stdout

    def callback_identity(self, pid, callback):
        info = read_process(pid)
        if info is None or info["state"] in {"Z", "X"}:
            return None
        callback_argv = os.fsencode(str(callback))
        is_callback_shell = info["exe"].name in {"bash", "sh", "dash"} and any(
            callback_argv in arg for arg in info["argv"])
        is_supervisor = info["exe_id"] in self.manager_executable_ids()
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
            if info["exe_id"] not in self.manager_executable_ids():
                return None
            expected = {"pid": pid, "starttime": starttime, "exe_id": info["exe_id"]}
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
            if (info["exe_id"] in self.manager_executable_ids()
                    and env_contains(info, "TRIM_APPDEST", self.app)):
                if not any(identity_matches(item, info) for item in self.manager_identities):
                    self.manager_identities.append(info)
            elif info["exe_id"] in runtime_exe_ids and env_contains(info, "TRIM_PKGHOME", self.home):
                if not any(identity_matches(item, info) for item in self.runtime_identities):
                    self.runtime_identities.append(info)

    def manager_stop_command(self):
        if self.binary.is_file() and os.access(self.binary, os.X_OK):
            return ["bash", str(ROOT / "cmd/main"), "stop"]
        if self.installed_helper.is_file():
            return [str(self.installed_helper), "service", "stop"]
        if self.helper.is_file():
            return [str(self.helper), "service", "stop"]
        return None

    def stop_manager(self):
        if not (self.var / "supervisor.pid").exists():
            return
        self.track_owned_processes()
        command = self.manager_stop_command()
        require(command is not None, "cannot stop manager without target or staged helper")
        code, output = self.command(command, timeout=35)
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
                command = self.manager_stop_command()
                if command is None:
                    print("cleanup: no target or staged helper available for native stop")
                else:
                    result = subprocess.run(command, env=self.env, stdout=subprocess.PIPE,
                                            stderr=subprocess.STDOUT, text=True, timeout=35)
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
    upgrade_init = ROOT / "cmd/upgrade_init"
    upgrade_init_source = upgrade_init.read_text(encoding="utf-8")
    upgrade_init_active = [
        line.strip() for line in upgrade_init_source.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]
    require(upgrade_init_active[:3] == [
        'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then',
        'exec "${0%/*}/qwenpaw-supervisor" platform upgrade-init',
        "fi",
    ], "cmd/upgrade_init must prefer the staged sibling supervisor")
    for name, (path, action) in CALLBACKS.items():
        require(path.is_file() and os.access(path, os.X_OK),
                f"missing executable {name} callback: {path}")
        source = path.read_text(encoding="utf-8")
        executable = "\n".join(line.partition("#")[0] for line in source.splitlines())
        active = [line.strip() for line in executable.splitlines() if line.strip()]
        if action in {"upgrade", "uninstall"}:
            expected = [
                'if [ -n "${TRIM_PKGVAR:-}" ] && [ -x "${TRIM_PKGVAR}/qwenpaw-supervisor-helper" ]; then',
                f'exec "${{TRIM_PKGVAR}}/qwenpaw-supervisor-helper" platform {action}',
                "fi",
                'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then',
                f'exec "${{0%/*}}/qwenpaw-supervisor" platform {action}',
                "fi",
                "exit 127",
            ]
            require(active == expected,
                    f"{path.relative_to(ROOT)} must use a persistent helper first, a sibling fallback, and fail closed")
        else:
            require(re.search(rf'exec\s+"\$candidate"\s+platform\s+{re.escape(action)}(?:\s|$)', executable),
                    f"{path.relative_to(ROOT)} is not a thin native platform {action} exec callback")
            require("qwenpaw-supervisor-helper" not in executable,
                    "install callback must not use a persistent helper fallback")
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


def stage_callback_dispatch_stub(path, marker):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        f"#!/bin/sh\nprintf '%s\\n' '{marker}'\nprintf 'arg=%s\\n' \"$@\"\n",
        encoding="utf-8",
    )
    os.chmod(path, 0o755)


def case_lifecycle_callback_prefers_var_helper(parent, hashes, action):
    with Fixture(parent, f"callback-var-priority-{action}", hashes) as fixture:
        wrapper_dir = fixture.root / "callback-wrapper"
        stage_callback_dispatch_stub(wrapper_dir / "qwenpaw-supervisor", f"selected-sibling-{action}")
        stage_callback_dispatch_stub(fixture.helper, f"selected-var-{action}")

        code, output = fixture.callback(action, directory=wrapper_dir)
        require(code == 0, f"{action} callback stub returned {code}: {output}")
        require(f"selected-var-{action}" in output,
                f"{action} callback did not prefer the persistent var helper: {output}")
        require(f"selected-sibling-{action}" not in output,
                f"{action} callback selected the stale sibling despite a var helper: {output}")
        require(f"arg=platform\narg={action}" in output,
                f"{action} callback did not preserve the fixed Rust action arguments: {output}")


def case_empty_shell(parent, hashes):
    with Fixture(parent, "empty-shell", hashes) as fixture:
        fixture.env.pop("QWENPAW_GATEWAY_SOCKET", None)
        code, output = fixture.callback("install")
        require(code == 0, f"install callback failed without a pin (rc={code}): {output}")
        manager = fixture.manager_identity()
        require(env_contains(manager, "TRIM_APPDEST", fixture.app),
                "install callback supervisor lost TRIM_APPDEST")
        require(not any(item.startswith(b"QWENPAW_GATEWAY_SOCKET=") for item in manager["environ"]),
                "install callback unexpectedly injected QWENPAW_GATEWAY_SOCKET")
        gateway = fixture.app / "app.sock"
        private = fixture.var / "supervisor-control.sock"
        require(gateway.is_socket(), "install callback did not create target/app.sock")
        require(private.is_socket(), "install callback did not create the private control UDS")
        require(gateway.stat().st_ino != private.stat().st_ino,
                "gateway and private control sockets must remain distinct")
        require(not fixture.venv.exists(), "empty shell created a venv")
        require(not fixture.pin.exists(), "empty shell created a version pin")
        require(not (fixture.control / "pip-started").exists(), "empty shell invoked pip")
        require(not (fixture.control / "init-count").exists(), "empty shell invoked init")


def case_empty_shell_upgrade(parent, hashes):
    with Fixture(parent, "empty-shell-upgrade", hashes) as fixture:
        config = fixture.user_data / "config.json"
        auth = fixture.user_secret / "auth.json"
        config.write_text("preserve existing shared data\\n")
        auth.write_text("preserve existing auth\\n")
        fixture.stage_installed_helper()
        code, output = fixture.callback("upgrade")
        require(code == 0,
                f"empty-shell upgrade callback failed without a pin (rc={code}): {output}")
        require("外壳升级未检测到内核且缺少版本钉；仅升级外壳，不创建 venv 或运行 pip" in output,
                "empty-shell upgrade did not report the shell-only upgrade plan")
        require("首次安装未提供合法版本钉" not in output,
                "empty-shell upgrade used the first-install diagnostic")
        fixture.manager_identity()
        require(config.read_text() == "preserve existing shared data\\n",
                "empty-shell upgrade changed shared application data")
        require(auth.read_text() == "preserve existing auth\\n",
                "empty-shell upgrade changed authentication data")
        require(not fixture.venv.exists(), "empty-shell upgrade created a venv")
        require(not fixture.pin.exists(), "empty-shell upgrade created a version pin")
        require(not (fixture.control / "pip-started").exists(),
                "empty-shell upgrade invoked pip")
        require(not (fixture.control / "init-count").exists(),
                "empty-shell upgrade invoked init")
        require(not (fixture.var / "pip-mirror").exists(),
                "empty-shell upgrade seeded a pip mirror")
        require(fixture.control_status()["seq"] == 0,
                "empty-shell upgrade submitted a kernel install task")


def case_lifecycle_lock_with_read_only_parent(parent, hashes):
    require(os.geteuid() != 0, "lifecycle lock permission case requires a non-root test user")
    with Fixture(parent, "lifecycle-lock-read-only-parent", hashes) as fixture:
        original_mode = fixture.root.stat().st_mode & 0o777
        fixture.entry_path("install")
        try:
            os.chmod(fixture.root, 0o555)
            require(os.access(fixture.var, os.W_OK), "application var directory is not writable")
            require(not os.access(fixture.root, os.W_OK),
                    "read-only fixture parent is unexpectedly writable")
            code, output = fixture.callback("install", output_parent=fixture.var)
            require(code == 0,
                    f"install callback failed with a writable var and read-only parent "
                    f"(rc={code}): {output}")
            require((fixture.var / ".qwenpaw-lifecycle.lock").is_file(),
                    "lifecycle lock was not created inside the application var directory")
            require(not (fixture.root / ".qwenpaw-lifecycle.lock").exists(),
                    "lifecycle lock was created in the read-only parent")
        finally:
            os.chmod(fixture.root, original_mode)


def case_removed_target_without_helper_is_127(parent, hashes):
    with Fixture(parent, "removed-target-without-helper", hashes) as fixture:
        fixture.binary.unlink()
        require(not fixture.installed_helper.exists(),
                "installed callback fixture unexpectedly contains a sibling helper")
        require(not fixture.helper.exists(), "baseline fixture unexpectedly contains a var helper")
        for name in ("upgrade", "uninstall"):
            code, output = fixture.command([str(fixture.entry_path(name))])
            require(code == 127,
                    f"{name} callback did not fail with 127 when both packaged binaries were removed: "
                    f"rc={code}, output={output}")


def stage_candidate_upgrade_helper(fixture):
    old_target_marker = "OLD_TARGET_UPGRADE_INIT_MUST_NOT_RUN"
    fixture.binary.write_text(
        f"#!/bin/sh\nprintf '%s\\n' '{old_target_marker}'\nexit 0\n",
        encoding="utf-8",
    )
    os.chmod(fixture.binary, 0o755)
    (fixture.var / "pip-mirror").write_text("https://pypi.org/simple\n")
    candidate_cmd = fixture.candidate_cmd
    code, output = fixture.upgrade_init(directory=candidate_cmd)
    return code, output, old_target_marker


def case_upgrade_init_prefers_candidate_supervisor(parent, hashes):
    with Fixture(parent, "candidate-upgrade-init", hashes) as fixture:
        code, output, old_target_marker = stage_candidate_upgrade_helper(fixture)
        require(code == 0, f"candidate upgrade_init failed (rc={code}): {output}")
        require(old_target_marker not in output,
                f"upgrade_init executed the old target supervisor instead of the staged candidate: {output}")
        require("wizard_pypi_mirror=official" in output,
                f"candidate upgrade_init did not preserve mirror output: {output}")
        require(fixture.helper.is_file(), "candidate upgrade_init did not stage a regular var helper")
        helper_metadata = fixture.helper.stat()
        require(stat.S_ISREG(helper_metadata.st_mode), "staged helper is not a regular file")
        require(stat.S_IMODE(helper_metadata.st_mode) == 0o750,
                f"staged helper mode is not 0750: {stat.S_IMODE(helper_metadata.st_mode):04o}")
        require(sha256(fixture.helper) == hashes["supervisor"],
                "staged helper SHA256 differs from the release supervisor source")
        require(not list(fixture.var.glob(".qwenpaw-supervisor-helper.*.tmp")),
                "helper staging left a temporary file")


def case_legacy_upgrade_init_does_not_stage_var_helper(parent, hashes):
    with Fixture(parent, "legacy-upgrade-init", hashes) as fixture:
        marker = "LEGACY_TARGET_UPGRADE_INIT"
        fixture.binary.write_text(
            f"#!/bin/sh\nprintf '%s\\n' '{marker}'\nexit 0\n",
            encoding="utf-8",
        )
        os.chmod(fixture.binary, 0o755)
        installed_cmd = fixture.root / "installed/cmd"
        code, output = fixture.upgrade_init(directory=installed_cmd)
        require(code == 0 and marker in output,
                f"upgrade_init did not preserve the legacy target fallback (rc={code}): {output}")
        require(not fixture.helper.exists(),
                "legacy target upgrade_init unexpectedly staged a var helper")


def case_installed_callback_falls_back_to_var_helper(parent, hashes):
    with Fixture(parent, "installed-callback-var-helper", hashes) as fixture:
        fixture.create_runtime("2.4.1")
        fixture.pin.write_text("2.4.1\n")
        code, output = fixture.callback("install")
        require(code == 0, f"install callback failed while creating the var helper (rc={code}): {output}")
        installed_cmd = fixture.root / "installed/cmd"
        installed_cmd.mkdir(parents=True)
        require(not (installed_cmd / "qwenpaw-supervisor").exists(),
                "installed callback fixture unexpectedly has its sibling supervisor")
        code, output = fixture.callback("upgrade", directory=installed_cmd)
        require(code == 0,
                f"installed callback did not fall back to the var helper (rc={code}): {output}")
        require("开始升级回调" in output,
                f"var-helper callback did not enter the Rust upgrade workflow: {output}")


def case_persistent_helper_after_target_removal(parent, hashes):
    with Fixture(parent, "persistent-helper", hashes) as fixture:
        fixture.create_runtime("2.4.1")
        fixture.pin.write_text("2.4.1\n")
        fixture.user_data.joinpath("config.json").write_text("shared-data-sentinel\n")
        fixture.user_secret.joinpath("auth.json").write_text("auth-sentinel\n")
        (fixture.home / "workspace-sentinel").write_text("home-sentinel\n")
        (fixture.etc / "settings").write_text("etc-sentinel\n")
        (fixture.var / "var-sentinel").write_text("var-sentinel\n")

        code, output, old_target_marker = stage_candidate_upgrade_helper(fixture)
        require(code == 0 and old_target_marker not in output,
                f"candidate upgrade_init did not replace the old target path (rc={code}): {output}")
        shutil.copy2(SUPERVISOR_SOURCE, fixture.binary)
        os.chmod(fixture.binary, 0o755)

        installed_cmd = fixture.root / "installed/cmd"
        installed_cmd.mkdir(parents=True)
        require(not (installed_cmd / "qwenpaw-supervisor").exists(),
                "installed callback fixture unexpectedly has its sibling supervisor")
        fixture.start_manager(expect_kernel=True)
        prior = fixture.control_status()
        code, output = fixture.callback("upgrade", directory=installed_cmd)
        require(code == 0,
                f"installed callback without a sibling failed to use the var helper (rc={code}): {output}")
        require("开始升级回调" in output,
                f"var-helper callback did not enter the Rust upgrade workflow: {output}")
        require(fixture.control_status()["seq"] == prior["seq"],
                "healthy var-helper upgrade unexpectedly submitted a runtime install task")
        require(fixture.venv.joinpath("installed").read_text() == "2.4.1\n",
                "healthy var-helper upgrade changed the runtime")
        require(fixture.pin.read_text() == "2.4.1\n",
                "healthy var-helper upgrade changed the exact version pin")
        require(not (fixture.control / "pip-started").exists(),
                "healthy var-helper upgrade invoked pip")
        fixture.manager_identity()
        fixture.stop_manager()

        fixture.binary.unlink()
        require(not fixture.binary.exists(), "target supervisor binary was not removed")
        code, output = fixture.callback("upgrade", directory=installed_cmd)
        require(code == 1 and "开始升级回调" in output
                and "升级后启动监督者失败" in output,
                f"target-missing callback did not enter Rust and fail closed at manager startup "
                f"(rc={code}): {output}")
        require(not (fixture.var / "supervisor.pid").exists(),
                "upgrade started a long-lived manager from the persistent var helper")
        require(fixture.helper.is_file(), "failed target upgrade removed the persistent var helper")

        fixture.env.pop("wizard_keep_data", None)
        code, output = fixture.callback("uninstall", directory=installed_cmd)
        require(code == 0 and "卸载清理完成" in output,
                f"installed uninstall callback without sibling failed to use the var helper "
                f"or default keep mode (rc={code}): {output}")
        require(fixture.helper.is_file(), "default keep uninstall removed the persistent var helper")
        require(not fixture.venv.exists(), "default keep uninstall did not remove the venv")
        require(fixture.pin.read_text() == "2.4.1\n", "default keep uninstall removed the version pin")
        require((fixture.home / "workspace-sentinel").read_text() == "home-sentinel\n",
                "default keep uninstall removed other home data")
        require((fixture.etc / "settings").read_text() == "etc-sentinel\n",
                "default keep uninstall removed etc data")
        require((fixture.var / "var-sentinel").read_text() == "var-sentinel\n",
                "default keep uninstall removed other var data")
        require(fixture.user_data.joinpath("config.json").read_text() == "shared-data-sentinel\n",
                "default keep uninstall changed shared application data")
        require(fixture.user_secret.joinpath("auth.json").read_text() == "auth-sentinel\n",
                "default keep uninstall changed authentication data")
        require(fixture.sentinel.read_text() == "must survive uninstall remove\n",
                "installed callbacks changed the outer sentinel")


def case_helper_remove_cleans_var_after_target_removal(parent, hashes):
    with Fixture(parent, "persistent-helper-remove", hashes) as fixture:
        fixture.create_runtime("2.4.1")
        fixture.pin.write_text("2.4.1\n")
        code, output = fixture.callback("install")
        require(code == 0, f"install callback failed while staging remove helper (rc={code}): {output}")
        require(fixture.helper.is_file(), "remove scenario did not stage helper")
        fixture.stop_manager()
        fixture.binary.unlink()
        installed_cmd = fixture.root / "installed/cmd"
        installed_cmd.mkdir(parents=True)

        code, output = fixture.callback("uninstall", directory=installed_cmd,
                                        wizard_keep_data="remove")
        require(code == 0, f"remove uninstall fallback helper failed (rc={code}): {output}")
        require(not fixture.var.exists(), "remove uninstall did not clean helper with the existing var scope")
        require(fixture.share.is_dir(), "remove uninstall removed the shared root")
        require(fixture.sentinel.read_text() == "must survive uninstall remove\n",
                "remove uninstall touched the outer sentinel")


def case_helper_staging_failures(parent, hashes):
    with Fixture(parent, "install-helper-staging-failure", hashes) as fixture:
        fixture.helper.mkdir()
        code, output = fixture.callback("install")
        require(code != 0, f"install silently ignored helper staging failure: {output}")
        require(fixture.helper.is_dir(), "install staging failure replaced the blocking helper directory")
        require(not list(fixture.var.glob(".qwenpaw-supervisor-helper.*.tmp")),
                "failed install helper staging left a temporary file")

    with Fixture(parent, "upgrade-init-helper-staging-failure", hashes) as fixture:
        fixture.helper.mkdir()
        code, output = fixture.upgrade_init()
        require(code != 0, f"upgrade_init silently ignored helper staging failure: {output}")
        require(fixture.helper.is_dir(), "upgrade_init staging failure replaced the blocking helper directory")
        require(not list(fixture.var.glob(".qwenpaw-supervisor-helper.*.tmp")),
                "failed upgrade_init helper staging left a temporary file")


def prepare_cross_version_fixture(fixture):
    legacy_binary = fixture.root / "legacy-qwenpaw-supervisor"
    legacy_binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    os.chmod(legacy_binary, 0o755)
    legacy_upgrade_init = fixture.root / "legacy-upgrade-init"
    legacy_upgrade_init.write_text(
        f'#!/bin/sh\nexec "{legacy_binary}" platform upgrade-init\n', encoding="utf-8"
    )
    os.chmod(legacy_upgrade_init, 0o755)
    result = subprocess.run(
        [str(legacy_upgrade_init)], env=fixture.env, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True, timeout=10,
    )
    require(result.returncode == 0, f"legacy upgrade_init simulation failed: {result.stdout}")
    require(not fixture.helper.exists(), "legacy upgrade_init unexpectedly produced a var helper")
    shutil.copy2(legacy_binary, fixture.binary)
    os.chmod(fixture.binary, 0o644)
    return fixture.stage_installed_helper()


def case_cross_version_upgrade_uses_candidate_helper(parent, hashes):
    with Fixture(parent, "cross-version-upgrade", hashes) as fixture:
        candidate_helper = prepare_cross_version_fixture(fixture)
        require(not os.access(fixture.binary, os.X_OK), "target binary fixture is still executable")
        code, output = fixture.callback("upgrade")
        require(code == 1 and "升级后启动监督者失败" in output,
                f"outer helper did not enter Rust upgrade flow and fail closed on the unavailable target "
                f"(rc={code}): {output}")
        require("开始升级回调" in output,
                f"upgrade callback did not reach the Rust platform workflow: {output}")
        require(not fixture.helper.exists(), "cross-version upgrade relied on a newly staged var helper")
        require(not (fixture.var / "supervisor.pid").exists(),
                f"unavailable target started a long-lived manager via {candidate_helper}")


def case_cross_version_uninstall_uses_candidate_helper(parent, hashes):
    with Fixture(parent, "cross-version-uninstall", hashes) as fixture:
        prepare_cross_version_fixture(fixture)
        fixture.venv.mkdir()
        (fixture.venv / "legacy-runtime-sentinel").write_text("remove this venv\n")
        code, output = fixture.callback("uninstall", wizard_keep_data="keep")
        require(code == 0 and "卸载清理完成" in output,
                f"outer helper did not enter the real Rust uninstall flow (rc={code}): {output}")
        require(not fixture.venv.exists(), "outer-helper uninstall did not remove the venv")
        require(not fixture.helper.exists(), "cross-version uninstall depended on a var helper")
        require(fixture.user_data.is_dir(), "keep uninstall removed shared user data")
        require(fixture.sentinel.read_text() == "must survive uninstall remove\n",
                "cross-version uninstall changed the outer sentinel")


def case_installed_helper_starts_only_target_manager(parent, hashes):
    with Fixture(parent, "installed-helper-target-manager", hashes) as fixture:
        installed_helper = fixture.stage_installed_helper()
        code, output = fixture.command([str(installed_helper), "service", "start"])
        require(code == 0, f"installed helper could not start the target manager: {output}")
        manager = fixture.manager_identity()
        target_metadata = fixture.binary.stat()
        helper_metadata = installed_helper.stat()
        require(manager["exe_id"] == (target_metadata.st_dev, target_metadata.st_ino),
                f"long-lived manager did not use target/bin binary: {manager['exe']} vs {fixture.binary}")
        require(manager["exe_id"] != (helper_metadata.st_dev, helper_metadata.st_ino),
                "callback helper was started as a second long-lived manager")
        code, output = fixture.command([str(installed_helper), "service", "status"])
        require(code == 0, f"installed helper did not recognize the target manager identity: {output}")
        fixture.stop_manager()


def case_cmd_main_starts_installed_target_manager(parent, hashes):
    with Fixture(parent, "cmd-main-target-bin-manager", hashes) as fixture:
        require(fixture.binary.is_file(), "installed target/bin supervisor binary is missing")
        require(not (fixture.app / "app/bin/qwenpaw-supervisor").exists(),
                "fixture must not provide the FPK-internal app/bin path")
        installed_helper = fixture.stage_installed_helper()
        code, output = fixture.command(["bash", str(ROOT / "cmd/main"), "start"])
        require(code == 0, f"cmd/main could not start the target/bin manager (rc={code}): {output}")
        manager = fixture.manager_identity()
        target_metadata = fixture.binary.stat()
        require(manager["exe_id"] == (target_metadata.st_dev, target_metadata.st_ino),
                f"cmd/main did not start the installed target/bin manager: {manager['exe']}")
        code, output = fixture.command([str(installed_helper), "service", "status"])
        require(code == 0, f"installed helper did not recognize the target/bin manager: {output}")
        fixture.stop_manager()


def case_start_fails_closed_without_executable_target(parent, hashes):
    with Fixture(parent, "target-start-fail-closed", hashes) as fixture:
        installed_helper = fixture.stage_installed_helper()
        os.chmod(fixture.binary, 0o644)
        code, output = fixture.command([str(installed_helper), "service", "start"])
        require(code != 0, f"service start accepted a non-executable target binary: {output}")
        require(not (fixture.var / "supervisor.pid").exists(),
                "service start registered a manager without an executable target")
        require(not fixture.helper.exists(), "service start staged or launched a var helper")


def case_start_fails_closed_without_target_binary(parent, hashes):
    with Fixture(parent, "target-missing-start-fail-closed", hashes) as fixture:
        installed_helper = fixture.stage_installed_helper()
        fixture.binary.unlink()
        code, output = fixture.command([str(installed_helper), "service", "start"])
        require(code != 0, f"service start accepted a missing target binary: {output}")
        require(not (fixture.var / "supervisor.pid").exists(),
                "service start registered a manager when target/bin was missing")
        require(not fixture.helper.exists(), "service start staged or launched a var helper")


def case_stop_running_target_without_execute_bit(parent, hashes):
    with Fixture(parent, "target-stop-without-execute-bit", hashes) as fixture:
        installed_helper = fixture.stage_installed_helper()
        fixture.start_manager()
        manager = fixture.manager_identity()
        os.chmod(fixture.binary, 0o644)
        code, output = fixture.command([str(installed_helper), "service", "stop"], timeout=35)
        require(code == 0, f"outer helper could not stop the verified running target (rc={code}): {output}")
        wait_for(lambda: not alive(manager), 8, "verified target manager exit")
        require(not (fixture.var / "supervisor.pid").exists(), "stop left the manager PID registered")


def case_healthy_upgrade(parent, hashes):
    with Fixture(parent, "healthy-upgrade", hashes) as fixture:
        fixture.create_runtime("2.4.1")
        fixture.pin.write_text("2.4.1\n")
        fixture.user_data.joinpath("config.json").write_text("existing config\n")
        fixture.stage_installed_helper()
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
        fixture.stage_installed_helper()
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
        fixture.stage_installed_helper()
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
        fixture.stage_installed_helper()
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
        fixture.stage_installed_helper()
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


def case_fnos_layout_uninstall(parent, hashes, choice):
    with Fixture(parent, f"fnos-uninstall-{choice}", hashes, fnos_layout=True) as fixture:
        fixture.create_runtime("2.4.1")
        fixture.pin.write_text("2.4.1\n")
        fixture.user_data.joinpath("config.json").write_text("shared-data-sentinel\n")
        fixture.user_secret.joinpath("auth.json").write_text("auth-sentinel\n")
        (fixture.home / "workspace-sentinel").write_text("home-sentinel\n")
        (fixture.etc / "settings").write_text("etc-sentinel\n")
        (fixture.var / "var-sentinel").write_text("var-sentinel\n")
        shutil.copy2(SUPERVISOR_SOURCE, fixture.helper)
        os.chmod(fixture.helper, 0o750)
        fixture.stage_installed_helper()
        code, output = fixture.callback("uninstall", wizard_keep_data=choice)
        require(code == 0 and "卸载清理完成" in output,
                f"real fnOS layout uninstall {choice} failed (rc={code}): {output}")
        require(fixture.sentinel.read_text() == "must survive uninstall remove\n",
                "real fnOS layout uninstall changed the outer sentinel")
        if choice == "remove":
            for path in (fixture.home, fixture.etc, fixture.var,
                         fixture.user_data, fixture.user_secret):
                require(not path.exists(), f"remove left real fnOS deletion target: {path}")
        else:
            require(not fixture.venv.exists(), "keep uninstall left home/venv")
            require(fixture.pin.read_text() == "2.4.1\n", "keep uninstall removed the version pin")
            require(fixture.helper.is_file(), "keep uninstall removed the var helper")
            require((fixture.home / "workspace-sentinel").read_text() == "home-sentinel\n",
                    "keep uninstall removed other home data")
            require((fixture.etc / "settings").read_text() == "etc-sentinel\n",
                    "keep uninstall removed etc data")
            require((fixture.var / "var-sentinel").read_text() == "var-sentinel\n",
                    "keep uninstall removed other var data")
            require(fixture.user_data.joinpath("config.json").read_text()
                    == "shared-data-sentinel\n", "keep uninstall changed shared data")
            require(fixture.user_secret.joinpath("auth.json").read_text() == "auth-sentinel\n",
                    "keep uninstall changed authentication data")


def case_fnos_layout_upgrade_callback(parent, hashes):
    with Fixture(parent, "fnos-upgrade-callback", hashes, fnos_layout=True) as fixture:
        fixture.stage_installed_helper()
        code, output = fixture.callback("upgrade")
        supervisor_log = fixture.var / "supervisor.log"
        supervisor_output = supervisor_log.read_text(errors="replace") if supervisor_log.exists() else "<absent>"
        require(code == 0,
                f"formal upgrade callback rejected real fnOS path layout (rc={code}): {output}; "
                f"supervisor log: {supervisor_output}")
        require("开始升级回调" in output,
                f"formal upgrade callback did not enter Rust upgrade workflow: {output}")
        require("卸载路径校验失败" not in output,
                f"upgrade callback reported uninstall-path validation failure: {output}")
        require(fixture.manager_identity()["exe_id"]
                == (fixture.binary.stat().st_dev, fixture.binary.stat().st_ino),
                "real-layout upgrade did not start the packaged target supervisor")


def case_fnos_layout_rejects_wrong_root(parent, hashes):
    with Fixture(parent, "fnos-wrong-root", hashes, fnos_layout=True) as fixture:
        wrong_home = fixture.root / "vol1/@wrong/qwenpaw"
        wrong_home.mkdir(parents=True)
        fixture.env["TRIM_PKGHOME"] = str(wrong_home)
        fixture.stage_installed_helper()
        code, output = fixture.callback("uninstall", wizard_keep_data="remove")
        require(code == 1 and "卸载路径校验失败" in output,
                f"uninstall accepted an incorrect platform root (rc={code}): {output}")
        require(wrong_home.is_dir(), "invalid platform root was deleted")
        require(fixture.home.is_dir(), "configured valid home root was affected")
        require(fixture.etc.is_dir() and fixture.var.is_dir(),
                "invalid platform root allowed other package roots to be deleted")
        require(fixture.user_data.is_dir() and fixture.user_secret.is_dir(),
                "invalid platform root allowed shared data to be deleted")


def case_uds_failure(parent, hashes):
    with Fixture(parent, "uds-failure", hashes) as fixture:
        fixture.create_runtime("2.1.0")
        fixture.pin.write_text("2.1.0\n")
        before = (fixture.venv / "installed").read_bytes()
        (fixture.control / "remove-control-socket").write_text("yes\n")
        fixture.stage_installed_helper()
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
        fixture.stage_installed_helper()
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
    with tempfile.TemporaryDirectory(prefix="qpc-") as temporary:
        parent = Path(temporary)
        manifest = parent / "binary-sources.sha256"
        manifest.write_text(f"{hashes['supervisor']}  {SUPERVISOR_SOURCE}\n"
                            f"{hashes['runtime']}  {RUNTIME_SOURCE}\n")
        failures = []
        for action in ("upgrade", "uninstall"):
            run_case(f"{action} callback prefers an existing var helper over its sibling",
                     lambda action=action: case_lifecycle_callback_prefers_var_helper(
                         parent, hashes, action), failures)
        run_case("empty shell install does not create a venv or run pip",
                 lambda: case_empty_shell(parent, hashes), failures)
        run_case("empty shell upgrade keeps the shell usable without installing a kernel",
                 lambda: case_empty_shell_upgrade(parent, hashes), failures)
        run_case("lifecycle lock uses writable var when its parent is read-only",
                 lambda: case_lifecycle_lock_with_read_only_parent(parent, hashes), failures)
        run_case("removed target and absent sibling helper return 127",
                 lambda: case_removed_target_without_helper_is_127(parent, hashes), failures)
        run_case("upgrade-init runs the candidate supervisor and stages its var helper",
                 lambda: case_upgrade_init_prefers_candidate_supervisor(parent, hashes), failures)
        run_case("legacy target upgrade-init does not stage a var helper",
                 lambda: case_legacy_upgrade_init_does_not_stage_var_helper(parent, hashes), failures)
        run_case("installed callback falls back to an existing var helper",
                 lambda: case_installed_callback_falls_back_to_var_helper(parent, hashes), failures)
        run_case("legacy var helper stages but removed target upgrade fails closed and uninstall keeps data",
                 lambda: case_persistent_helper_after_target_removal(parent, hashes), failures)
        run_case("remove uninstall cleans the retained helper within the var deletion scope",
                 lambda: case_helper_remove_cleans_var_after_target_removal(parent, hashes), failures)
        run_case("helper staging failures make install and upgrade-init nonzero",
                 lambda: case_helper_staging_failures(parent, hashes), failures)
        run_case("cross-version upgrade without var helper enters Rust and fails closed on unavailable target",
                 lambda: case_cross_version_upgrade_uses_candidate_helper(parent, hashes), failures)
        run_case("cross-version uninstall without var helper uses sibling Rust helper",
                 lambda: case_cross_version_uninstall_uses_candidate_helper(parent, hashes), failures)
        run_case("installed helper starts only the packaged target supervisor",
                 lambda: case_installed_helper_starts_only_target_manager(parent, hashes), failures)
        run_case("cmd/main starts only target/bin and outer helper recognizes that manager",
                 lambda: case_cmd_main_starts_installed_target_manager(parent, hashes), failures)
        run_case("outer helper refuses to start a non-executable target",
                 lambda: case_start_fails_closed_without_executable_target(parent, hashes), failures)
        run_case("outer helper refuses to start when target/bin is missing",
                 lambda: case_start_fails_closed_without_target_binary(parent, hashes), failures)
        run_case("outer helper stops a verified running target after its execute bit is removed",
                 lambda: case_stop_running_target_without_execute_bit(parent, hashes), failures)
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
        for choice in ("keep", "remove"):
            run_case(f"real fnOS roots uninstall {choice} with the exact deletion scope",
                     lambda choice=choice: case_fnos_layout_uninstall(parent, hashes, choice), failures)
        run_case("real fnOS roots pass the formal upgrade callback",
                 lambda: case_fnos_layout_upgrade_callback(parent, hashes), failures)
        run_case("wrong fnOS package root is rejected before deletion",
                 lambda: case_fnos_layout_rejects_wrong_root(parent, hashes), failures)
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
