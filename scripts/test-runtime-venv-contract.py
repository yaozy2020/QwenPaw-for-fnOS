#!/usr/bin/env python3
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlsplit
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("QWENPAW_TEST_SUP_BIN", ROOT / "supervisor/target/release/qwenpaw-supervisor")).resolve()
TOKEN = "runtime_contract_test_token_12345678901234567890"
VERSIONS = ("1.0.0", "2.0.0", "3.0.0")
FAILURES = []


def check(condition, message):
    if condition:
        print(f"[PASS] {message}", flush=True)
    else:
        print(f"[FAIL] {message}", flush=True)
        FAILURES.append(message)


def test_environment(root):
    env = dict(os.environ)
    for key in list(env):
        lower = key.lower()
        if key.startswith("PIP_") or key.startswith("PYTHON") or lower.endswith("_proxy"):
            env.pop(key, None)
    env.update(
        {
            "HOME": str(root / "runner-home"),
            "TMPDIR": str(root / "tmp"),
            "XDG_CACHE_HOME": str(root / "cache"),
            "PIP_CONFIG_FILE": os.devnull,
            "PIP_DISABLE_PIP_VERSION_CHECK": "1",
            "PIP_NO_CACHE_DIR": "1",
            "PIP_NO_INPUT": "1",
            "PYTHONNOUSERSITE": "1",
            "NO_PROXY": "*",
            "no_proxy": "*",
        }
    )
    for name in ("HOME", "TMPDIR", "XDG_CACHE_HOME"):
        Path(env[name]).mkdir(parents=True, exist_ok=True)
    return env


def run_process(argv, *, env, cwd, timeout=30, stdin_text=None):
    input_options = {"stdin": subprocess.DEVNULL} if stdin_text is None else {"input": stdin_text}
    return subprocess.run(
        [str(part) for part in argv],
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
        check=False,
        **input_options,
    )


