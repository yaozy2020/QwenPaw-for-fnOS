#!/usr/bin/env python3
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parent.parent
CGI_SOURCE = ROOT / "app/ui/api.cgi"
SUPERVISOR_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_SUP_BIN", ROOT / "supervisor/target/release/qwenpaw-supervisor"
)).resolve()
RUNTIME_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_RUNTIME_BIN", ROOT / "supervisor/target/release/qwenpaw-test-runtime"
)).resolve()
TOKEN = "native_api_cgi_isolated_token_2026_11_04"
CSRF = "native-api-cgi-csrf-token"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
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
        return identity, fields[0], int(fields[1])
    except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError, ValueError, OSError):
        return None


def process_is_alive(identity):
    current = read_process(identity[0])
    return current is not None and current[0] == identity and current[1] not in ("Z", "X")


def signal_owned_process(identity, signum):
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise AssertionError("safe process cleanup requires pidfd_open and pidfd_send_signal")
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


def wait_for_process_exit(identity, timeout=4):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not process_is_alive(identity):
            return
        time.sleep(0.05)
    raise AssertionError(f"owned process remained alive: pid={identity[0]}")


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


def wait_for(predicate, timeout=15, description="condition"):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError(f"timed out waiting for {description}")


def assert_native_cgi_entry(path):
    if not path.is_file() or not os.access(path, os.X_OK):
        raise AssertionError(f"api.cgi must exist and be executable: {path}")
    contents = path.read_bytes()
    if contents.startswith(b"#!"):
        try:
            source = contents.decode("utf-8")
        except UnicodeDecodeError as error:
            raise AssertionError(f"api.cgi is not a valid UTF-8 thin entry: {error}") from error
        active_lines = [line for line in source.splitlines() if not line.lstrip().startswith("#")]
        active_source = "\n".join(active_lines)
        for command in ("curl", "python3"):
            if re.search(rf"\b{command}\b", active_source):
                raise AssertionError(
                    f"api.cgi has not converged to the native CGI bridge; shell still refers to {command}"
                )
        if not re.search(r"\bexec\b", active_source) or "qwenpaw-supervisor" not in active_source:
            raise AssertionError("shell api.cgi must be a thin exec entry to qwenpaw-supervisor")
    elif not contents.startswith(b"\x7fELF"):
        raise AssertionError("api.cgi must be a native executable or a thin executable wrapper")
    return hashlib.sha256(contents).hexdigest()


class CGIResponse:
    def __init__(self, status, headers, body):
        self.status = status
        self.headers = headers
        self.body = body

    def json(self):
        try:
            return json.loads(self.body)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise AssertionError(f"CGI did not return valid JSON: {self.body!r}") from error


def parse_cgi_response(stdout):
    text = stdout.decode("utf-8", errors="replace").replace("\r\n", "\n")
    if "\n\n" not in text:
        raise AssertionError(f"CGI response has no header/body separator: {text!r}")
    raw_headers, body = text.split("\n\n", 1)
    status = 200
    headers = {}
    for line in raw_headers.splitlines():
        if ":" not in line:
            continue
        name, value = line.split(":", 1)
        value = value.strip()
        if name.strip().lower() == "status":
            match = re.match(r"(\d{3})", value)
            if not match:
                raise AssertionError(f"invalid CGI Status header: {line!r}")
            status = int(match.group(1))
        else:
            headers[name.strip().lower()] = value
    return CGIResponse(status, headers, body)


