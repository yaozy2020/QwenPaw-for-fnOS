#!/usr/bin/env python3
"""Test-only browser HTTP(S) -> Gateway SOCK_STREAM adapter."""

import ctypes
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import ssl
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = Path(__file__).resolve()
SUPERVISOR_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_SUP_BIN", ROOT / "supervisor/target/release/qwenpaw-supervisor"
)).resolve()
RUNTIME_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_RUNTIME_BIN", ROOT / "supervisor/target/release/qwenpaw-test-runtime"
)).resolve()
WEB_SOURCE = Path(os.environ.get("QWENPAW_TEST_WEB_HTML", ROOT / "web/dist/index.html")).resolve()
PREFIX = "/app/qwenpaw"
API = PREFIX + "/api/"
ADMIN = {"X-Trim-Userid": "1000", "X-Trim-Username": "browser-gateway-test-admin", "X-Trim-Isadmin": "true"}
TOKEN = "isolated-browser-gateway-supervisor-token-2026"
MAX_BODY = 64 * 1024
MAX_RESPONSE = 8 * 1024 * 1024
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


def read_process(pid):
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
        fields = raw[raw.rfind(")") + 2:].split()
        executable = os.readlink(f"/proc/{pid}/exe")
        if executable.endswith(" (deleted)"):
            executable = executable[:-10]
        return (pid, int(fields[19]), executable), fields[0], int(fields[1])
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


def write_cleanup_report(value):
    path = os.environ.get("QWENPAW_BROWSER_CLEANUP_REPORT")
    if path:
        temporary = Path(path + ".tmp")
        temporary.write_text(json.dumps(value))
        temporary.replace(path)


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def wait_for(predicate, label, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.03)
    raise RuntimeError(f"timed out: {label}")


def identity_for(process, executable):
    info = read_process(process.pid)
    if info is None or info[1] in ("Z", "X"):
        return None
    if info[0][2] != str(executable):
        return None
    status = Path(f"/proc/{process.pid}/status").read_text()
    uid = int(next(line for line in status.splitlines() if line.startswith("Uid:")).split()[2])
    require(uid == os.geteuid(), "fixture process euid mismatch")
    again = read_process(process.pid)
    require(again is not None and again[0] == info[0], "fixture process starttime/exe changed")
    return info[0]


class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, fixture):
        super().__init__("qwenpaw-backend.internal", timeout=10)
        self.fixture = fixture

    def connect(self):
        self.sock = self.fixture.connect_gateway()