def create_wheel(wheel_dir, distribution, version, files, entry_points=None):
    normalized = distribution.replace("-", "_")
    dist_info = f"{normalized}-{version}.dist-info"
    contents = dict(files)
    contents[f"{dist_info}/METADATA"] = (
        "Metadata-Version: 2.1\n"
        f"Name: {distribution}\n"
        f"Version: {version}\n"
        "Requires-Python: >=3.8\n\n"
    ).encode()
    contents[f"{dist_info}/WHEEL"] = (
        "Wheel-Version: 1.0\n"
        "Generator: qwenpaw-runtime-venv-contract\n"
        "Root-Is-Purelib: true\n"
        "Tag: py3-none-any\n\n"
    ).encode()
    if entry_points:
        contents[f"{dist_info}/entry_points.txt"] = entry_points.encode()

    record_name = f"{dist_info}/RECORD"
    record = []
    for name, data in sorted(contents.items()):
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode().rstrip("=")
        record.append(f"{name},sha256={digest},{len(data)}\n")
    record.append(f"{record_name},,\n")
    contents[record_name] = "".join(record).encode()

    filename = f"{normalized}-{version}-py3-none-any.whl"
    wheel_path = wheel_dir / filename
    with zipfile.ZipFile(wheel_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, data in sorted(contents.items()):
            archive.writestr(name, data)
    return wheel_path


def make_qwenpaw_wheels(wheel_dir):
    built = {}
    for version in VERSIONS:
        package = {
            "qwenpaw/__init__.py": f"__version__ = {version!r}\n".encode(),
            "qwenpaw/cli.py": (
                "import json\n"
                "import sys\n"
                "import qwenpaw\n"
                "\n"
                "def main():\n"
                "    if sys.argv[1:] != ['--contract-probe']:\n"
                "        return 2\n"
                "    print(json.dumps({'version': qwenpaw.__version__, 'prefix': sys.prefix, 'module': qwenpaw.__file__}))\n"
                "    return 0\n"
            ).encode(),
            "qwenpaw/__main__.py": (
                "import argparse\n"
                "import json\n"
                "import os\n"
                "import sys\n"
                "from pathlib import Path\n"
                "from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer\n"
                "import qwenpaw\n"
                "\n"
                "if sys.argv[1:] == ['init', '--defaults']:\n"
                "    if sys.stdin.read().strip() != 'Yes':\n"
                "        raise SystemExit(24)\n"
                "    working_dir = Path(os.environ['QWENPAW_WORKING_DIR'])\n"
                "    working_dir.mkdir(parents=True, exist_ok=True)\n"
                "    count_path = working_dir / '.runtime-contract-init-count'\n"
                "    count = int(count_path.read_text()) if count_path.exists() else 0\n"
                "    count_path.write_text(f'{count + 1}\\n')\n"
                "    config_path = working_dir / 'config.json'\n"
                "    workspaces_path = working_dir / 'workspaces'\n"
                "    if os.path.lexists(config_path) or os.path.lexists(workspaces_path):\n"
                "        raise SystemExit(25)\n"
                "    with config_path.open('x') as config:\n"
                "        config.write(json.dumps({'initialized': True}) + '\\n')\n"
                "    raise SystemExit(0)\n"
                "\n"
                "parser = argparse.ArgumentParser()\n"
                "subparsers = parser.add_subparsers(dest='command', required=True)\n"
                "app = subparsers.add_parser('app')\n"
                "app.add_argument('--host', required=True)\n"
                "app.add_argument('--port', required=True, type=int)\n"
                "app.add_argument('--log-level', required=True)\n"
                "args = parser.parse_args()\n"
                "if qwenpaw.__version__ == '3.0.0':\n"
                "    print('contract: qwenpaw 3.0.0 app startup failure', file=sys.stderr)\n"
                "    raise SystemExit(23)\n"
                "\n"
                "class Handler(BaseHTTPRequestHandler):\n"
                "    def do_GET(self):\n"
                "        if self.path != '/api/version':\n"
                "            self.send_error(404)\n"
                "            return\n"
                "        payload = json.dumps({'version': qwenpaw.__version__, 'prefix': sys.prefix, 'module': qwenpaw.__file__}).encode()\n"
                "        self.send_response(200)\n"
                "        self.send_header('Content-Type', 'application/json')\n"
                "        self.send_header('Content-Length', str(len(payload)))\n"
                "        self.end_headers()\n"
                "        self.wfile.write(payload)\n"
                "    def log_message(self, *_args):\n"
                "        pass\n"
                "\n"
                "ThreadingHTTPServer((args.host, args.port), Handler).serve_forever()\n"
            ).encode(),
        }
        wheel = create_wheel(
            wheel_dir,
            "qwenpaw",
            version,
            package,
            "[console_scripts]\nqwenpaw-contract = qwenpaw.cli:main\n",
        )
        built[version] = wheel
    return built


def create_probe_wheel(wheel_dir):
    return create_wheel(
        wheel_dir,
        "runtime-contract-probe",
        "1.0.0",
        {"runtime_contract_probe/__init__.py": b"VALUE = 'local-pip-operation-ok'\n"},
    )


class SimpleIndexHandler(BaseHTTPRequestHandler):
    wheel_dir = None
    versions = ()
    requests = []
    requests_lock = threading.Lock()

    def _respond(self, send_body):
        path = urlsplit(self.path).path
        with self.requests_lock:
            self.requests.append((self.client_address[0], path, self.headers.get("Host", "")))

        if path == "/simple/qwenpaw/":
            port = self.server.server_address[1]
            links = "".join(
                f'<a href="http://127.0.0.1:{port}/packages/{quote(f"qwenpaw-{version}-py3-none-any.whl")}">'
                f'qwenpaw-{version}-py3-none-any.whl</a><br>\n'
                for version in self.versions
            )
            body = f"<!doctype html><html><body>{links}</body></html>".encode()
            content_type = "text/html; charset=utf-8"
        elif path.startswith("/packages/"):
            filename = path.removeprefix("/packages/")
            if Path(filename).name != filename:
                self.send_error(400)
                return
            wheel_path = self.wheel_dir / filename
            if not wheel_path.is_file():
                self.send_error(404)
                return
            body = wheel_path.read_bytes()
            content_type = "application/octet-stream"
        else:
            self.send_error(404)
            return

        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if send_body:
            self.wfile.write(body)

    def do_GET(self):
        self._respond(True)

    def do_HEAD(self):
        self._respond(False)

    def log_message(self, *_args):
        pass


def start_simple_index(wheel_dir, versions):
    handler = type(
        "LocalSimpleIndexHandler",
        (SimpleIndexHandler,),
        {"wheel_dir": wheel_dir, "versions": versions, "requests": [], "requests_lock": threading.Lock()},
    )
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, name="local-simple-index", daemon=True)
    thread.start()
    return server, thread, f"http://127.0.0.1:{server.server_address[1]}/simple"


def free_port(excluded=()):
    for _ in range(100):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        if port not in excluded and port not in (19090, 19091):
            return port
    raise RuntimeError("unable to allocate a loopback test port")


