#!/usr/bin/env python3
import hashlib
import http.server
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
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parent.parent
SUPERVISOR_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_SUP_BIN", ROOT / "supervisor/target/release/qwenpaw-supervisor"
)).resolve()
RUNTIME_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_RUNTIME_BIN", ROOT / "supervisor/target/release/qwenpaw-test-runtime"
)).resolve()
INDEX_SOURCE = ROOT / "app/ui/index.cgi"
API_SOURCE = ROOT / "app/ui/api.cgi"
WEB_SOURCE = ROOT / "web/dist/index.html"
TOKEN = "isolated-browser-cgi-supervisor-token-2026"
INDEX_BODY = (
    "<html><body>"
    "<a href='qwenpaw-2.0.0.tar.gz'>stable</a>"
    "<a href='qwenpaw-2.1.0b1-py3-none-any.whl'>beta</a>"
    "<a href='unrelated-99.0.0.tar.gz'>unrelated</a>"
    "</body></html>"
).encode()
MIRROR_SOURCES = {
    "aliyun": "https://mirrors.aliyun.com/pypi/simple/",
    "tsinghua": "https://pypi.tuna.tsinghua.edu.cn/simple",
    "ustc": "https://pypi.mirrors.ustc.edu.cn/simple/",
    "tencent": "https://mirrors.cloud.tencent.com/pypi/simple",
}


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
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
        raise RuntimeError("safe fixture cleanup requires pidfd_open and pidfd_send_signal")
    current = read_process(identity[0])
    if current is None or current[1] in ("Z", "X"):
        return False
    if current[0] != identity:
        raise RuntimeError(f"refusing to signal a process whose identity changed: pid={identity[0]}")
    try:
        pidfd = os.pidfd_open(identity[0])
    except ProcessLookupError:
        return False
    try:
        current = read_process(identity[0])
        if current is None or current[1] in ("Z", "X"):
            return False
        if current[0] != identity:
            raise RuntimeError(f"refusing to signal a reused pid: pid={identity[0]}")
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
    raise RuntimeError(f"owned fixture process remained alive: pid={identity[0]}")


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


def parse_cgi(stdout):
    separator = b"\r\n\r\n"
    offset = stdout.find(separator)
    width = len(separator)
    if offset < 0:
        separator = b"\n\n"
        offset = stdout.find(separator)
        width = len(separator)
    if offset < 0:
        raise RuntimeError(f"CGI response has no header/body separator: {stdout[:512]!r}")

    headers = []
    status = 200
    for line in stdout[:offset].replace(b"\r\n", b"\n").split(b"\n"):
        if not line or b":" not in line:
            continue
        name, value = line.split(b":", 1)
        name = name.decode("ascii").strip()
        value = value.decode("latin-1").strip()
        if name.lower() == "status":
            status = int(value[:3])
        else:
            headers.append((name, value))
    return status, headers, stdout[offset + width:]