class Fixture:
    def __init__(self, root, binary_sha256, runtime_sha256, cgi_sha256):
        self.root = root
        self.home = root / "home"
        self.var = root / "var"
        self.etc = root / "etc"
        self.share = root / "share"
        self.appdest = root / "target"
        self.tmp = root / "tmp"
        for directory in (
            self.home / "venv/bin",
            self.var,
            self.etc,
            self.share / ".qwenpaw",
            self.share / ".qwenpaw.secret",
            self.appdest / "app/bin",
            self.appdest / "ui",
            self.tmp,
        ):
            directory.mkdir(parents=True, exist_ok=True)

        self.app_port = free_port()
        self.supervisor_port = free_port()
        while self.supervisor_port == self.app_port:
            self.supervisor_port = free_port()
        self.python = self.home / "venv/bin/python3"
        shutil.copy2(RUNTIME_SOURCE, self.python)
        if sha256(self.python) != runtime_sha256:
            raise AssertionError("release test-runtime SHA256 changed while preparing fixture")
        self.binary = self.appdest / "app/bin/qwenpaw-supervisor"
        shutil.copy2(SUPERVISOR_SOURCE, self.binary)
        if sha256(self.binary) != binary_sha256:
            raise AssertionError("release supervisor SHA256 changed while preparing fixture")
        self.cgi = self.appdest / "ui/api.cgi"
        shutil.copy2(CGI_SOURCE, self.cgi)
        if sha256(self.cgi) != cgi_sha256:
            raise AssertionError("api.cgi changed while preparing fixture")
        os.chmod(self.cgi, stat.S_IMODE(CGI_SOURCE.stat().st_mode))

        (self.home / "venv/installed").write_text("1.0.0\n")
        (self.var / "qwenpaw-version").write_text("1.0.0\n")
        (self.var / "service_port").write_text(f"{self.app_port}\n")
        (self.var / "qwenpaw-log-level").write_text("info\n")
        self.token_file = self.var / "supervisor-token"
        self.token_file.write_text(TOKEN)
        os.chmod(self.token_file, 0o600)
        self.mirror_file = self.var / "pip-mirror"
        self.auth_file = self.share / ".qwenpaw.secret/auth.json"
        self.auth_file.write_text("isolated test auth data\n")
        (self.share / ".qwenpaw/config.json").write_text("{}\n")
        self.ui_config = self.appdest / "ui/config"
        self.ui_config.write_text(json.dumps({
            ".url": {
                "qwenpaw.main": {"port": str(self.app_port), "noDisplay": False},
                "qwenpaw.console": {"port": "15666", "type": "iframe"},
            }
        }))

        self.env = dict(os.environ)
        for name in tuple(self.env):
            if name.startswith("HTTP_"):
                self.env.pop(name, None)
        self.env.update({
            "TRIM_PKGHOME": str(self.home),
            "TRIM_PKGVAR": str(self.var),
            "TRIM_PKGETC": str(self.etc),
            "TRIM_DATA_SHARE_PATHS": str(self.share),
            "TRIM_APPDEST": str(self.appdest),
            "TRIM_SERVICE_PORT": str(self.app_port),
            "SUPERVISOR_PORT": str(self.supervisor_port),
            "SUPERVISOR_AUTOSTART": "0",
            "QWENPAW_PYTHON_BIN": str(self.python),
            "QWENPAW_TEST_CONTROL": str(self.root),
            "QWENPAW_SUP_URL": f"http://127.0.0.1:{self.supervisor_port}",
            "TMPDIR": str(self.tmp),
            "REMOTE_ADDR": "127.0.0.1",
            "CONTENT_LENGTH": "0",
        })
        for name in (
            "QWENPAW_CRASH_POINT", "QWENPAW_ENABLE_CRASH_POINT",
            "QWENPAW_TEST_RUNTIME_INSTALL_VERSION",
        ):
            self.env.pop(name, None)
        for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
            self.env.pop(name, None)
        self.env["NO_PROXY"] = "127.0.0.1,localhost"
        self.process = None
        self.process_identity = None
        self.descendants = {}
        self.process_log = None
        self.exit_code = None

    def _request_supervisor_status(self):
        connection = http.client.HTTPConnection("127.0.0.1", self.supervisor_port, timeout=1)
        try:
            connection.request("GET", "/status", headers={"X-Supervisor-Token": TOKEN})
            response = connection.getresponse()
            response.read()
            return response.status == 200
        except OSError:
            return False
        finally:
            connection.close()

    def start(self):
        if self.process is not None:
            raise AssertionError("supervisor fixture already started")
        self.process_log = (self.root / "supervisor-stdout.log").open("ab")
        self.process = subprocess.Popen(
            [str(self.binary)], env=self.env, stdout=self.process_log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )

        def capture_identity():
            if self.process.poll() is not None:
                raise AssertionError((self.root / "supervisor-stdout.log").read_text(errors="replace"))
            info = read_process(self.process.pid)
            if info is None:
                return False
            if Path(info[0][2]) != self.binary:
                raise AssertionError(f"supervisor executable mismatch: {info[0][2]} != {self.binary}")
            self.process_identity = info[0]
            return self._request_supervisor_status()

        wait_for(capture_identity, timeout=15, description="release supervisor startup")
        print(f"PASS: release supervisor ready on isolated port {self.supervisor_port}")

    def remember_descendants(self):
        if self.process is None:
            return
        for identity in process_tree(self.process.pid):
            if identity[0] != self.process.pid:
                self.descendants[identity[0]] = identity

    def stop(self):
        if self.process is None:
            return
        self.remember_descendants()
        expected = self.process_identity
        if expected is None:
            info = read_process(self.process.pid)
            if info is not None and Path(info[0][2]) == self.binary:
                expected = info[0]
                self.process_identity = expected
        if self.process.poll() is None:
            if expected is None:
                raise AssertionError("cannot safely identify the release supervisor for cleanup")
            signal_owned_process(expected, signal.SIGTERM)
            try:
                self.process.wait(timeout=28)
            except subprocess.TimeoutExpired:
                signal_owned_process(expected, signal.SIGKILL)
                self.process.wait(timeout=5)
                raise AssertionError("release supervisor exceeded graceful shutdown budget")
        self.exit_code = self.process.returncode
        self.remember_descendants()
        self.process = None
        self.process_identity = None
        if self.process_log:
            self.process_log.close()
            self.process_log = None
        try:
            self.cleanup_descendants()
        finally:
            if self.exit_code != 0:
                log = (self.root / "supervisor-stdout.log").read_text(errors="replace")
                raise AssertionError(f"release supervisor exited {self.exit_code}:\n{log}")

    def cleanup_descendants(self):
        for identity in self.descendants.values():
            if process_is_alive(identity):
                signal_owned_process(identity, signal.SIGTERM)
                try:
                    wait_for_process_exit(identity, timeout=2)
                except AssertionError:
                    signal_owned_process(identity, signal.SIGKILL)
                    wait_for_process_exit(identity, timeout=3)

    def run_cgi(self, action, method="GET", params=None, *, csrf=True,
                cookie_token=None, header_token=None, query_csrf=False,
                cookie_header=None, https=False):
        values = [("action", action)]
        for key, value in (params or {}).items():
            if value is not None:
                values.append((key, str(value)))
        if method == "POST" and query_csrf:
            values.append(("_csrf", CSRF))
        env = dict(self.env)
        env.update({
            "QUERY_STRING": urlencode(values),
            "REQUEST_METHOD": method,
            "HTTPS": "on" if https else "off",
            "REQUEST_SCHEME": "https" if https else "http",
        })
        env.pop("HTTP_COOKIE", None)
        env.pop("HTTP_X_CSRF_TOKEN", None)
        if cookie_header is not None:
            env["HTTP_COOKIE"] = cookie_header
        if method == "POST" and csrf:
            cookie_value = CSRF if cookie_token is None else cookie_token
            header_value = CSRF if header_token is None else header_token
            if cookie_value is not None and cookie_header is None:
                env["HTTP_COOKIE"] = f"session=isolated; qwenpaw_csrf={cookie_value}"
            if header_value is not None:
                env["HTTP_X_CSRF_TOKEN"] = header_value
        process = subprocess.Popen(
            [str(self.cgi)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=45)
        except subprocess.TimeoutExpired:
            identities = process_tree(process.pid)
            for identity in reversed(identities):
                if identity[0] != process.pid and process_is_alive(identity):
                    signal_owned_process(identity, signal.SIGTERM)
            own_identity = read_process(process.pid)
            if own_identity is not None:
                signal_owned_process(own_identity[0], signal.SIGTERM)
            try:
                stdout, stderr = process.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                for identity in reversed(identities):
                    if process_is_alive(identity):
                        signal_owned_process(identity, signal.SIGKILL)
                stdout, stderr = process.communicate(timeout=3)
            raise AssertionError(
                f"api.cgi timed out for action={action!r}; stderr={stderr.decode(errors='replace')}"
            )
        if process.returncode != 0:
            raise AssertionError(
                f"api.cgi exited {process.returncode} for action={action!r}: "
                f"stdout={stdout.decode(errors='replace')!r}, stderr={stderr.decode(errors='replace')!r}"
            )
        return parse_cgi_response(stdout)

    def status(self):
        response = self.run_cgi("status")
        if response.status != 200:
            raise AssertionError(f"status action returned HTTP {response.status}: {response.body}")
        result = response.json()
        if not isinstance(result, dict) or result.get("success") is not True:
            raise AssertionError(f"status action returned an unexpected response: {result!r}")
        return result

    def wait_for_kernel(self, running, *, port=None, version=None):
        def ready():
            state = self.status()
            if state.get("running") is not running:
                return False
            if port is not None and state.get("port") != port:
                return False
            if version is not None and state.get("version") != version:
                return False
            return state

        return wait_for(ready, timeout=20, description=f"kernel running={running}, port={port}, version={version}")

    def assert_kernel_health(self, port, version):
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
        try:
            connection.request("GET", "/api/version")
            response = connection.getresponse()
            payload = response.read()
            if response.status != 200:
                raise AssertionError(f"test-runtime health returned {response.status}: {payload!r}")
            result = json.loads(payload)
            if result.get("version") != version:
                raise AssertionError(f"test-runtime version mismatch: {result!r}")
        finally:
            connection.close()

    def mirror_snapshot(self):
        try:
            info = self.mirror_file.stat()
            return (
                True, self.mirror_file.read_bytes(), stat.S_IMODE(info.st_mode),
                info.st_ino, info.st_mtime_ns,
            )
        except FileNotFoundError:
            return (False, None, None, None, None)


class LocalIndexHandler(http.server.BaseHTTPRequestHandler):
    body = (
        "<html><body>"
        "<a href='qwenpaw-2.2.0.tar.gz'>stable</a>"
        "<a href='qwenpaw-2.1.0b1-py3-none-any.whl'>beta</a>"
        "<a href='other-package-90.0.0.tar.gz'>other</a>"
        "</body></html>"
    ).encode()
    observed = []

    def do_GET(self):
        type(self).observed.append((self.command, self.path))
        if self.path != "/simple/qwenpaw/":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(self.body)))
        self.end_headers()
        self.wfile.write(self.body)

    def log_message(self, *_args):
        pass