def http_json(url, *, token, method="GET", body=None, timeout=5):
    data = None if body is None else json.dumps(body).encode()
    headers = {"X-Supervisor-Token": token}
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        response = urllib.request.urlopen(request, timeout=timeout)
    except HTTPError as error:
        response = error
    with response:
        payload = response.read()
        try:
            parsed = json.loads(payload) if payload else None
        except (json.JSONDecodeError, UnicodeDecodeError):
            parsed = None
        return response.status, parsed, payload.decode("utf-8", "replace")


def read_process_identity(pid):
    try:
        stat_text = Path(f"/proc/{pid}/stat").read_text()
        fields = stat_text[stat_text.rfind(")") + 2 :].split()
        if len(fields) < 20:
            return None
        executable = os.stat(f"/proc/{pid}/exe")
        cmdline = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        environ = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        return {
            "pid": pid,
            "starttime": int(fields[19]),
            "state": fields[0],
            "device": executable.st_dev,
            "inode": executable.st_ino,
            "cmdline": cmdline,
            "environ": set(environ),
        }
    except (OSError, ValueError, IndexError):
        return None


def process_identity_matches(process, identity):
    current = read_process_identity(process.pid)
    return bool(
        current
        and current["state"] not in ("Z", "X")
        and current["starttime"] == identity["starttime"]
        and (current["device"], current["inode"]) == (identity["device"], identity["inode"])
        and current["cmdline"]
        and current["cmdline"][0] == os.fsencode(str(BIN))
    )


def supervisor_identity_matches(process, identity, env):
    current = read_process_identity(process.pid)
    return bool(
        process_identity_matches(process, identity)
        and current
        and f"TRIM_PKGHOME={env['TRIM_PKGHOME']}".encode() in current["environ"]
        and f"TRIM_PKGVAR={env['TRIM_PKGVAR']}".encode() in current["environ"]
    )


def signal_owned_supervisor(process, identity, env, signum):
    if process.poll() is not None:
        return False
    if not process_identity_matches(process, identity):
        raise RuntimeError(f"supervisor PID {process.pid} identity changed; refusing to signal")
    if hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"):
        pidfd = os.pidfd_open(process.pid)
        try:
            if not process_identity_matches(process, identity):
                raise RuntimeError(f"supervisor PID {process.pid} identity changed before pidfd signal")
            signal.pidfd_send_signal(pidfd, signum)
        finally:
            os.close(pidfd)
    elif signum == signal.SIGTERM:
        process.terminate()
    else:
        process.kill()
    return True


def wait_for(predicate, *, timeout, description, process=None, log_path=None):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            detail = log_path.read_text(errors="replace") if log_path and log_path.exists() else ""
            raise RuntimeError(f"supervisor exited with {process.returncode} while {description}\n{detail}")
        try:
            result = predicate()
            if result:
                return result
        except (OSError, URLError, TimeoutError) as error:
            last_error = error
        time.sleep(0.1)
    detail = log_path.read_text(errors="replace") if log_path and log_path.exists() else ""
    suffix = f"; last request error: {last_error}" if last_error else ""
    raise TimeoutError(f"timed out {description}{suffix}\n{detail}")


def create_trim_fixture(root, system_python, index_url, supervisor_port, kernel_port):
    trim = root / "trim"
    paths = {
        "home": trim / "home",
        "var": trim / "var",
        "etc": trim / "etc",
        "appdest": trim / "appdest",
        "share": trim / "share",
    }
    for path in paths.values():
        path.mkdir(parents=True, exist_ok=True)
    (paths["var"] / "supervisor-token").write_text(TOKEN)
    (paths["var"] / "service_port").write_text(f"{kernel_port}\n")
    (paths["var"] / "qwenpaw-log-level").write_text("info\n")
    ui = paths["appdest"] / "ui"
    ui.mkdir()
    (ui / "config").write_text(
        json.dumps(
            {
                ".url": {
                    "qwenpaw.main": {"port": str(kernel_port), "noDisplay": True},
                    "qwenpaw.console": {"type": "iframe", "port": "15666"},
                }
            }
        )
        + "\n"
    )
    (paths["share"] / ".qwenpaw").mkdir()

    env = test_environment(root)
    env.update(
        {
            "TRIM_PKGHOME": str(paths["home"]),
            "TRIM_PKGVAR": str(paths["var"]),
            "TRIM_PKGETC": str(paths["etc"]),
            "TRIM_APPDEST": str(paths["appdest"]),
            "TRIM_DATA_SHARE_PATHS": str(paths["share"]),
            "TRIM_SERVICE_PORT": str(kernel_port),
            "SUPERVISOR_PORT": str(supervisor_port),
            "SUPERVISOR_AUTOSTART": "0",
            "QWENPAW_WEB_PROXY_PORT": "off",
            "QWENPAW_PYTHON_BIN": str(system_python),
            "QWENPAW_CONTRACT_ROOT": str(root),
            "QWENPAW_CONTRACT_INDEX": index_url,
        }
    )
    return paths, env