class Fixture:
    def __init__(self, port):
        self.temporary_directory = tempfile.TemporaryDirectory(prefix="qwenpaw-browser-cgi-")
        self.root = Path(self.temporary_directory.name).resolve()
        if self.root.is_relative_to(Path("/var/apps/qwenpaw")):
            raise RuntimeError(f"refusing to use a real deployment path: {self.root}")
        self.port = port
        self.supervisor_port = free_port()
        while self.supervisor_port == port:
            self.supervisor_port = free_port()
        self.app_port = free_port()
        while self.app_port in (port, self.supervisor_port):
            self.app_port = free_port()
        self.home = self.root / "home"
        self.var = self.root / "var"
        self.etc = self.root / "etc"
        self.share_root = self.root / "shares/qwenpaw"
        self.share = self.share_root / ".qwenpaw"
        self.share_secret = self.share_root / ".qwenpaw.secret"
        self.appdest = self.root / "target"
        self.tmp = self.root / "tmp"
        for path in (
            self.home,
            self.var,
            self.etc,
            self.share,
            self.share_secret,
            self.appdest / "app/bin",
            self.appdest / "ui",
            self.appdest / "www",
            self.tmp,
        ):
            path.mkdir(parents=True, exist_ok=True)

        self.binary = self.appdest / "app/bin/qwenpaw-supervisor"
        self.runtime = self.home / "venv/bin/python3"
        self.base_python = self.root / "runtime-source"
        shutil.copy2(SUPERVISOR_SOURCE, self.binary)
        shutil.copy2(RUNTIME_SOURCE, self.base_python)
        shutil.copy2(INDEX_SOURCE, self.appdest / "ui/index.cgi")
        shutil.copy2(API_SOURCE, self.appdest / "ui/api.cgi")
        os.chmod(self.appdest / "ui/index.cgi", 0o755)
        os.chmod(self.appdest / "ui/api.cgi", 0o755)
        self.binary_sha256 = sha256(SUPERVISOR_SOURCE)
        self.runtime_sha256 = sha256(RUNTIME_SOURCE)
        self.index_sha256 = sha256(INDEX_SOURCE)
        self.api_sha256 = sha256(API_SOURCE)
        self.web_sha256 = sha256(WEB_SOURCE)
        self.source_hashes = {
            SUPERVISOR_SOURCE: self.binary_sha256,
            RUNTIME_SOURCE: self.runtime_sha256,
            INDEX_SOURCE: self.index_sha256,
            API_SOURCE: self.api_sha256,
            WEB_SOURCE: self.web_sha256,
        }
        if (
            sha256(self.binary) != self.binary_sha256
            or sha256(self.base_python) != self.runtime_sha256
            or sha256(self.appdest / "ui/index.cgi") != self.index_sha256
            or sha256(self.appdest / "ui/api.cgi") != self.api_sha256
        ):
            raise RuntimeError("test input changed while preparing isolated browser fixture")

        index_url = f"http://127.0.0.1:{self.port}/mirror/"
        html = WEB_SOURCE.read_text(encoding="utf-8")
        for name, source in MIRROR_SOURCES.items():
            replacement = index_url + name
            if source not in html:
                raise RuntimeError(f"built console does not contain expected mirror URL: {source}")
            html = html.replace(source, replacement)
        (self.appdest / "www/index.html").write_text(html, encoding="utf-8")
        (self.var / "service_port").write_text(f"{self.app_port}\n")
        (self.var / "qwenpaw-log-level").write_text("info\n")
        self.token_file = self.var / "supervisor-token"
        self.token_file.write_text(TOKEN)
        os.chmod(self.token_file, 0o600)
        self.mirror_file = self.var / "pip-mirror"
        self.mirror_file.write_text(f"{index_url}aliyun\n")
        (self.var / "qwenpaw.log").write_text("browser-e2e-runtime-log\n")
        self.user_config = self.share / "config.json"
        self.user_config.write_text('{"fixture":"preserve-user-data"}\n')
        self.auth_file = self.share_secret / "auth.json"
        self.auth_file.write_text("isolated-browser-auth-must-survive\n")
        (self.appdest / "ui/config").write_text(json.dumps({
            ".url": {
                "qwenpaw.main": {"port": str(self.app_port), "noDisplay": False},
                "qwenpaw.console": {"port": "15666", "type": "iframe"},
            }
        }))
        self.process = None
        self.process_identity = None
        self.descendants = {}
        self.lose_upgrade_receipt = False
        self.upgrade_posts = []
        self.upgrade_statuses = []
        self.lock = threading.Lock()
        self.log = (self.root / "supervisor.log.console").open("ab")
        self.env = os.environ.copy()
        for name in tuple(self.env):
            if name.startswith("HTTP_") or name in (
                "HTTPS", "REQUEST_SCHEME", "QUERY_STRING", "REQUEST_URI", "SCRIPT_NAME",
                "PATH_INFO", "REQUEST_METHOD", "CONTENT_LENGTH", "CONTENT_TYPE",
                "QWENPAW_CRASH_POINT", "QWENPAW_ENABLE_CRASH_POINT",
                "QWENPAW_TEST_RUNTIME_INSTALL_VERSION",
            ):
                self.env.pop(name, None)
        for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
            self.env.pop(name, None)
        self.env.update({
            "TRIM_PKGHOME": str(self.home),
            "TRIM_PKGVAR": str(self.var),
            "TRIM_PKGETC": str(self.etc),
            "TRIM_DATA_SHARE_PATHS": str(self.share_root),
            "TRIM_APPDEST": str(self.appdest),
            "TRIM_SERVICE_PORT": str(self.app_port),
            "SUPERVISOR_PORT": str(self.supervisor_port),
            "SUPERVISOR_AUTOSTART": "0",
            "QWENPAW_PYTHON_BIN": str(self.base_python),
            "QWENPAW_TEST_CONTROL": str(self.root),
            "TMPDIR": str(self.tmp),
            "PATH": "/usr/bin:/bin",
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
        })
        self.supervisor_port = int(self.env["SUPERVISOR_PORT"])
        if self.supervisor_port == self.app_port:
            raise RuntimeError("isolated application and supervisor ports unexpectedly match")

    def start(self):
        self.process = subprocess.Popen(
            [str(self.binary)], cwd=self.root, env=self.env,
            stdout=self.log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"release supervisor exited {self.process.returncode}: {self.read_log()}")
            info = read_process(self.process.pid)
            if info is not None:
                if Path(info[0][2]) != self.binary:
                    raise RuntimeError(f"supervisor executable mismatch: {info[0][2]} != {self.binary}")
                self.process_identity = info[0]
            try:
                import urllib.request
                req = urllib.request.Request(
                    f"http://127.0.0.1:{self.supervisor_port}/status",
                    headers={"X-Supervisor-Token": TOKEN},
                )
                with urllib.request.urlopen(req, timeout=1) as response:
                    result = json.loads(response.read())
                if result.get("success") is True and result.get("installed") is False:
                    if self.process_identity is None:
                        raise RuntimeError("supervisor readiness observed without recording process identity")
                    return
            except (OSError, ValueError, json.JSONDecodeError):
                pass
            time.sleep(0.05)
        raise RuntimeError(f"release supervisor did not become ready: {self.read_log()}")

    def remember_descendants(self):
        if self.process is None or self.process_identity is None:
            return
        if not process_is_alive(self.process_identity):
            return
        for identity in process_tree(self.process.pid):
            if identity[0] != self.process.pid:
                self.descendants[identity[0]] = identity

    def cleanup_descendants(self):
        for identity in self.descendants.values():
            if not process_is_alive(identity):
                continue
            signal_owned_process(identity, signal.SIGTERM)
            try:
                wait_for_process_exit(identity, timeout=2)
            except RuntimeError:
                signal_owned_process(identity, signal.SIGKILL)
                wait_for_process_exit(identity, timeout=3)

    def read_log(self):
        self.log.flush()
        return (self.root / "supervisor.log.console").read_text(errors="replace")

    def metadata(self):
        return {
            "appPort": self.app_port,
            "binaries": {
                "apiCgi": {"path": str(API_SOURCE), "sha256": self.api_sha256},
                "indexCgi": {"path": str(INDEX_SOURCE), "sha256": self.index_sha256},
                "runtime": {"path": str(RUNTIME_SOURCE), "sha256": self.runtime_sha256},
                "supervisor": {"path": str(SUPERVISOR_SOURCE), "sha256": self.binary_sha256},
                "webConsole": {"path": str(WEB_SOURCE), "sha256": self.web_sha256},
            },
            "mirrors": {
                "aliyun": f"http://127.0.0.1:{self.port}/mirror/aliyun",
                "tsinghua": f"http://127.0.0.1:{self.port}/mirror/tsinghua",
            },
            "files": {
                "auth": str(self.auth_file),
                "config": str(self.user_config),
                "installLog": str(self.var / "qwenpaw-upgrade.log.pip"),
                "kernelLog": str(self.var / "qwenpaw.log"),
                "mirror": str(self.mirror_file),
                "pin": str(self.var / "qwenpaw-version"),
                "servicePort": str(self.var / "service_port"),
                "uiConfig": str(self.appdest / "ui/config"),
                "venv": str(self.home / "venv"),
            },
        }

    def stop(self):
        errors = []
        if self.process is not None:
            self.remember_descendants()
            if self.process.poll() is None:
                if self.process_identity is None:
                    info = read_process(self.process.pid)
                    if info is not None and Path(info[0][2]) == self.binary:
                        self.process_identity = info[0]
                if self.process_identity is None:
                    raise RuntimeError("cannot safely identify the release supervisor for cleanup")
                signal_owned_process(self.process_identity, signal.SIGTERM)
                try:
                    self.process.wait(timeout=29)
                except subprocess.TimeoutExpired:
                    signal_owned_process(self.process_identity, signal.SIGKILL)
                    self.process.wait(timeout=5)
                    raise RuntimeError("release supervisor exceeded graceful shutdown budget")
            code = self.process.wait(timeout=1)
            if self.process_identity is not None and process_is_alive(self.process_identity):
                raise RuntimeError("release supervisor identity remains alive after wait")
            log = self.read_log() if code != 0 else ""
            self.process = None
            self.cleanup_descendants()
            if code != 0:
                errors.append(f"release supervisor exited {code}: {log}")

        for path, expected in self.source_hashes.items():
            if not path.is_file() or sha256(path) != expected:
                errors.append(f"test input changed during browser run: {path}")

        runtime_ports = {self.app_port}
        try:
            runtime_ports.add(int((self.var / "service_port").read_text().strip()))
        except (OSError, ValueError):
            errors.append("could not verify the final isolated test-runtime port")
        ports = [(self.supervisor_port, "supervisor")]
        ports.extend((port, "test-runtime") for port in runtime_ports)
        for port, label in ports:
            try:
                connection = socket.create_connection(("127.0.0.1", port), timeout=0.25)
            except OSError:
                continue
            connection.close()
            errors.append(f"isolated {label} endpoint remains reachable on port {port}")

        if not self.log.closed:
            self.log.close()
        self.temporary_directory.cleanup()
        if errors:
            raise RuntimeError("; ".join(errors))


class BrowserHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    @property
    def fixture(self):
        return self.server.fixture

    def do_GET(self):
        parsed = urlsplit(self.path)
        if parsed.path == "/favicon.ico":
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            return
        if parsed.path == "/__e2e/meta":
            self.send_json(200, self.fixture.metadata())
            return
        if parsed.path == "/__e2e/observations":
            with self.fixture.lock:
                observations = {
                    "indexRequests": list(self.server.index_requests),
                    "upgradePosts": [dict(item) for item in self.fixture.upgrade_posts],
                    "upgradeStatuses": [dict(item) for item in self.fixture.upgrade_statuses],
                }
            self.send_json(200, observations)
            return
        if parsed.path == "/__e2e/allocate-port":
            self.send_json(200, {"port": free_port()})
            return
        if parsed.path == "/__e2e/lose-next-upgrade-receipt":
            self.send_error(405)
            return
        if parsed.path.startswith("/mirror/"):
            self.serve_index(parsed.path)
            return
        if parsed.path.startswith("/cgi/ThirdParty/qwenpaw/index.cgi"):
            self.run_cgi(self.fixture.appdest / "ui/index.cgi", "GET", parsed)
            return
        if parsed.path == "/cgi/ThirdParty/qwenpaw/api.cgi":
            self.run_cgi(self.fixture.appdest / "ui/api.cgi", "GET", parsed)
            return
        self.send_error(404)

    def do_HEAD(self):
        parsed = urlsplit(self.path)
        if parsed.path.startswith("/cgi/ThirdParty/qwenpaw/index.cgi"):
            self.run_cgi(self.fixture.appdest / "ui/index.cgi", "HEAD", parsed)
        elif parsed.path == "/cgi/ThirdParty/qwenpaw/api.cgi":
            self.run_cgi(self.fixture.appdest / "ui/api.cgi", "HEAD", parsed)
        else:
            self.send_error(404)

    def do_POST(self):
        parsed = urlsplit(self.path)
        if parsed.path == "/__e2e/lose-next-upgrade-receipt":
            with self.fixture.lock:
                self.fixture.lose_upgrade_receipt = True
            self.send_json(200, {"armed": True})
            return
        if parsed.path != "/cgi/ThirdParty/qwenpaw/api.cgi":
            self.send_error(404)
            return
        self.run_cgi(self.fixture.appdest / "ui/api.cgi", "POST", parsed)

    def serve_index(self, path):
        if path.endswith("/qwenpaw/") and path.count("/") == 4:
            with self.fixture.lock:
                self.server.index_requests.append(path)
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(INDEX_BODY)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(INDEX_BODY)
            return
        self.send_error(404)

    def run_cgi(self, executable, method, parsed):
        content_length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(content_length) if content_length else b""
        action = parse_qs(parsed.query).get("action", [""])[0]
        query = parse_qs(parsed.query)
        upgrade_attempt = None
        if method == "POST" and action == "upgrade":
            upgrade_attempt = {
                "version": query.get("version", [""])[0],
                "status": None,
                "success": None,
                "seq": None,
                "instance": None,
                "receiptLost": False,
            }
            with self.fixture.lock:
                self.fixture.upgrade_posts.append(upgrade_attempt)
        path = parsed.path
        if "index.cgi" in path:
            path_info = path.split("index.cgi", 1)[1]
            script_name = path[:path.index("index.cgi") + len("index.cgi")]
        else:
            path_info = ""
            script_name = path
        env = self.fixture.env.copy()
        env.update({
            "REQUEST_METHOD": method,
            "QUERY_STRING": parsed.query,
            "CONTENT_LENGTH": str(len(body)),
            "CONTENT_TYPE": self.headers.get("Content-Type", ""),
            "HTTP_COOKIE": self.headers.get("Cookie", ""),
            "HTTP_X_CSRF_TOKEN": self.headers.get("X-CSRF-Token", ""),
            "HTTP_ACCEPT_ENCODING": self.headers.get("Accept-Encoding", ""),
            "REQUEST_URI": self.path,
            "SCRIPT_NAME": script_name,
            "PATH_INFO": path_info,
            "REMOTE_ADDR": self.client_address[0],
            "HTTP_HOST": "qwenpaw-backend.internal",
            "HTTP_ORIGIN": self.headers.get("Origin", ""),
            "HTTPS": "on",
            "REQUEST_SCHEME": "https",
        })
        try:
            result = subprocess.run(
                [str(executable)], cwd=self.fixture.root, env=env, input=body,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60, check=False,
            )
        except subprocess.TimeoutExpired:
            self.send_json(504, {"success": False, "message": "isolated CGI timed out"})
            return
        if result.returncode != 0:
            self.send_json(500, {
                "success": False,
                "message": f"isolated CGI exited {result.returncode}",
                "stderr": result.stderr.decode("utf-8", errors="replace")[:1000],
            })
            return
        try:
            status, headers, response_body = parse_cgi(result.stdout)
        except (RuntimeError, ValueError) as error:
            self.send_json(502, {"success": False, "message": str(error)})
            return

        try:
            payload = json.loads(response_body)
            if not isinstance(payload, dict):
                payload = {}
        except (json.JSONDecodeError, UnicodeDecodeError):
            payload = {}
        with self.fixture.lock:
            should_lose = method == "POST" and action == "upgrade" and self.fixture.lose_upgrade_receipt
            if should_lose:
                self.fixture.lose_upgrade_receipt = False
            if upgrade_attempt is not None:
                upgrade_attempt.update({
                    "status": status,
                    "success": payload.get("success"),
                    "seq": payload.get("seq"),
                    "instance": payload.get("instance"),
                    "receiptLost": should_lose,
                })
            elif method == "GET" and action == "upgrade_status":
                self.fixture.upgrade_statuses.append({
                    "phase": payload.get("phase"),
                    "seq": payload.get("seq"),
                    "instance": payload.get("instance"),
                    "upgrading": payload.get("upgrading"),
                })
        if should_lose:
            self.close_connection = True
            try:
                self.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.connection.close()
            return

        self.send_response(status)
        sent_content_length = False
        for name, value in headers:
            lower = name.lower()
            if lower in ("connection", "server", "date", "transfer-encoding"):
                continue
            if lower == "content-length":
                sent_content_length = True
            self.send_header(name, value)
        if not sent_content_length:
            self.send_header("Content-Length", str(len(response_body)))
        self.send_header("Connection", "close")
        self.end_headers()
        if method != "HEAD":
            self.wfile.write(response_body)
        self.close_connection = True

    def send_json(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def log_message(self, *_args):
        pass


def main():
    for path, label in (
        (SUPERVISOR_SOURCE, "release supervisor"),
        (RUNTIME_SOURCE, "release test-runtime"),
        (INDEX_SOURCE, "native index.cgi"),
        (API_SOURCE, "native api.cgi"),
        (WEB_SOURCE, "built web console"),
    ):
        if not path.is_file() or not os.access(path, os.X_OK if path in (SUPERVISOR_SOURCE, RUNTIME_SOURCE, INDEX_SOURCE, API_SOURCE) else os.R_OK):
            raise SystemExit(f"{label} is missing or unusable: {path}")

    port = int(os.environ.get("QWENPAW_BROWSER_PORT", "4175"))
    fixture = None
    server = None
    try:
        fixture = Fixture(port)
        fixture.start()
        server = http.server.ThreadingHTTPServer(("127.0.0.1", port), BrowserHandler)
        server.fixture = fixture
        server.index_requests = []
        server.daemon_threads = False
        server.block_on_close = True
        def stop_server(_signum, _frame):
            threading.Thread(target=server.shutdown, daemon=True).start()
        signal.signal(signal.SIGTERM, stop_server)
        signal.signal(signal.SIGINT, stop_server)
        print(f"TEST_SUPERVISOR={SUPERVISOR_SOURCE}", flush=True)
        print(f"TEST_SUPERVISOR_SHA256={fixture.binary_sha256}", flush=True)
        print(f"TEST_RUNTIME={RUNTIME_SOURCE}", flush=True)
        print(f"TEST_RUNTIME_SHA256={fixture.runtime_sha256}", flush=True)
        print(f"TEST_INDEX_CGI_SHA256={fixture.index_sha256}", flush=True)
        print(f"TEST_API_CGI_SHA256={fixture.api_sha256}", flush=True)
        print(f"TEST_WEB_CONSOLE_SHA256={fixture.web_sha256}", flush=True)
        print(f"TEST_FIXTURE_ROOT={fixture.root}", flush=True)
        print(f"BROWSER_CGI_GATEWAY=http://127.0.0.1:{port}", flush=True)
        server.serve_forever(poll_interval=0.1)
    finally:
        if server is not None:
            server.server_close()
        if fixture is not None:
            fixture.stop()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"browser CGI gateway failed: {error}", file=sys.stderr)
        raise SystemExit(1)