class GatewayFixture:
    def __init__(self, port, tls_enabled=False):
        self.temporary_directory = tempfile.TemporaryDirectory(prefix="qpg-gateway-")
        self.root = Path(self.temporary_directory.name).resolve()
        require(not self.root.is_relative_to(Path("/var/apps/qwenpaw")),
                f"refusing to use a real deployment path: {self.root}")
        self.port = port
        self.tls_enabled = tls_enabled
        self.home = self.root / "home"
        self.var = self.root / "var"
        self.etc = self.root / "etc"
        self.share_root = self.root / "shares/qwenpaw"
        self.share = self.share_root / ".qwenpaw"
        self.share_secret = self.share_root / ".qwenpaw.secret"
        self.appdest = self.root / "target"
        self.tmp = self.root / "tmp"
        for path in (self.home, self.var, self.etc, self.share, self.share_secret,
                     self.appdest / "bin", self.appdest / "ui", self.appdest / "www", self.tmp):
            path.mkdir(parents=True, exist_ok=True)

        self.binary = self.appdest / "bin/qwenpaw-supervisor"
        self.runtime = self.home / "venv/bin/python3"
        self.base_python = self.root / "runtime-source"
        shutil.copy2(SUPERVISOR_SOURCE, self.binary)
        shutil.copy2(RUNTIME_SOURCE, self.base_python)
        self.binary_sha256 = sha256(SUPERVISOR_SOURCE)
        self.runtime_sha256 = sha256(RUNTIME_SOURCE)
        self.web_sha256 = sha256(WEB_SOURCE)
        self.source_hashes = {
            SUPERVISOR_SOURCE: self.binary_sha256,
            RUNTIME_SOURCE: self.runtime_sha256,
            WEB_SOURCE: self.web_sha256,
            SCRIPT: sha256(SCRIPT),
        }
        require(sha256(self.binary) == self.binary_sha256
                and sha256(self.base_python) == self.runtime_sha256,
                "test input changed while preparing isolated Gateway fixture")

        self.lock = threading.Lock()
        self.reservations = {}
        self.web_proxy_ports = set()
        self.web_proxy_setting = os.environ.get("QWENPAW_E2E_WEB_PROXY_PORT", "off")
        require(self.web_proxy_setting in ("off", "0"), "browser fixture only supports off or automatic Web proxy ports")
        self.mirror_port = self.reserve_port()
        self.supervisor_port = self.reserve_port()
        self.app_port = self.reserve_port()
        (self.var / "service_port").write_text(f"{self.app_port}\n")
        (self.var / "qwenpaw-log-level").write_text("info\n")
        (self.var / "qwenpaw.log").write_text("browser-e2e-runtime-log\n")
        self.token_file = self.var / "supervisor-token"
        self.token_file.write_text(TOKEN)
        os.chmod(self.token_file, 0o600)
        self.mirror_file = self.var / "pip-mirror"
        mirror_base = f"http://127.0.0.1:{self.mirror_port}/mirror/"
        self.mirror_file.write_text(f"{mirror_base}aliyun\n")

        self.user_config = self.share / "config.json"
        self.user_config.write_text('{"fixture":"preserve-user-data"}\n')
        self.auth_file = self.share_secret / "auth.json"
        self.auth_file.write_text("isolated-browser-auth-must-survive\n")
        self.ui_path = self.appdest / "ui/config"
        self.ui_path.write_text(json.dumps({".url": {
            "qwenpaw.main": {
                "type": "iframe", "protocol": "", "url": PREFIX + "/web/",
                "gatewayPrefix": PREFIX, "gatewaySocket": "app.sock",
                "noDisplay": False,
            },
            "qwenpaw.console": {
                "url": PREFIX + "/", "gatewayPrefix": PREFIX,
                "gatewaySocket": "app.sock", "type": "iframe",
            },
        }}))
        self.console = (self.appdest / "www/index.html")
        html = WEB_SOURCE.read_text(encoding="utf-8")
        for name, source in MIRROR_SOURCES.items():
            replacement = mirror_base + name
            require(source in html, f"built console does not contain expected mirror URL: {source}")
            html = html.replace(source, replacement)
        self.console.write_text(html, encoding="utf-8")

        self.gateway = self.root / "gateway.sock"
        require(len(os.fsencode(self.gateway)) < 104, "fixture gateway socket path too long")
        self.gateway_inode = None
        self.adapter_identity = read_process(os.getpid())[0]
        require(self.adapter_identity[2] == os.environ["QWENPAW_BROWSER_ADAPTER_EXE"],
                "adapter executable identity changed")
        self.source_hashes[Path(self.adapter_identity[2])] = sha256(Path(self.adapter_identity[2]))
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
            "QWENPAW_WEB_PROXY_PORT": self.web_proxy_setting,
            "QWENPAW_PYTHON_BIN": str(self.base_python),
            "QWENPAW_TEST_CONTROL": str(self.root),
            "QWENPAW_GATEWAY_SOCKET": str(self.gateway),
            "QWENPAW_ENABLE_CRASH_POINT": "1",
            "QWENPAW_TEST_GATEWAY_PEER_EXE": self.adapter_identity[2],
            "TMPDIR": str(self.tmp),
            "PATH": "/usr/bin:/bin",
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
        })
        self.forwarded = []
        self.process = None
        self.process_identity = None
        self.descendants = {}
        self.lose_upgrade_receipt = False
        self.upgrade_posts = []
        self.upgrade_statuses = []
        self.sentinel = None
        self.sentinel_identity = None
        self.log = (self.root / "supervisor.log.console").open("ab")

    def reserve_port(self):
        reservation = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
        require(port != self.port, "reserved runtime port collides with browser adapter")
        self.reservations[port] = reservation
        return port

    def release_port(self, port):
        with self.lock:
            reservation = self.reservations.pop(port, None)
        if reservation is not None:
            reservation.close()

    def connect_gateway(self):
        require(self.process_identity is not None and process_is_alive(self.process_identity),
                "gateway owner is not the registered supervisor")
        info = self.gateway.lstat()
        require(stat.S_ISSOCK(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
                and info.st_uid == os.geteuid(), "unsafe gateway socket")
        inode = (info.st_dev, info.st_ino)
        require(self.gateway_inode in (None, inode), "gateway socket identity changed")
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            connection.settimeout(10)
            connection.connect(str(self.gateway))
            pid, uid, _gid = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            peer = read_process(pid)
            require(uid == os.geteuid() and peer is not None and peer[0] == self.process_identity,
                    "refusing to send HTTP to an unregistered gateway peer")
            current = self.gateway.lstat()
            require((current.st_dev, current.st_ino) == inode, "gateway socket changed during connect")
            self.gateway_inode = inode
            return connection
        except BaseException:
            connection.close()
            raise

    def enable_tls(self, server):
        certificate = self.root / "browser-cert.pem"
        private_key = self.root / "browser-key.pem"
        result = subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(private_key),
             "-out", str(certificate), "-days", "1", "-subj", "/CN=127.0.0.1",
             "-addext", "subjectAltName=IP:127.0.0.1"],
            capture_output=True, text=True, check=False,
        )
        require(result.returncode == 0, f"cannot generate test TLS certificate: {result.stderr.strip()}")
        os.chmod(private_key, stat.S_IRUSR | stat.S_IWUSR)
        os.chmod(certificate, stat.S_IRUSR | stat.S_IWUSR)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, private_key)
        server.socket = context.wrap_socket(server.socket, server_side=True)

    def start(self):
        self.sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(600)"],
                                         env=dict(os.environ), start_new_session=True)
        wait_for(lambda: self.capture_sentinel(), "register unrelated sentinel")
        self.release_port(self.supervisor_port)
        self.process = subprocess.Popen([str(self.binary), "serve"], cwd=self.root, env=self.env,
                                        stdout=self.log, stderr=subprocess.STDOUT, start_new_session=True)

        def ready():
            require(self.process.poll() is None, f"release supervisor exited: {self.read_log()}")
            identity = identity_for(self.process, self.binary)
            if identity is None:
                return False
            self.process_identity = identity
            try:
                connection = UnixHTTPConnection(self)
                try:
                    connection.request("GET", API + "status", headers=ADMIN)
                    response = connection.getresponse()
                    data = response.read(MAX_RESPONSE + 1)
                    require(response.status == 200, f"real UDS readiness HTTP {response.status}: {data[:300]!r}")
                    payload = json.loads(data)
                    require(payload.get("success") is True and payload.get("installed") is False,
                            "unexpected UDS readiness status")
                    return True
                finally:
                    connection.close()
            except (FileNotFoundError, ConnectionRefusedError):
                return False
        wait_for(ready, "real Gateway UDS HTTP readiness")
        if self.web_proxy_setting == "0":
            path = self.var / "web_proxy_port"
            wait_for(path.exists, "automatic Web proxy port persistence")
            port = int(path.read_text().strip())
            require(1024 <= port <= 65535, "invalid automatic Web proxy port")
            self.web_proxy_ports.add(port)
        print(f"REGISTER supervisor={self.process_identity!r} adapter={self.adapter_identity!r} sentinel={self.sentinel_identity!r}", flush=True)

    def capture_sentinel(self):
        self.sentinel_identity = identity_for(self.sentinel, Path(sys.executable))
        return self.sentinel_identity is not None

    def read_log(self):
        self.log.flush()
        return (self.root / "supervisor.log.console").read_text(errors="replace")

    def metadata(self):
        return {
            "appPort": self.app_port,
            "transport": "HTTP -> AF_UNIX/SOCK_STREAM -> release supervisor Gateway",
            "fixtureVersion": 1,
            "identityBoundary": "root UID in isolated user namespace; non-root test-only executable fallback + synthetic admin; NOT fnOS identity/session acceptance",
            "managementTCP": "not forwarded",
            "adapter": {"pid": self.adapter_identity[0], "starttime": self.adapter_identity[1],
                        "exe": self.adapter_identity[2], "uid": os.geteuid(), "role": "test-admin",
                        "uidMap": Path("/proc/self/uid_map").read_text().strip()},
            "injectedIdentity": dict(ADMIN),
            "binaries": {
                "runtime": {"path": str(RUNTIME_SOURCE), "sha256": self.runtime_sha256},
                "supervisor": {"path": str(SUPERVISOR_SOURCE), "sha256": self.binary_sha256},
                "webConsole": {"path": str(WEB_SOURCE), "sha256": self.web_sha256},
                "fixture": {"path": str(SCRIPT), "sha256": self.source_hashes[SCRIPT]},
                "adapter": {"path": self.adapter_identity[2],
                            "sha256": self.source_hashes[Path(self.adapter_identity[2])]},
            },
            "gatewaySocket": str(self.gateway),
            "mirrors": {
                "aliyun": f"http://127.0.0.1:{self.mirror_port}/mirror/aliyun",
                "tsinghua": f"http://127.0.0.1:{self.mirror_port}/mirror/tsinghua",
            },
            "files": {
                "auth": str(self.auth_file), "config": str(self.user_config),
                "installLog": str(self.var / "qwenpaw-upgrade.log.pip"),
                "kernelLog": str(self.var / "qwenpaw.log"), "mirror": str(self.mirror_file),
                "pin": str(self.var / "qwenpaw-version"),
                "servicePort": str(self.var / "service_port"), "uiConfig": str(self.ui_path),
                "webProxyPort": str(self.var / "web_proxy_port"), "venv": str(self.home / "venv"),
            },
        }

    def remember_descendants(self):
        if self.process is None or self.process_identity is None or not process_is_alive(self.process_identity):
            return
        for identity in process_tree(self.process.pid):
            if identity[0] != self.process.pid:
                require(Path(identity[2]).is_relative_to(self.root),
                        "supervisor descendant escaped the fixture executable roots")
                self.descendants[identity[0]] = identity

    def cleanup_descendants(self):
        for identity in self.descendants.values():
            if process_is_alive(identity):
                signal_owned_process(identity, signal.SIGTERM)
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline and process_is_alive(identity):
                    time.sleep(0.05)
                if process_is_alive(identity):
                    signal_owned_process(identity, signal.SIGKILL)
            require(not process_is_alive(identity), "registered descendant survived cleanup")
            try:
                os.waitpid(identity[0], os.WNOHANG)
            except ChildProcessError:
                pass
        require(not self.gateway.exists(), "graceful shutdown left the Gateway socket behind")
        require(self.sentinel_identity is not None and process_is_alive(self.sentinel_identity),
                "unrelated sentinel did not survive supervisor shutdown")

    def stop(self):
        errors = []
        for reservation in self.reservations.values():
            reservation.close()
        self.reservations.clear()
        try:
            if self.process is not None:
                self.remember_descendants()
                if self.process.poll() is None:
                    require(self.process_identity is not None, "cannot safely identify supervisor for cleanup")
                    signal_owned_process(self.process_identity, signal.SIGTERM)
                    try:
                        self.process.wait(timeout=29)
                    except subprocess.TimeoutExpired:
                        signal_owned_process(self.process_identity, signal.SIGKILL)
                        self.process.wait(timeout=5)
                        raise RuntimeError("Gateway supervisor cleanup exceeded graceful shutdown budget")
                require(self.process.returncode == 0,
                        f"Gateway supervisor exited {self.process.returncode}: {self.read_log()}")
                if self.process_identity is not None:
                    require(not process_is_alive(self.process_identity),
                            "supervisor identity remains alive after wait")
                self.process = None
                self.cleanup_descendants()

            for path, expected in self.source_hashes.items():
                require(path.is_file() and sha256(path) == expected,
                        f"test input changed during browser run: {path}")
            runtime_ports = {self.app_port}
            try:
                runtime_ports.add(int((self.var / "service_port").read_text().strip()))
            except (OSError, ValueError):
                errors.append("could not verify the final isolated test-runtime port")
            if self.web_proxy_setting == "0":
                try:
                    self.web_proxy_ports.add(int((self.var / "web_proxy_port").read_text().strip()))
                except (OSError, ValueError):
                    errors.append("could not verify the final Web proxy port")
            for port, label in [(self.supervisor_port, "supervisor"),
                                *((port, "test-runtime") for port in runtime_ports),
                                *((port, "web-proxy") for port in self.web_proxy_ports)]:
                try:
                    connection = socket.create_connection(("127.0.0.1", port), timeout=0.25)
                except OSError:
                    continue
                connection.close()
                errors.append(f"isolated {label} endpoint remains reachable on port {port}")
        except Exception as error:
            errors.append(str(error))

        if self.sentinel is not None:
            try:
                require(self.sentinel_identity is not None, "sentinel identity was never registered")
                require(process_is_alive(self.sentinel_identity),
                        "unrelated sentinel was killed during management flow/cleanup")
            except Exception as error:
                errors.append(str(error))
            finally:
                if self.sentinel_identity is not None:
                    try:
                        signal_owned_process(self.sentinel_identity, signal.SIGTERM)
                        self.sentinel.wait(timeout=5)
                    except Exception as error:
                        errors.append(str(error))
        if not self.log.closed:
            self.log.close()
        self.temporary_directory.cleanup()
        require(not self.root.exists(), "temporary fixture root was not removed")
        require(not errors, "; ".join(errors))
        print(f"CLEANUP gateway root={self.root} supervisor and descendants stopped; socket removed; sentinel survived and reaped", flush=True)


class GatewayHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    @property
    def fixture(self):
        return self.server.fixture

    def setup(self):
        super().setup()
        self.connection.settimeout(10)

    def handle_request(self):
        require(self.client_address[0] == "127.0.0.1", "non-loopback browser peer")
        if self.headers.get_all("Host") != [f"127.0.0.1:{self.fixture.port}"]:
            self.send_error(403)
            return
        parsed = urlsplit(self.path)
        if parsed.scheme or parsed.netloc:
            self.send_error(400)
            return
        lengths = self.headers.get_all("Content-Length", [])
        if len(lengths) > 1 or self.headers.get("Transfer-Encoding"):
            self.send_error(400)
            return
        try:
            size = int(lengths[0]) if lengths else 0
        except ValueError:
            self.send_error(400)
            return
        if not 0 <= size <= MAX_BODY:
            self.send_error(413)
            return
        body = self.rfile.read(size)
        if len(body) != size:
            self.send_error(400)
            return
        if self.command == "GET" and parsed.path == "/__e2e/allocate-port":
            with self.fixture.lock:
                port = self.fixture.reserve_port()
            self.send_json(200, {"port": port})
            return
        if self.command == "GET" and parsed.path == "/__e2e/observations":
            with self.fixture.lock:
                observations = {"indexRequests": list(self.server.index_requests),
                                "upgradePosts": [dict(item) for item in self.fixture.upgrade_posts],
                                "upgradeStatuses": [dict(item) for item in self.fixture.upgrade_statuses],
                                "forwarded": [dict(item) for item in self.fixture.forwarded]}
            self.send_json(200, observations)
            return
        if self.command == "GET" and parsed.path == "/__e2e/meta":
            self.send_json(200, self.fixture.metadata())
            return
        if self.command == "GET" and parsed.path == "/favicon.ico":
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            return
        if self.command == "GET" and parsed.path.startswith("/mirror/"):
            self.serve_index(parsed.path)
            return
        if self.command == "POST" and parsed.path == "/__e2e/lose-next-upgrade-receipt":
            with self.fixture.lock:
                self.fixture.lose_upgrade_receipt = True
            self.send_json(200, {"armed": True})
            return
        if parsed.path != PREFIX and not parsed.path.startswith(PREFIX + "/"):
            self.send_error(404)
            return
        self.forward_gateway(parsed, body)

    do_GET = handle_request
    do_HEAD = handle_request
    do_POST = handle_request
    do_PUT = handle_request
    do_DELETE = handle_request
    do_OPTIONS = handle_request

    def serve_index(self, path):
        if path.endswith("/qwenpaw/") and path.count("/") == 4:
            with self.fixture.lock:
                self.server.index_requests.append(path)
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(INDEX_BODY)))
            self.send_header("Connection", "close")
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(INDEX_BODY)
            self.close_connection = True
            return
        self.send_error(404)

    def send_json(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)
        self.close_connection = True

    def forward_gateway(self, parsed, body):
        fixture = self.fixture
        headers = {"Host": "qwenpaw-backend.internal", "Connection": "close",
                   "X-Forwarded-Proto": "https", **ADMIN}
        for name in ("Origin", "Referer", "Sec-Fetch-Site", "Cookie", "X-CSRF-Token",
                     "Accept-Encoding", "Content-Type", "Accept"):
            values = self.headers.get_all(name, [])
            if len(values) > 1:
                self.send_error(400)
                return
            if values:
                headers[name] = values[0]
        try:
            data = json.loads(body) if body else {}
            if not isinstance(data, dict):
                data = {}
        except (ValueError, UnicodeDecodeError):
            data = {}
        if self.command == "POST" and parsed.path == API + "kernel/install":
            fixture.release_port(fixture.app_port)
        if self.command == "POST" and parsed.path == API + "port" and isinstance(data.get("port"), int):
            fixture.release_port(data["port"])
        record = {"method": self.command, "path": parsed.path, "origin": headers.get("Origin"),
                  "upstreamHost": headers["Host"], "fetchSite": headers.get("Sec-Fetch-Site"),
                  "contentType": headers.get("Content-Type"), "csrfHeader": bool(headers.get("X-CSRF-Token")),
                  "csrfQuery": "_csrf=" in parsed.query, "body": data, "receiptLost": False}
        require(TOKEN.encode() not in body, "private token appeared in a browser request")
        connection = UnixHTTPConnection(fixture)
        try:
            connection.request(self.command, self.path, body=body or None, headers=headers)
            upstream = connection.getresponse()
            response_body = upstream.read(MAX_RESPONSE + 1)
            require(len(response_body) <= MAX_RESPONSE, "gateway response exceeded fixture limit")
            response_headers = upstream.getheaders()
            status = upstream.status
        finally:
            connection.close()
        require(TOKEN.encode() not in response_body and TOKEN not in str(response_headers),
                "private supervisor token was disclosed")
        try:
            payload = json.loads(response_body)
            if not isinstance(payload, dict):
                payload = {}
        except (ValueError, UnicodeDecodeError):
            payload = {}
        record.update({"status": status, "success": payload.get("success"),
                       "seq": payload.get("seq"), "instance": payload.get("instance")})
        with fixture.lock:
            if self.command == "POST" and parsed.path == API + "kernel/install":
                if fixture.lose_upgrade_receipt:
                    require(200 <= status < 300 and payload.get("success") is True
                            and isinstance(payload.get("seq"), int) and payload.get("instance"),
                            "loss injection did not reach a real accepted result")
                    fixture.lose_upgrade_receipt = False
                    record["receiptLost"] = True
                fixture.upgrade_posts.append({**record, "version": data.get("version"), "mirror": data.get("mirror")})
            if self.command == "GET" and parsed.path == API + "kernel/install-status":
                fixture.upgrade_statuses.append({name: payload.get(name)
                                                  for name in ("phase", "seq", "instance", "upgrading")})
            fixture.forwarded.append(record)
        fixture.remember_descendants()
        if record["receiptLost"]:
            self.close_connection = True
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return
        self.send_response(status)
        for name, value in response_headers:
            if name.lower() not in ("connection", "content-length", "transfer-encoding", "server", "date"):
                self.send_header(name, value)
        length = next((value for name, value in response_headers if name.lower() == "content-length"), "0") if self.command == "HEAD" else str(len(response_body))
        self.send_header("Content-Length", length)
        self.send_header("Connection", "close")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(response_body)
        self.close_connection = True

    def log_message(self, *_args):
        pass


class MirrorHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    @property
    def fixture(self):
        return self.server.fixture

    def handle_request(self):
        require(self.client_address[0] == "127.0.0.1", "non-loopback mirror peer")
        if self.headers.get_all("Host") != [f"127.0.0.1:{self.server.server_port}"]:
            self.send_error(403)
            return
        parsed = urlsplit(self.path)
        if parsed.scheme or parsed.netloc:
            self.send_error(400)
            return
        if self.command in ("GET", "HEAD") and parsed.path.startswith("/mirror/"):
            GatewayHandler.serve_index(self, parsed.path)
            return
        self.send_error(404)

    do_GET = handle_request
    do_HEAD = handle_request

    def log_message(self, *_args):
        pass


def serve():
    require(os.geteuid() == 0, "test Gateway peer needs UID 0 in an isolated user namespace, never sudo")
    require(ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, "cannot enable adopted-child cleanup")
    for path in (SUPERVISOR_SOURCE, RUNTIME_SOURCE):
        require(path.is_file() and os.access(path, os.X_OK), f"release binary missing: {path}")
        with path.open("rb") as stream:
            require(stream.read(4) == b"\x7fELF", f"not a native release binary: {path}")
    require(WEB_SOURCE.is_file(), "built console missing; use npm run test:e2e:gateway --prefix web")
    tls_mode = os.environ.get("QWENPAW_BROWSER_TLS", "0")
    require(tls_mode in ("0", "1"), "QWENPAW_BROWSER_TLS must be 0 or 1")
    tls_enabled = tls_mode == "1"
    port = int(os.environ.get("QWENPAW_BROWSER_PORT", "4176"))
    require(1024 <= port <= 65535, "invalid browser adapter port")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), GatewayHandler)
    server.daemon_threads = False
    server.block_on_close = True
    fixture = None
    mirror_server = None
    mirror_thread = None
    shutdown_thread = None
    errors = []
    ready = False
    try:
        fixture = GatewayFixture(port, tls_enabled)
        server.fixture = fixture
        server.index_requests = []
        fixture.release_port(fixture.mirror_port)
        mirror_server = http.server.ThreadingHTTPServer(("127.0.0.1", fixture.mirror_port), MirrorHandler)
        mirror_server.fixture = fixture
        mirror_server.index_requests = server.index_requests
        mirror_server.daemon_threads = False
        mirror_server.block_on_close = True
        mirror_thread = threading.Thread(target=mirror_server.serve_forever, daemon=False)
        mirror_thread.start()
        if tls_enabled:
            fixture.enable_tls(server)
        fixture.start()
        ready = True

        def shutdown(_signum, _frame):
            nonlocal shutdown_thread

            def stop_servers():
                server.shutdown()
                mirror_server.shutdown()

            shutdown_thread = threading.Thread(target=stop_servers, daemon=True)
            shutdown_thread.start()

        signal.signal(signal.SIGTERM, shutdown)
        signal.signal(signal.SIGINT, shutdown)
        print(json.dumps(fixture.metadata(), ensure_ascii=False), flush=True)
        scheme = "https" if tls_enabled else "http"
        print(f"BROWSER_GATEWAY={scheme}://127.0.0.1:{port}{PREFIX}/", flush=True)
        server.serve_forever(poll_interval=0.1)
    finally:
        server.server_close()
        if mirror_server is not None:
            mirror_server.shutdown()
            mirror_server.server_close()
        if mirror_thread is not None:
            mirror_thread.join(timeout=5)
            if mirror_thread.is_alive():
                errors.append("local mirror HTTP server did not stop")
        if shutdown_thread is not None:
            shutdown_thread.join(timeout=5)
            if shutdown_thread.is_alive():
                errors.append("Gateway HTTP server shutdown did not finish")
        if fixture is not None:
            if fixture.lose_upgrade_receipt:
                errors.append("receipt loss was armed but never reached a real accepted result")
            try:
                fixture.stop()
            except Exception as error:
                errors.append(str(error))
        write_cleanup_report({"ok": ready and not errors, "errors": errors,
                              "rootRemoved": fixture is not None and not fixture.root.exists(),
                              "supervisorSha256": fixture.binary_sha256 if fixture is not None else None,
                              "checks": ["pidfd/starttime/exe", "supervisor/descendant exit",
                                         "Gateway socket removed before root deletion",
                                         "runtime/management ports closed", "source hashes unchanged",
                                         "unrelated sentinel survived/reaped"]})
        require(not errors, "; ".join(errors))