def start_supervisor(root, env):
    log_path = root / "supervisor.stdout.log"
    log_file = log_path.open("wb")
    process = subprocess.Popen(
        [str(BIN)],
        cwd=root,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=log_file,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    identity = None
    try:
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            candidate = read_process_identity(process.pid)
            if process.poll() is not None:
                break
            if candidate and candidate["state"] not in ("Z", "X"):
                identity = candidate
                if supervisor_identity_matches(process, identity, env):
                    break
            time.sleep(0.02)
        if identity is None or not supervisor_identity_matches(process, identity, env):
            raise RuntimeError(f"could not verify isolated release supervisor identity (exit={process.poll()})")

        base = f"http://127.0.0.1:{env['SUPERVISOR_PORT']}"
        wait_for(
            lambda: (lambda result: result if result[0] == 200 and result[1] else None)(
                http_json(base + "/status", token=TOKEN)
            ),
            timeout=10,
            description="waiting for release supervisor HTTP /status",
            process=process,
            log_path=log_path,
        )
        return process, identity, log_file, log_path, base
    except Exception as error:
        cleanup_error = None
        try:
            if process.poll() is None:
                candidate = identity or read_process_identity(process.pid)
                if candidate and process_identity_matches(process, candidate):
                    signal_owned_supervisor(process, candidate, env, signal.SIGTERM)
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        signal_owned_supervisor(process, candidate, env, signal.SIGKILL)
                        process.wait(timeout=5)
                else:
                    cleanup_error = "supervisor identity could not be verified; refusing to signal"
        except Exception as cleanup:
            cleanup_error = str(cleanup)
        finally:
            log_file.close()
        detail = log_path.read_text(errors="replace") if log_path.exists() else ""
        suffix = f"; cleanup error: {cleanup_error}" if cleanup_error else ""
        raise RuntimeError(f"{error}{suffix}\n{detail}") from error


def wait_install_terminal(base, process, log_path):
    def poll():
        status, payload, _ = http_json(base + "/kernel/install-status", token=TOKEN)
        if status != 200 or not isinstance(payload, dict):
            return None
        phase = payload.get("phase")
        if not payload.get("upgrading") and phase in ("done", "failed"):
            return payload
        return None

    return wait_for(
        poll,
        timeout=120,
        description="waiting for real HTTP install transaction terminal state",
        process=process,
        log_path=log_path,
    )


def submit_install(base, version, index_url):
    status, payload, raw = http_json(
        base + "/kernel/install",
        token=TOKEN,
        method="POST",
        body={"version": version, "mirror": index_url},
        timeout=20,
    )
    check(
        status == 200 and isinstance(payload, dict) and payload.get("success") is True,
        f"real HTTP POST /kernel/install accepted qwenpaw=={version} (HTTP {status}; {raw[:200]})",
    )


def check_generation(paths, env, root, expected_version, kernel_port, probe_wheel, base, label):
    active = paths["home"] / "venv"
    python = active / "bin/python3"
    print(f"[CHECK] {label}: active={active}", flush=True)

    check(python.is_file() and os.access(python, os.X_OK), f"{label}: active venv contains an executable real python3")
    if not python.is_file() or not os.access(python, os.X_OK):
        return

    inspection_code = (
        "import json, pathlib, sys, qwenpaw; "
        "print(json.dumps({'prefix': sys.prefix, 'executable': sys.executable, "
        "'version': qwenpaw.__version__, 'module': qwenpaw.__file__}))"
    )
    try:
        inspected = run_process([python, "-c", inspection_code], env=env, cwd=root, timeout=15)
        inspected_json = json.loads(inspected.stdout) if inspected.returncode == 0 else None
        check(
            inspected.returncode == 0 and isinstance(inspected_json, dict),
            f"{label}: active venv python imports qwenpaw",
        )
        if inspected_json:
            check(
                Path(inspected_json["prefix"]).resolve() == active.resolve(),
                f"{label}: active venv python sys.prefix resolves to active path",
            )
            check(
                inspected_json.get("version") == expected_version,
                f"{label}: imported qwenpaw version is {expected_version}",
            )
            check(
                Path(inspected_json["module"]).resolve().is_relative_to(active.resolve()),
                f"{label}: imported qwenpaw module is inside active venv",
            )
        elif inspected.returncode != 0:
            check(False, f"{label}: venv import probe stderr={inspected.stderr.strip()!r}")
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
        check(False, f"{label}: active venv import/sys.prefix probe ran: {error}")

    console_script = active / "bin/qwenpaw-contract"
    check(console_script.is_file() and os.access(console_script, os.X_OK), f"{label}: pip created executable qwenpaw-contract console script")
    if console_script.is_file():
        try:
            shebang = console_script.read_text(errors="replace").splitlines()[0]
            check(
                shebang == f"#!{python}",
                f"{label}: console-script shebang points at active python3 (found {shebang!r})",
            )
            result = run_process([console_script, "--contract-probe"], env=env, cwd=root, timeout=15)
            try:
                payload = json.loads(result.stdout) if result.returncode == 0 else None
            except json.JSONDecodeError:
                payload = None
            check(
                result.returncode == 0 and isinstance(payload, dict)
                and payload.get("version") == expected_version
                and Path(payload.get("prefix", "/nonexistent")).resolve() == active.resolve(),
                f"{label}: console-script executes with correct version and active sys.prefix"
                f" (exit={result.returncode}, stderr={result.stderr.strip()!r})",
            )
        except (OSError, subprocess.SubprocessError, IndexError) as error:
            check(False, f"{label}: console-script shebang/execution probe ran: {error}")
    else:
        check(False, f"{label}: console-script cannot execute because it was not created")

    try:
        pip_version = run_process([python, "-m", "pip", "--version"], env=env, cwd=root, timeout=20)
        check(
            pip_version.returncode == 0 and str(active) in pip_version.stdout,
            f"{label}: subsequent python -m pip --version uses active venv (exit={pip_version.returncode})",
        )
        if pip_version.returncode != 0:
            check(False, f"{label}: pip --version stderr={pip_version.stderr.strip()!r}")

        pip_install = run_process(
            [
                python,
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-cache-dir",
                "--no-input",
                "--no-index",
                "--no-deps",
                "--force-reinstall",
                probe_wheel,
            ],
            env=env,
            cwd=root,
            timeout=45,
        )
        check(
            pip_install.returncode == 0,
            f"{label}: subsequent real pip install succeeds using a local wheel only"
            f" (exit={pip_install.returncode}; {pip_install.stderr.strip()[-300:]!r})",
        )
        pip_check = run_process([python, "-m", "pip", "check"], env=env, cwd=root, timeout=20)
        check(
            pip_check.returncode == 0 and "No broken requirements found" in pip_check.stdout,
            f"{label}: subsequent real pip check passes (exit={pip_check.returncode})",
        )
        probe = run_process(
            [python, "-c", "import runtime_contract_probe as p; print(p.VALUE)"],
            env=env,
            cwd=root,
            timeout=15,
        )
        check(
            probe.returncode == 0 and probe.stdout.strip() == "local-pip-operation-ok",
            f"{label}: locally installed follow-up package imports from active venv",
        )
    except (OSError, subprocess.SubprocessError) as error:
        check(False, f"{label}: follow-up real pip operation ran: {error}")

    try:
        status, health, raw = http_json(
            f"http://127.0.0.1:{kernel_port}/api/version", token=TOKEN, timeout=5
        )
        check(
            status == 200 and isinstance(health, dict) and health.get("version") == expected_version,
            f"{label}: actual managed kernel health endpoint reports {expected_version} (HTTP {status}; {raw[:200]})",
        )
        if isinstance(health, dict):
            check(
                Path(health.get("prefix", "/nonexistent")).resolve() == active.resolve(),
                f"{label}: actual managed kernel sys.prefix resolves to active venv",
            )
            check(
                Path(health.get("module", "/nonexistent")).resolve().is_relative_to(active.resolve()),
                f"{label}: actual managed kernel imports qwenpaw from active venv",
            )
    except (OSError, URLError, TimeoutError, json.JSONDecodeError) as error:
        check(False, f"{label}: actual managed kernel health request succeeded: {error}")

    try:
        status, snapshot, _ = http_json(base + "/status", token=TOKEN)
        check(
            status == 200 and isinstance(snapshot, dict)
            and snapshot.get("installed") is True
            and snapshot.get("running") is True
            and snapshot.get("version") == expected_version,
            f"{label}: supervisor HTTP /status agrees with live kernel version {expected_version}",
        )
    except (OSError, URLError, TimeoutError) as error:
        check(False, f"{label}: supervisor status confirms installed/running version: {error}")

    pin_path = paths["var"] / "qwenpaw-version"
    try:
        pin = pin_path.read_text().strip()
    except OSError:
        pin = None
    check(pin == expected_version, f"{label}: committed version pin is {expected_version}")


def read_init_count(working_dir):
    try:
        return int((working_dir / ".runtime-contract-init-count").read_text().strip())
    except (OSError, ValueError):
        return None


def check_init_count(working_dir, expected, label):
    count = read_init_count(working_dir)
    check(count == expected, f"{label}: initialization count is {expected} (found {count!r})")


def check_user_data(working_dir, expected_files, label):
    for relative_path, expected in expected_files.items():
        try:
            actual = (working_dir / relative_path).read_bytes()
        except OSError:
            actual = None
        check(actual == expected, f"{label}: existing {relative_path} remains byte-for-byte unchanged")


def check_initialization_protocol(paths, env, root):
    python = paths["home"] / "venv/bin/python3"
    probe_root = root / "init-probes"
    cases = (
        ("wrong-confirmation", "wrong", "No\n"),
        ("existing-config", "config", "Yes\n"),
        ("existing-workspaces", "workspaces", "Yes\n"),
    )
    for name, existing_data, confirmation in cases:
        working_dir = probe_root / name
        working_dir.mkdir(parents=True)
        original_config = None
        original_workspace_marker = None
        if existing_data == "config":
            original_config = '{"preserve": "config"}\n'
            (working_dir / "config.json").write_text(original_config)
        elif existing_data == "workspaces":
            workspace = working_dir / "workspaces"
            workspace.mkdir()
            original_workspace_marker = "preserve workspace data\n"
            (workspace / "marker").write_text(original_workspace_marker)

        probe_env = dict(env)
        probe_env["QWENPAW_WORKING_DIR"] = str(working_dir)
        result = run_process(
            [python, "-m", "qwenpaw", "init", "--defaults"],
            env=probe_env,
            cwd=root,
            timeout=15,
            stdin_text=confirmation,
        )
        check(
            result.returncode != 0,
            f"init protocol rejects {name} (exit={result.returncode})",
        )
        if existing_data == "wrong":
            check(
                not (working_dir / "config.json").exists()
                and read_init_count(working_dir) is None,
                "init protocol requires stdin Yes before recording or creating data",
            )
        elif existing_data == "config":
            check(
                (working_dir / "config.json").read_text() == original_config
                and not (working_dir / "workspaces").exists(),
                "init protocol preserves existing config.json without creating workspaces",
            )
            check_init_count(working_dir, 1, "existing-config protection probe")
        else:
            check(
                (working_dir / "workspaces/marker").read_text() == original_workspace_marker
                and not (working_dir / "config.json").exists(),
                "init protocol preserves existing workspaces without creating config.json",
            )
            check_init_count(working_dir, 1, "existing-workspaces protection probe")


def tcp_port_closed(port):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.5)
        return sock.connect_ex(("127.0.0.1", port)) != 0