class LocalIndex:
    def __enter__(self):
        LocalIndexHandler.observed = []
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), LocalIndexHandler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        return f"http://127.0.0.1:{self.server.server_address[1]}/simple"

    def __exit__(self, *_args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)


class StubHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.requests.append((self.command, self.path, self.headers.get("X-Supervisor-Token")))
        self.send_response(self.server.status_code)
        self.send_header("Content-Type", self.server.content_type)
        self.send_header("Content-Length", str(len(self.server.payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(self.server.payload)

    def log_message(self, *_args):
        pass


class StubUpstream:
    def __init__(self, port, status_code, payload, content_type="application/json"):
        self.port = port
        self.status_code = status_code
        self.payload = payload.encode("utf-8")
        self.content_type = content_type

    def __enter__(self):
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", self.port), StubHandler)
        self.server.daemon_threads = True
        self.server.status_code = self.status_code
        self.server.payload = self.payload
        self.server.content_type = self.content_type
        self.server.requests = []
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        return self

    def __exit__(self, *_args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)


def assert_success(response, action):
    if response.status != 200:
        raise AssertionError(f"{action} returned HTTP {response.status}: {response.body}")
    result = response.json()
    if not isinstance(result, dict) or result.get("success") is not True:
        raise AssertionError(f"{action} did not report success: {result!r}")
    return result


def test_auth_csrf_and_routing(fixture):
    initial = fixture.status()
    assert initial["installed"] and initial["running"] is False, initial
    assert fixture.process is not None
    fixture.remember_descendants()
    assert not (fixture.var / "kernel.pid").exists()

    original_token = fixture.token_file.read_bytes()
    original_mode = stat.S_IMODE(fixture.token_file.stat().st_mode)
    try:
        fixture.token_file.write_text("wrong_isolated_supervisor_token_2026\n")
        os.chmod(fixture.token_file, 0o600)
        unauthorized = fixture.run_cgi("status")
        assert unauthorized.status == 401, (unauthorized.status, unauthorized.body)
        assert unauthorized.json().get("success") is False
    finally:
        fixture.token_file.write_bytes(original_token)
        os.chmod(fixture.token_file, original_mode)
    assert fixture.status()["running"] is False

    missing_cookie = fixture.run_cgi("start", method="POST", csrf=False)
    assert missing_cookie.status == 403, (missing_cookie.status, missing_cookie.body)
    mismatch = fixture.run_cgi(
        "set_mirror", method="POST", params={"mirror": "https://mirror.example/simple"},
        csrf=True, cookie_token="cookie-token", header_token="different-token",
    )
    assert mismatch.status == 403, (mismatch.status, mismatch.body)
    assert fixture.mirror_snapshot() == (False, None, None, None, None)
    missing_header = fixture.run_cgi(
        "set_mirror", method="POST", params={"mirror": "https://mirror.example/simple"},
        csrf=True, cookie_token=CSRF, header_token="",
    )
    assert missing_header.status == 403, (missing_header.status, missing_header.body)
    assert fixture.mirror_snapshot() == (False, None, None, None, None)

    https_legacy = fixture.run_cgi(
        "set_mirror", method="POST", params={"mirror": "https://mirror.example/simple"},
        https=True, cookie_token=CSRF, header_token=CSRF,
    )
    assert https_legacy.status == 403, (https_legacy.status, https_legacy.body)
    duplicate_https_cookie = fixture.run_cgi(
        "set_mirror", method="POST", params={"mirror": "https://mirror.example/simple"},
        https=True,
        cookie_header=(
            f"session=isolated; __Host-qwenpaw_csrf={CSRF}; "
            f"__Host-qwenpaw_csrf={CSRF}"
        ),
        header_token=CSRF,
    )
    assert duplicate_https_cookie.status == 403, (
        duplicate_https_cookie.status, duplicate_https_cookie.body
    )
    duplicate_http_cookie = fixture.run_cgi(
        "set_mirror", method="POST", params={"mirror": "https://mirror.example/simple"},
        cookie_header=f"session=isolated; qwenpaw_csrf={CSRF}; qwenpaw_csrf={CSRF}",
        header_token=CSRF,
    )
    assert duplicate_http_cookie.status == 403, (
        duplicate_http_cookie.status, duplicate_http_cookie.body
    )
    assert fixture.mirror_snapshot() == (False, None, None, None, None)

    query_log = fixture.var / "qwenpaw.log"
    query_log.write_text("must be cleared with HTTP legacy cookie and CSRF header\n")
    host_header = fixture.run_cgi(
        "clear_logs", method="POST", https=True,
        cookie_header=f"session=isolated; __Host-qwenpaw_csrf={CSRF}",
        header_token=CSRF,
    )
    assert_success(host_header, "clear_logs with HTTPS host-only cookie and CSRF header")
    assert query_log.read_bytes() == b""
    query_log.write_text("must be cleared with HTTP legacy cookie and CSRF header\n")
    http_legacy_header = fixture.run_cgi("clear_logs", method="POST")
    assert_success(http_legacy_header, "clear_logs with HTTP legacy cookie and CSRF header")
    assert query_log.read_bytes() == b""

    query_log.write_text("must be cleared with query CSRF fallback\n")
    query_fallback = fixture.run_cgi(
        "clear_logs", method="POST", csrf=True, header_token=None, query_csrf=True,
    )
    assert_success(query_fallback, "clear_logs with query CSRF fallback")
    assert query_log.read_bytes() == b""
    assert fixture.mirror_snapshot() == (False, None, None, None, None)
    assert not (fixture.var / "kernel.pid").exists()
    assert fixture.status()["running"] is False

    unknown = fixture.run_cgi("not_a_legacy_action")
    assert unknown.status == 404 and unknown.json().get("success") is False
    wrong_post = fixture.run_cgi("status", method="POST")
    assert wrong_post.status == 405, (wrong_post.status, wrong_post.body)
    assert "GET" in wrong_post.headers.get("allow", "")
    wrong_get = fixture.run_cgi("start", method="GET")
    assert wrong_get.status == 405, (wrong_get.status, wrong_get.body)
    assert "POST" in wrong_get.headers.get("allow", "")
    head = fixture.run_cgi("status", method="HEAD")
    assert head.status == 200 and head.json().get("success") is True
    assert fixture.status()["running"] is False
    print("PASS: CGI supervisor-token authorization, double-submit CSRF, HEAD, 404/405 and zero rejected writes")


def test_fake_upstream_contracts(fixture):
    fixture.stop()
    status_body = '{"success":false,"message":"upstream status sentinel"}'
    with StubUpstream(fixture.supervisor_port, 418, status_body) as upstream:
        response = fixture.run_cgi("status")
        assert response.status == 418, (response.status, response.body)
        assert response.body.strip() == status_body
        assert upstream.server.requests == [("GET", "/status", TOKEN)], upstream.server.requests

    with StubUpstream(fixture.supervisor_port, 200, "not-json from isolated upstream", "text/plain") as upstream:
        response = fixture.run_cgi("status")
        assert response.status == 200
        result = response.json()
        assert result.get("success") is False, result
        assert "not-json from isolated upstream" not in response.body
        assert upstream.server.requests == [("GET", "/status", TOKEN)], upstream.server.requests

    response = fixture.run_cgi("status")
    assert response.status == 502, (response.status, response.body)
    assert response.json().get("success") is False
    fixture.start()
    print("PASS: non-JSON fallback, upstream HTTP state passthrough, stopped supervisor → 502")


def test_legacy_actions(fixture):
    get_status = fixture.run_cgi("status")
    status = assert_success(get_status, "status")
    assert status["running"] is False and status["version"] == "1.0.0", status
    get_logs = fixture.run_cgi("logs")
    logs = assert_success(get_logs, "logs")
    assert isinstance(logs.get("logs"), list), logs
    install_status = assert_success(fixture.run_cgi("upgrade_status"), "upgrade_status")
    assert install_status.get("upgrading") is False, install_status
    install_logs = assert_success(fixture.run_cgi("upgrade_logs"), "upgrade_logs")
    assert isinstance(install_logs.get("logs"), list), install_logs

    with LocalIndex() as index_url:
        set_mirror = fixture.run_cgi(
            "set_mirror", method="POST", params={"mirror": index_url}, query_csrf=True
        )
        assert_success(set_mirror, "set_mirror")
        mirror_bytes = (index_url + "\n").encode()
        assert fixture.mirror_file.read_bytes() == mirror_bytes
        mirror_metadata = fixture.mirror_file.stat()
        mirror_before = fixture.mirror_snapshot()

        malicious_mirror = f"https://mirror.example/simple;touch {fixture.root / 'unexpected-write'}"
        invalid = fixture.run_cgi(
            "set_mirror", method="POST", params={"mirror": malicious_mirror}
        )
        invalid_result = invalid.json()
        assert invalid.status == 200 and invalid_result.get("success") is False, (
            invalid.status, invalid_result
        )
        assert fixture.mirror_snapshot() == mirror_before, "invalid mirror changed pip-mirror bytes or metadata"
        assert not (fixture.root / "unexpected-write").exists(), "invalid mirror escaped its input boundary"

        get_mirror = assert_success(fixture.run_cgi("get_mirror"), "get_mirror")
        assert get_mirror.get("mirror") == index_url, get_mirror

        versions_response = fixture.run_cgi("version_list")
        versions = assert_success(versions_response, "version_list")
        version_names = [item.get("version") for item in versions.get("versions", [])]
        assert versions.get("latest_stable") == "2.2.0", versions
        assert versions.get("latest_beta") == "2.1.0b1", versions
        assert version_names[:2] == ["2.2.0", "2.1.0b1"], versions
        assert any(item.get("version") == "2.1.0b1" and item.get("is_beta") is True for item in versions["versions"])
        assert LocalIndexHandler.observed == [("GET", "/simple/qwenpaw/")], LocalIndexHandler.observed

        original_mirror = fixture.mirror_file.read_bytes()
        try:
            fixture.mirror_file.write_text("https://mirror.example/simple;bad\n")
            failed_versions = fixture.run_cgi("version_list")
            assert failed_versions.status == 502, (failed_versions.status, failed_versions.body)
            assert failed_versions.json().get("success") is False
        finally:
            fixture.mirror_file.write_bytes(original_mirror)
            os.chmod(fixture.mirror_file, stat.S_IMODE(mirror_metadata.st_mode))

    started = assert_success(fixture.run_cgi("start", method="POST"), "start")
    status = fixture.wait_for_kernel(True, port=fixture.app_port)
    fixture.assert_kernel_health(fixture.app_port, "1.0.0")

    upgrade = assert_success(
        fixture.run_cgi("upgrade", method="POST", params={"version": "2.0.0"}),
        "upgrade",
    )
    assert isinstance(upgrade.get("seq"), int) and upgrade.get("instance"), upgrade

    def install_finished():
        result = fixture.run_cgi("upgrade_status")
        if result.status != 200:
            raise AssertionError(f"upgrade_status returned HTTP {result.status}: {result.body}")
        value = result.json()
        if value.get("phase") == "failed":
            raise AssertionError(f"test-runtime install failed: {value!r}")
        return value if value.get("upgrading") is False and value.get("phase") == "done" else False

    final_install = wait_for(install_finished, timeout=30, description="native upgrade action to complete")
    assert final_install.get("seq") == upgrade["seq"], (upgrade, final_install)
    status = fixture.wait_for_kernel(True, port=fixture.app_port, version="2.0.0")
    fixture.assert_kernel_health(fixture.app_port, "2.0.0")
    assert_success(fixture.run_cgi("upgrade_logs"), "upgrade_logs after upgrade")

    new_port = free_port()
    while new_port == fixture.supervisor_port:
        new_port = free_port()
    changed = assert_success(
        fixture.run_cgi("change_port", method="POST", params={"port": new_port}),
        "change_port",
    )
    assert changed.get("port") == new_port, changed
    status = fixture.wait_for_kernel(True, port=new_port, version="2.0.0")
    fixture.assert_kernel_health(new_port, "2.0.0")
    fixture.app_port = new_port

    clear_logs = assert_success(
        fixture.run_cgi("clear_logs", method="POST", query_csrf=True), "clear_logs"
    )
    assert (fixture.var / "qwenpaw.log").read_bytes() == b""

    reset_auth = assert_success(
        fixture.run_cgi("reset_auth", method="POST"), "reset_auth"
    )
    assert not fixture.auth_file.exists(), (reset_auth, "auth.json remains after reset")
    assert not (fixture.auth_file.parent / "auth.json.bak-reset").exists()
    status = fixture.wait_for_kernel(True, port=new_port, version="2.0.0")
    fixture.assert_kernel_health(new_port, "2.0.0")

    restart = assert_success(fixture.run_cgi("restart", method="POST"), "restart")
    status = fixture.wait_for_kernel(True, port=new_port, version="2.0.0")
    fixture.assert_kernel_health(new_port, "2.0.0")

    stop = assert_success(fixture.run_cgi("stop", method="POST"), "stop")
    fixture.wait_for_kernel(False, port=new_port)

    start_again = assert_success(fixture.run_cgi("start", method="POST"), "start after stop")
    status = fixture.wait_for_kernel(True, port=new_port, version="2.0.0")
    fixture.assert_kernel_health(new_port, "2.0.0")

    fixture.auth_file.write_text("must survive kernel-only uninstall\n")
    uninstall = assert_success(
        fixture.run_cgi("uninstall_kernel", method="POST"), "uninstall_kernel"
    )
    assert not (fixture.home / "venv").exists(), "uninstall_kernel left the isolated venv"
    assert not (fixture.var / "qwenpaw-version").exists(), "uninstall_kernel left the version pin"
    assert fixture.auth_file.read_text() == "must survive kernel-only uninstall\n"
    assert (fixture.share / ".qwenpaw/config.json").read_text() == "{}\n"
    after_uninstall = fixture.status()
    assert after_uninstall["installed"] is False and after_uninstall["running"] is False, after_uninstall

    passed = {
        "GET": ("status", "logs", "version_list", "get_mirror", "upgrade_status", "upgrade_logs"),
        "POST": ("start", "stop", "restart", "clear_logs", "upgrade", "uninstall_kernel", "reset_auth", "change_port", "set_mirror"),
    }
    assert len(passed["GET"]) == 6 and len(passed["POST"]) == 9
    print("PASS: all 15 legacy actions succeed through the real api.cgi with release supervisor/test-runtime")


def main():
    for executable, label in ((SUPERVISOR_SOURCE, "release supervisor"), (RUNTIME_SOURCE, "release qwenpaw-test-runtime")):
        if not executable.is_file() or not os.access(executable, os.X_OK):
            raise SystemExit(f"build release {label} before running this test: {executable}")
    cgi_sha256 = assert_native_cgi_entry(CGI_SOURCE)
    supervisor_sha256 = sha256(SUPERVISOR_SOURCE)
    runtime_sha256 = sha256(RUNTIME_SOURCE)
    print(f"TEST_SUPERVISOR={SUPERVISOR_SOURCE}")
    print(f"TEST_SUPERVISOR_SHA256={supervisor_sha256}")
    print(f"TEST_RUNTIME={RUNTIME_SOURCE}")
    print(f"TEST_RUNTIME_SHA256={runtime_sha256}")
    print(f"TEST_API_CGI={CGI_SOURCE}")
    print(f"TEST_API_CGI_SHA256={cgi_sha256}")

    with tempfile.TemporaryDirectory(prefix="qwenpaw-native-api-cgi-") as directory:
        root = Path(directory).resolve()
        if str(root).startswith("/var/apps/qwenpaw"):
            raise AssertionError(f"refusing to use a real deployment path as test root: {root}")
        fixture = Fixture(root, supervisor_sha256, runtime_sha256, cgi_sha256)
        try:
            fixture.start()
            test_auth_csrf_and_routing(fixture)
            test_fake_upstream_contracts(fixture)
            test_legacy_actions(fixture)
            fixture.remember_descendants()
            current_supervisor = sha256(SUPERVISOR_SOURCE)
            current_runtime = sha256(RUNTIME_SOURCE)
            current_cgi = sha256(CGI_SOURCE)
            assert current_supervisor == supervisor_sha256, "release supervisor changed during test"
            assert current_runtime == runtime_sha256, "release test-runtime changed during test"
            assert current_cgi == cgi_sha256, "api.cgi changed during test"
        finally:
            fixture.stop()
    print("PASS: isolated native API CGI integration suite completed; no real service/data paths were used")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