def main():
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
            "safe cleanup requires Linux pidfds")
    if sys.argv[1:] == ["--serve"]:
        serve()
        return
    require(not sys.argv[1:], "use npm run test:e2e:gateway --prefix web")
    with tempfile.TemporaryDirectory(prefix="qpg-peer-") as root:
        executable = Path(root) / "python3"
        shutil.copy2(Path(os.readlink("/proc/self/exe")), executable)
        env = dict(os.environ, PYTHONHOME=sys.base_prefix, QWENPAW_BROWSER_ADAPTER_EXE=str(executable))
        command = [str(executable), str(SCRIPT), "--serve"]
        if os.geteuid() != 0:
            unshare = shutil.which("unshare")
            require(unshare is not None, "non-root Gateway fixture requires unshare --user --map-root-user")
            command = [unshare, "--user", "--map-root-user", *command]
        process = subprocess.Popen(command, env=env, start_new_session=True)
        identity = None

        def register():
            nonlocal identity
            require(process.poll() is None, f"adapter bootstrap failed: exit {process.returncode}")
            info = read_process(process.pid)
            if info is not None and info[0][2] == str(executable):
                identity = info[0]
                return True
            return False

        def forward(signum, _frame):
            if identity is not None:
                signal_owned_process(identity, signum)

        signal.signal(signal.SIGTERM, forward)
        signal.signal(signal.SIGINT, forward)
        try:
            wait_for(register, "adapter executable identity after unshare")
            require(process.wait() == 0, "Gateway fixture or its cleanup failed")
        finally:
            if process.poll() is None:
                require(identity is not None, "cannot signal unregistered adapter process")
                signal_owned_process(identity, signal.SIGTERM)
                try:
                    process.wait(timeout=45)
                except subprocess.TimeoutExpired:
                    signal_owned_process(identity, signal.SIGKILL)
                    process.wait(timeout=5)
                    raise RuntimeError("Gateway fixture cleanup exceeded budget")
    report_path = os.environ.get("QWENPAW_BROWSER_CLEANUP_REPORT")
    if report_path:
        report = json.loads(Path(report_path).read_text())
        report.update({"finalized": True, "peerRootRemoved": not Path(root).exists()})
        write_cleanup_report(report)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"browser Gateway failed: {error}", file=sys.stderr, flush=True)
        raise SystemExit(1)