def stop_supervisor(process, identity, env, log_file, log_path):
    exit_code = None
    if process is not None:
        if process.poll() is None:
            signal_owned_supervisor(process, identity, env, signal.SIGTERM)
            try:
                exit_code = process.wait(timeout=25)
            except subprocess.TimeoutExpired:
                signal_owned_supervisor(process, identity, env, signal.SIGKILL)
                exit_code = process.wait(timeout=5)
        else:
            exit_code = process.returncode
    if log_file is not None and not log_file.closed:
        log_file.close()
    if process is not None:
        check(exit_code == 0, f"isolated release supervisor shuts down cleanly (exit={exit_code})")
    if log_path and log_path.exists():
        print(f"SUPERVISOR_LOG={log_path}", flush=True)


def run_contract(root):
    if not BIN.is_file() or not os.access(BIN, os.X_OK):
        raise RuntimeError(f"required release supervisor is missing or not executable: {BIN}")
    supervisor_sha = hashlib.sha256(BIN.read_bytes()).hexdigest()
    print(f"SUPERVISOR_BINARY={BIN}", flush=True)
    print(f"SUPERVISOR_SHA256={supervisor_sha}", flush=True)
    print(f"SYSTEM_PYTHON={Path(sys.executable).resolve()}", flush=True)
    print(f"PYTHON_VERSION={sys.version.splitlines()[0]}", flush=True)
    print(f"TEMP_ROOT={root}", flush=True)
    print("REQUIRED_ASSERTIONS:", flush=True)
    for assertion in (
        "release supervisor is exercised through authenticated real HTTP install/status APIs",
        "real local Python venv has active sys.prefix, qwenpaw import, executable console script, and pip",
        "all qwenpaw release wheels are fetched only from a localhost PEP 503 Simple Index",
        "post-activation pip install/check and package import work without an external index",
        "the live /api/version health and supervisor status match the installed package generation",
        "an installable but non-starting generation triggers rollback and the previous generation remains usable",
        "release SHA256 is recorded and the owned supervisor is safely stopped before temporary-root cleanup",
    ):
        print(f"  - {assertion}", flush=True)

    env = test_environment(root)
    system_python = Path(sys.executable).resolve()
    preflight_venv = root / "venv-preflight"
    created = run_process([system_python, "-m", "venv", preflight_venv], env=env, cwd=root, timeout=180)
    if created.returncode != 0:
        raise RuntimeError(
            "system Python cannot create a real venv; this contract test must fail, not skip:\n"
            f"stdout={created.stdout}\nstderr={created.stderr}"
        )
    preflight_python = preflight_venv / "bin/python3"
    if not preflight_python.is_file() or not os.access(preflight_python, os.X_OK):
        raise RuntimeError("system Python venv capability probe produced no executable bin/python3")
    pip_probe = run_process([preflight_python, "-m", "pip", "--version"], env=env, cwd=root, timeout=30)
    if pip_probe.returncode != 0:
        raise RuntimeError(
            "real venv pip is unavailable; this contract test must fail, not skip:\n"
            f"stdout={pip_probe.stdout}\nstderr={pip_probe.stderr}"
        )
    print(f"VENV_PYTHON={preflight_python}", flush=True)
    print(f"VENV_PIP={pip_probe.stdout.strip()}", flush=True)
    check(True, "system Python creates a real venv containing working pip")
    shutil.rmtree(preflight_venv)

    wheel_dir = root / "wheelhouse"
    wheel_dir.mkdir()
    qwenpaw_wheels = make_qwenpaw_wheels(wheel_dir)
    probe_wheel = create_probe_wheel(wheel_dir)
    for version, wheel in qwenpaw_wheels.items():
        print(f"LOCAL_WHEEL_SHA256_{version}={hashlib.sha256(wheel.read_bytes()).hexdigest()} {wheel.name}", flush=True)

    index_server = None
    index_thread = None
    supervisor = None
    supervisor_identity = None
    supervisor_log_file = None
    supervisor_log_path = None
    paths = None
    supervisor_env = None
    kernel_port = None
    try:
        index_server, index_thread, index_url = start_simple_index(wheel_dir, VERSIONS)
        sup_port = free_port()
        kernel_port = free_port((sup_port,))
        paths, supervisor_env = create_trim_fixture(root, system_python, index_url, sup_port, kernel_port)
        supervisor, supervisor_identity, supervisor_log_file, supervisor_log_path, base = start_supervisor(root, supervisor_env)
        print(f"LOCAL_SIMPLE_INDEX={index_url}/qwenpaw/", flush=True)
        print(f"SUPERVISOR_HTTP={base}", flush=True)
        print(f"KERNEL_HTTP=http://127.0.0.1:{kernel_port}", flush=True)
        check(True, "release supervisor started as an identity-verified isolated process with temporary TRIM paths")

        submit_install(base, "1.0.0", index_url)
        first = wait_install_terminal(base, supervisor, supervisor_log_path)
        check(first.get("phase") == "done", f"HTTP initial install reaches done (phase={first.get('phase')}; error={first.get('error')!r})")
        if first.get("phase") == "done":
            check_generation(paths, supervisor_env, root, "1.0.0", kernel_port, probe_wheel, base, "generation 1 after staged→active")
        else:
            check_generation(paths, supervisor_env, root, "1.0.0", kernel_port, probe_wheel, base, "generation 1 diagnostic")

        working_dir = paths["share"] / ".qwenpaw"
        try:
            initial_config = json.loads((working_dir / "config.json").read_text())
        except (OSError, json.JSONDecodeError):
            initial_config = None
        check(
            initial_config == {"initialized": True},
            "initial install init creates config.json in QWENPAW_WORKING_DIR",
        )
        check_init_count(working_dir, 1, "initial install")
        config_bytes = (working_dir / "config.json").read_bytes() if (working_dir / "config.json").is_file() else None
        workspace_marker = working_dir / "workspaces/runtime-contract-user-data"
        workspace_marker.parent.mkdir(parents=True, exist_ok=True)
        workspace_marker.write_bytes(b"preserve user workspace data\n")
        user_data = {
            "config.json": config_bytes,
            "workspaces/runtime-contract-user-data": b"preserve user workspace data\n",
        }
        if (paths["home"] / "venv/bin/python3").is_file():
            check_initialization_protocol(paths, supervisor_env, root)
        else:
            check(False, "init protocol probes ran using the active real venv python")

        submit_install(base, "2.0.0", index_url)
        upgraded = wait_install_terminal(base, supervisor, supervisor_log_path)
        check(upgraded.get("phase") == "done", f"HTTP upgrade reaches done (phase={upgraded.get('phase')}; error={upgraded.get('error')!r})")
        if upgraded.get("phase") == "done":
            check_generation(paths, supervisor_env, root, "2.0.0", kernel_port, probe_wheel, base, "generation 2 after staged→active upgrade")
        else:
            check_generation(paths, supervisor_env, root, "2.0.0", kernel_port, probe_wheel, base, "generation 2 diagnostic")
        check_init_count(working_dir, 1, "after upgrade to 2.0.0")
        check_user_data(working_dir, user_data, "after upgrade to 2.0.0")

        submit_install(base, "3.0.0", index_url)
        rolled_back = wait_install_terminal(base, supervisor, supervisor_log_path)
        check(
            rolled_back.get("phase") == "failed"
            and "health_timeout" in str(rolled_back.get("error", "")),
            f"3.0.0 startup failure triggers health-timeout rollback (phase={rolled_back.get('phase')}; error={rolled_back.get('error')!r})",
        )
        check_generation(paths, supervisor_env, root, "2.0.0", kernel_port, probe_wheel, base, "generation 2 after rollback from generation 3")
        check_init_count(working_dir, 1, "after rollback from 3.0.0")
        check_user_data(working_dir, user_data, "after rollback from 3.0.0")
        try:
            kernel_log = (paths["var"] / "qwenpaw.log").read_text(errors="replace")
        except OSError:
            kernel_log = ""
        check(
            "contract: qwenpaw 3.0.0 app startup failure" in kernel_log,
            "3.0.0 test wheel remains installable but fails during app startup",
        )

        with index_server.RequestHandlerClass.requests_lock:
            requests = tuple(index_server.RequestHandlerClass.requests)
        requested_wheels = {Path(path).name for _host, path, _header in requests if path.startswith("/packages/")}
        check(
            all(host == "127.0.0.1" and path.startswith(("/simple/qwenpaw/", "/packages/")) for host, path, _header in requests),
            f"Simple Index observed only loopback index/wheel paths ({len(requests)} requests)",
        )
        for version in VERSIONS:
            filename = qwenpaw_wheels[version].name
            check(filename in requested_wheels, f"local index served requested qwenpaw {version} wheel")
        check(
            hashlib.sha256(BIN.read_bytes()).hexdigest() == supervisor_sha,
            "release supervisor SHA256 remained unchanged during test",
        )
    finally:
        try:
            stop_supervisor(supervisor, supervisor_identity, supervisor_env, supervisor_log_file, supervisor_log_path)
        finally:
            if index_server is not None:
                index_server.shutdown()
                index_server.server_close()
            if index_thread is not None:
                index_thread.join(timeout=5)

    if paths is not None:
        root_resolved = root.resolve()
        escaped_paths = [str(path) for path in paths.values() if not path.resolve().is_relative_to(root_resolved)]
        check(not escaped_paths, f"all isolated TRIM paths resolve under the temporary root (escaped={escaped_paths})")
    if supervisor is not None and kernel_port is not None:
        try:
            closed = wait_for(
                lambda: tcp_port_closed(kernel_port),
                timeout=10,
                description="waiting for the managed kernel port to close after supervisor shutdown",
            )
            check(closed, f"managed kernel port {kernel_port} is closed after supervisor shutdown")
        except TimeoutError as error:
            check(False, f"managed kernel port {kernel_port} closes after supervisor shutdown: {error}")
    print(f"TEMP_ROOT_CLEANUP=scheduled by TemporaryDirectory for {root}", flush=True)


def main():
    root = Path(tempfile.mkdtemp(prefix="qwenpaw-runtime-venv-contract-"))
    try:
        try:
            run_contract(root)
        except Exception as error:
            message = f"runtime venv contract test could not complete: {type(error).__name__}: {error}"
            print(f"[FAIL] {message}", flush=True)
            FAILURES.append(message)
    finally:
        try:
            shutil.rmtree(root)
            print(f"TEMP_ROOT_REMOVED={root}", flush=True)
        except OSError as error:
            message = f"failed to remove isolated temporary root {root}: {error}"
            print(f"[FAIL] {message}", flush=True)
            FAILURES.append(message)

    if FAILURES:
        print(f"RUNTIME VENV CONTRACT FAILED: {len(FAILURES)} assertion(s)", flush=True)
        for failure in FAILURES:
            print(f"  - {failure}", flush=True)
        return 1
    print("RUNTIME VENV CONTRACT PASSED", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
