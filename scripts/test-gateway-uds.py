#!/usr/bin/env python3
"""Local release-binary UDS tests; not fnOS gateway/session identity acceptance."""

import base64
import ctypes
import ctypes.util
import gzip
import hashlib
import http.client
from http.cookies import SimpleCookie
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
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
SUPERVISOR = Path(os.environ.get(
    "QWENPAW_TEST_SUP_BIN", ROOT / "supervisor/target/release/qwenpaw-supervisor"
)).resolve()
RUNTIME = Path(os.environ.get(
    "QWENPAW_TEST_RUNTIME_BIN", ROOT / "supervisor/target/release/qwenpaw-test-runtime"
)).resolve()
PREFIX = "/app/qwenpaw"
API = PREFIX + "/api"
ADMIN = {"X-Trim-Userid": "1000", "X-Trim-Username": "gateway-test-admin",
         "X-Trim-Isadmin": "true"}
USER = {"X-Trim-Userid": "1001", "X-Trim-Username": "gateway-test-user",
        "X-Trim-Isadmin": "false"}
PAGE = b"<!doctype html><meta charset=utf-8><title>Isolated Gateway UDS</title>\n"
CSS = b"body{color:#123456}\n"
SECRET = b"isolated-outside-secret-must-not-be-disclosed"
DIAGNOSTIC_LOGS = ("process.log", "var/supervisor.log")


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def wait_for(predicate, label, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(f"timed out: {label}")


def process_info(pid):
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
        fields = raw[raw.rfind(")") + 2:].split()
        exe = os.readlink(f"/proc/{pid}/exe")
        status = Path(f"/proc/{pid}/status").read_text()
        uid = next(line for line in status.splitlines() if line.startswith("Uid:")).split()[2]
        argv = Path(f"/proc/{pid}/cmdline").read_bytes()
        current = Path(f"/proc/{pid}/stat").read_text()
        current_fields = current[current.rfind(")") + 2:].split()
        if fields[19] != current_fields[19]:
            return None
        return {"identity": (pid, int(fields[19]), exe), "state": current_fields[0],
                "parent": int(current_fields[1]), "uid": int(uid), "argv": argv}
    except (OSError, IndexError, ValueError, StopIteration):
        return None


def alive(identity):
    current = process_info(identity[0])
    return current is not None and current["identity"] == identity and current["state"] not in ("Z", "X")


def owned_signal(identity, signum):
    current = process_info(identity[0])
    if current is None or current["state"] in ("Z", "X"):
        return
    require(current["identity"] == identity, f"refusing to signal changed identity: {identity!r}")
    try:
        fd = os.pidfd_open(identity[0])
    except ProcessLookupError:
        return
    try:
        current = process_info(identity[0])
        if current is None or current["state"] in ("Z", "X"):
            return
        require(current["identity"] == identity, f"refusing to signal reused PID: {identity!r}")
        signal.pidfd_send_signal(fd, signum)
    finally:
        os.close(fd)


def process_tree(pid):
    processes = {}
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit():
            info = process_info(int(entry.name))
            if info is not None and info["state"] not in ("Z", "X"):
                processes[int(entry.name)] = info
    selected = {pid}
    while True:
        children = {child for child, info in processes.items() if info["parent"] in selected}
        if children <= selected:
            break
        selected.update(children)
    return tuple(sorted(processes[child]["identity"] for child in selected if child in processes))


def service_manager_identities(binary):
    executable = str(binary)
    expected_argv = [os.fsencode(executable), b"serve"]
    identities = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        info = process_info(int(entry.name))
        if info is None or info["state"] in ("Z", "X") or info["identity"][2] != executable:
            continue
        argv = info["argv"].split(b"\0")
        if argv and argv[-1] == b"":
            argv.pop()
        if argv == expected_argv:
            require(info["uid"] == os.geteuid(), f"fixture manager euid changed: {info['identity']!r}")
            identities.append(info["identity"])
    return tuple(sorted(identities))


def filesystem_snapshot(root, excluded=()):
    excluded = set(excluded)
    result = []
    for path in sorted(root.rglob("*")):
        relative = str(path.relative_to(root))
        if relative in excluded:
            continue
        info = path.lstat()
        common = (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid, info.st_mtime_ns)
        if stat.S_ISLNK(info.st_mode):
            content = os.readlink(path)
        elif stat.S_ISREG(info.st_mode):
            content = path.read_bytes()
        else:
            content = None
        result.append((relative, common, content))
    return tuple(result)


def diagnostic_log_snapshot(root):
    result = []
    for name in DIAGNOSTIC_LOGS:
        path = root / name
        if not path.exists():
            result.append((name, None))
            continue
        info = path.lstat()
        require(stat.S_ISREG(info.st_mode), f"diagnostic log is not a regular file: {path}")
        metadata = (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid)
        result.append((name, (metadata, path.read_bytes())))
    return tuple(result)


def assert_gateway_log_append_only(before, after):
    prefix = b"[Gateway] forbidden: "
    allowed = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 _-"
    for (before_name, before_state), (after_name, after_state) in zip(before, after):
        require(before_name == after_name, "diagnostic log set changed")
        if before_state is None:
            require(after_state is None or after_state[1], f"diagnostic log disappeared or stayed empty: {after_name}")
            previous = b""
        else:
            require(after_state is not None, f"diagnostic log disappeared: {after_name}")
            require(after_state[0] == before_state[0], f"diagnostic log metadata changed: {after_name}")
            previous = before_state[1]
        current = b"" if after_state is None else after_state[1]
        require(current.startswith(previous), f"diagnostic log was rewritten: {after_name}")
        delta = current[len(previous):]
        for line in delta.splitlines(keepends=True):
            reason = line[len(prefix):-1] if line.startswith(prefix) and line.endswith(b"\n") else b""
            require(reason and all(byte in allowed for byte in reason),
                    f"unexpected Gateway diagnostic log append: {line!r}")


class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout=10):
        super().__init__("127.0.0.1", timeout=timeout)
        self.path = str(path)

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)


def http_request(connection, path, method="GET", headers=None, body=None):
    request_headers = {"Host": "127.0.0.1", "Connection": "close"}
    request_headers.update(headers or {})
    payload = None if body is None else json.dumps(body).encode()
    if body is not None:
        request_headers["Content-Type"] = "application/json"
    try:
        connection.request(method, path, body=payload, headers=request_headers)
        response = connection.getresponse()
        data = response.read(8 * 1024 * 1024 + 1)
        require(len(data) <= 8 * 1024 * 1024, "response exceeded fixture limit")
        return response.status, response.getheaders(), data
    finally:
        connection.close()


def header(response, name):
    return next((value for key, value in response[1] if key.lower() == name.lower()), None)


def response_json(response):
    require(response[0] == 200, f"HTTP {response[0]}: {response[2][:500]!r}")
    return json.loads(response[2])


class BrotliCodec:
    def __init__(self):
        encoder = ctypes.util.find_library("brotlienc")
        decoder = ctypes.util.find_library("brotlidec")
        require(encoder and decoder, "Brotli fixtures require libbrotlienc and libbrotlidec; no skipped compression checks")
        self.encoder = ctypes.CDLL(encoder)
        self.decoder = ctypes.CDLL(decoder)
        self.encoder.BrotliEncoderMaxCompressedSize.argtypes = [ctypes.c_size_t]
        self.encoder.BrotliEncoderMaxCompressedSize.restype = ctypes.c_size_t
        self.encoder.BrotliEncoderCompress.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_int,
            ctypes.c_size_t, ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t), ctypes.c_void_p]
        self.encoder.BrotliEncoderCompress.restype = ctypes.c_int
        self.decoder.BrotliDecoderDecompress.argtypes = [ctypes.c_size_t, ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t), ctypes.c_void_p]
        self.decoder.BrotliDecoderDecompress.restype = ctypes.c_int

    def compress(self, data):
        size = ctypes.c_size_t(self.encoder.BrotliEncoderMaxCompressedSize(len(data)))
        output = ctypes.create_string_buffer(size.value)
        source = ctypes.create_string_buffer(data)
        require(self.encoder.BrotliEncoderCompress(5, 22, 0, len(data), source, ctypes.byref(size), output) == 1,
                "Brotli fixture compression failed")
        return output.raw[:size.value]

    def decompress(self, data, length):
        size = ctypes.c_size_t(length)
        output = ctypes.create_string_buffer(max(1, length))
        source = ctypes.create_string_buffer(data)
        require(self.decoder.BrotliDecoderDecompress(len(data), source, ctypes.byref(size), output) == 1,
                "gateway returned invalid Brotli bytes")
        return output.raw[:size.value]


class Fixture:
    def __init__(self, root, binary_sha, runtime_sha, brotli):
        self.root = root
        self.app = root / "target"
        self.www = self.app / "www"
        self.gateway = root / "gateway.sock"
        self.control = root / "var/supervisor-control.sock"
        self.binary = self.app / "bin/qwenpaw-supervisor"
        self.helper = root / "helpers/python3"
        self.python = root / "home/venv/bin/python3"
        self.log_path = root / "process.log"
        self.process = None
        self.identity = None
        self.log = None
        self.owned = set()
        self.ports = []
        for name in ("home", "var", "etc", "share", "tmp", "helpers",
                     "target/www/assets", "target/ui", "target/bin"):
            (root / name).mkdir(parents=True, exist_ok=True)
        require(len(os.fsencode(self.gateway)) < 104, f"temporary UDS path is too long: {self.gateway}")
        shutil.copy2(SUPERVISOR, self.binary)
        shutil.copy2(RUNTIME, self.helper)
        require(sha256(self.binary) == binary_sha, "release supervisor changed while copying")
        require(sha256(self.helper) == runtime_sha, "release runtime changed while copying")
        self.binary_sha = binary_sha
        self.runtime_sha = runtime_sha
        (self.www / "index.html").write_bytes(PAGE)
        (self.www / "index.html.gz").write_bytes(gzip.compress(PAGE, mtime=0))
        (self.www / "index.html.br").write_bytes(brotli.compress(PAGE))
        (self.www / "assets/site.css").write_bytes(CSS)
        (root / "secret.txt").write_bytes(SECRET)
        (self.www / "assets/escape.css").symlink_to(root / "secret.txt")
        (self.www / "assets/escape").symlink_to(root, target_is_directory=True)
        self.port = self.reserve_port()
        self.sup_port = self.reserve_port()
        self.index_port = self.reserve_port()
        (root / "var/service_port").write_text(f"{self.port}\n")
        (root / "var/qwenpaw-log-level").write_text("info\n")
        (root / "var/pip-mirror").write_text(f"http://127.0.0.1:{self.index_port}/simple/\n")
        self.ui = self.app / "ui/config"
        self.ui.write_text(json.dumps({".url": {
            "qwenpaw.main": {"port": str(self.port), "noDisplay": True},
            "qwenpaw.console": {
                "url": PREFIX + "/", "gatewayPrefix": PREFIX,
                "gatewaySocket": "app.sock", "type": "iframe",
            },
        }}))
        self.console = json.loads(self.ui.read_text())[".url"]["qwenpaw.console"]
        self.env = {
            "PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TMPDIR": str(root / "tmp"),
            "TRIM_APPNAME": "qwenpaw", "TRIM_APPDEST": str(self.app),
            "TRIM_PKGHOME": str(root / "home"), "TRIM_PKGVAR": str(root / "var"),
            "TRIM_PKGETC": str(root / "etc"), "TRIM_DATA_SHARE_PATHS": str(root / "share"),
            "TRIM_SERVICE_PORT": str(self.port), "SUPERVISOR_PORT": str(self.sup_port),
            "SUPERVISOR_AUTOSTART": "0", "QWENPAW_WEB_PROXY_PORT": "off",
            "QWENPAW_PYTHON_BIN": str(self.helper),
            "QWENPAW_TEST_CONTROL": str(root), "QWENPAW_GATEWAY_SOCKET": str(self.gateway),
            "QWENPAW_ENABLE_CRASH_POINT": "1",
            "QWENPAW_TEST_GATEWAY_PEER_EXE": os.readlink("/proc/self/exe"),
        }

    def reserve_port(self):
        reservation = socket.socket()
        reservation.bind(("127.0.0.1", 0))
        self.ports.append(reservation)
        return reservation.getsockname()[1]

    def install_runtime(self):
        require(self.process is None, "runtime fixture must be prepared before supervisor start")
        self.python.parent.mkdir(parents=True)
        shutil.copy2(self.helper, self.python)
        (self.python.parent.parent / "installed").write_text("1.0.0\n")
        (self.root / "var/qwenpaw-version").write_text("1.0.0\n")

    def token(self):
        token = (self.root / "var/supervisor-token").read_bytes().strip()
        require(len(token) >= 32, "fixture token is missing or invalid")
        if not hasattr(self, "tokens"):
            self.tokens = set()
        self.tokens.add(token)
        return token

    def no_token_leak(self, data):
        if (self.root / "var/supervisor-token").is_file():
            self.token()
        if isinstance(data, str):
            data = data.encode()
        for token in getattr(self, "tokens", ()):
            require(token not in data, "private supervisor token was disclosed")

    def request(self, path, method="GET", headers=None, body=None, identity=ADMIN):
        values = dict(identity or {})
        values.update(headers or {})
        response = http_request(UnixHTTPConnection(self.gateway), path, method, values, body)
        self.no_token_leak(json.dumps(response[1]).encode() + response[2])
        return response

    def open_event_stream(self):
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(5)
        connection.connect(str(self.gateway))
        headers = {"Host": "127.0.0.1", "Connection": "keep-alive", **ADMIN}
        request = "GET " + API + "/events HTTP/1.1\r\n"
        request += "".join(f"{name}: {value}\r\n" for name, value in headers.items())
        connection.sendall((request + "\r\n").encode())
        response = bytearray()
        while b"\r\n\r\n" not in response:
            chunk = connection.recv(1)
            require(chunk, "gateway closed SSE connection without an HTTP response")
            response.extend(chunk)
        self.no_token_leak(response)
        lines = response.decode("iso-8859-1").split("\r\n")
        status = int(lines[0].split()[1])
        response_headers = dict(line.split(":", 1) for line in lines[1:] if line)
        return connection, status, {name.lower(): value.strip() for name, value in response_headers.items()}

    def run_cli(self, *arguments, gateway=None, capture_service=False, timeout=35):
        env = dict(self.env)
        if gateway is not None:
            env["QWENPAW_GATEWAY_SOCKET"] = str(gateway)
        argv = [str(self.binary), *arguments]
        worker = subprocess.Popen(argv, env=env, cwd=self.root, stdin=subprocess.DEVNULL,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        identity = None
        deadline = time.monotonic() + timeout
        try:
            while True:
                info = process_info(worker.pid)
                if info is not None and info["state"] not in ("Z", "X") and info["argv"]:
                    require(info["identity"][2] == str(self.binary),
                            f"CLI executable identity mismatch: {info['identity']!r}")
                    require(info["uid"] == os.geteuid(), "CLI euid identity mismatch")
                    if identity is None:
                        identity = info["identity"]
                        self.owned.add(identity)
                        print(f"REGISTER cli={identity!r} args={arguments!r}", flush=True)
                    require(info["identity"] == identity, "CLI process identity changed")
                    actual_argv = info["argv"].split(b"\0")
                    if actual_argv and actual_argv[-1] == b"":
                        actual_argv.pop()
                    require(actual_argv == [os.fsencode(value) for value in argv],
                            f"CLI argv identity mismatch: actual={actual_argv!r} expected={argv!r}")
                    if capture_service:
                        for candidate in process_tree(worker.pid):
                            if candidate[0] == worker.pid:
                                continue
                            child = process_info(candidate[0])
                            if child is None:
                                continue
                            require(child["identity"] == candidate and candidate[2] == str(self.binary),
                                    "service child executable identity mismatch")
                            require(child["parent"] in (worker.pid, os.getpid())
                                    and child["argv"].split(b"\0")[:-1] == [os.fsencode(self.binary), b"serve"],
                                    "service child ancestry/argv mismatch")
                            require(child["uid"] == os.geteuid(), "service child euid mismatch")
                            require(self.identity in (None, candidate), "service start spawned multiple managers")
                            if self.identity is None:
                                self.identity = candidate
                                self.owned.add(candidate)
                                print(f"REGISTER service-supervisor={candidate!r}", flush=True)
                if worker.poll() is not None:
                    break
                require(time.monotonic() < deadline, f"CLI timed out: {arguments!r}")
                time.sleep(0.01)
            stdout, stderr = worker.communicate(timeout=2)
            self.no_token_leak(stdout + stderr)
            return {"code": worker.returncode, "stdout": stdout, "stderr": stderr, "pid": worker.pid}
        finally:
            if worker.poll() is None:
                require(identity is not None, "cannot signal an unregistered CLI process")
                owned_signal(identity, signal.SIGKILL)
                worker.wait(timeout=5)
            worker.stdout.close()
            worker.stderr.close()

    def start_service(self):
        require(self.process is None and self.identity is None, "fixture already running")
        for reservation in self.ports[:2]:
            reservation.close()
        result = self.run_cli("service", "start", capture_service=True)
        require(result["code"] == 0, f"service start failed: {result}\n{self.diagnostics()}")
        require(self.identity is not None and alive(self.identity), "service start did not leave a verified manager")
        require(f"started (pid {self.identity[0]})" in result["stdout"], "service start did not confirm HTTP readiness")
        self.check_private()
        time.sleep(0.1)

    def start(self):
        require(self.process is None, "fixture already running")
        for reservation in self.ports[:2]:
            reservation.close()
        self.log = self.log_path.open("ab")
        self.process = subprocess.Popen([str(self.binary), "serve"], env=self.env, cwd=self.root,
                                        stdout=self.log, stderr=self.log, start_new_session=True)

        def capture():
            if self.process.poll() is not None:
                raise AssertionError(f"supervisor exited {self.process.returncode}: {self.diagnostics()}")
            info = process_info(self.process.pid)
            if info is None or info["identity"][2] != str(self.binary):
                return False
            self.identity = info["identity"]
            if self.identity not in self.owned:
                print(f"REGISTER supervisor={self.identity!r}", flush=True)
            self.owned.add(self.identity)
            if not info["argv"]:
                return False
            require(info["argv"].split(b"\0")[0] == os.fsencode(self.binary),
                    f"supervisor argv identity mismatch: {info['argv']!r}")
            return True

        wait_for(capture, "capture supervisor identity")

        def ready():
            if self.process.poll() is not None:
                raise AssertionError(f"supervisor exited {self.process.returncode}: {self.diagnostics()}")
            try:
                response = self.request(API + "/status")
            except (OSError, http.client.HTTPException):
                return False
            require(response[0] == 200, f"gateway readiness HTTP {response[0]}: {response[2]!r}")
            return True

        try:
            wait_for(ready, "real Gateway UDS HTTP readiness (not TCP/PID readiness)")
        except Exception as error:
            raise AssertionError(f"{error}\n{self.diagnostics()}") from error
        self.check_private()
        time.sleep(0.1)

    def expect_start_failure(self, timeout=5):
        require(self.process is None and self.identity is None, "fixture already running")
        for reservation in self.ports[:2]:
            reservation.close()
        self.log = self.log_path.open("ab")
        self.process = subprocess.Popen([str(self.binary), "serve"], env=self.env, cwd=self.root,
                                        stdout=self.log, stderr=self.log, start_new_session=True)
        deadline = time.monotonic() + timeout
        timed_out = False
        while self.process.poll() is None:
            info = process_info(self.process.pid)
            if info is not None and info["state"] not in ("Z", "X"):
                require(info["identity"][2] == str(self.binary),
                        f"failed-start executable identity mismatch: {info['identity']!r}")
                require(info["uid"] == os.geteuid(), "failed-start euid identity mismatch")
                if self.identity is None:
                    self.identity = info["identity"]
                    self.owned.add(self.identity)
                    print(f"REGISTER expected-failure-supervisor={self.identity!r}", flush=True)
                require(info["identity"] == self.identity, "failed-start process identity changed")
                if info["argv"]:
                    argv = info["argv"].split(b"\0")
                    if argv and argv[-1] == b"":
                        argv.pop()
                    require(argv == [os.fsencode(self.binary), b"serve"],
                            f"failed-start argv identity mismatch: {argv!r}")
            if time.monotonic() >= deadline:
                require(self.identity is not None,
                        "failed-start hung before a verifiable executable identity was captured")
                owned_signal(self.identity, signal.SIGTERM)
                try:
                    self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    current = process_info(self.identity[0])
                    require(current is not None and current["identity"] == self.identity,
                            f"refusing to kill changed failed-start identity: {self.identity!r}")
                    owned_signal(self.identity, signal.SIGKILL)
                    self.process.wait(timeout=5)
                timed_out = True
                break
            time.sleep(0.01)
        returncode = self.process.wait(timeout=5)
        self.process = None
        self.identity = None
        self.log.close()
        self.log = None
        require(not timed_out, f"expected startup rejection hung before exiting: {self.diagnostics()}")
        require(returncode != 0, f"supervisor unexpectedly started successfully: {self.diagnostics()}")
        return returncode

    def check_private(self):
        gateway = self.gateway.lstat()
        control = self.control.lstat()
        require(stat.S_ISSOCK(gateway.st_mode) and stat.S_ISSOCK(control.st_mode), "listeners must be real Unix sockets")
        require((gateway.st_dev, gateway.st_ino) != (control.st_dev, control.st_ino), "gateway reused private control UDS")
        require(stat.S_IMODE(control.st_mode) == 0o600, f"private control mode changed: {oct(control.st_mode)}")
        require(control.st_uid == os.geteuid(), "private control socket owner mismatch")
        require(stat.S_IMODE((self.root / "var/supervisor-token").stat().st_mode) == 0o600, "private token is not 0600")

    def csrf(self):
        response = self.request(PREFIX + "/")
        require(response[0] == 200 and response[2] == PAGE, f"CSRF bootstrap is not the real fixture page: {response!r}")
        cookies = SimpleCookie()
        raw = [value for name, value in response[1] if name.lower() == "set-cookie"]
        for value in raw:
            cookies.load(value)
        candidates = [value for name, value in cookies.items() if "csrf" in name.lower()]
        require(len(candidates) == 1, f"expected one issued CSRF cookie: {raw!r}")
        cookie = candidates[0]
        require(bool(cookie.value) and cookie["samesite"].lower() in ("lax", "strict"), "CSRF cookie lacks token/SameSite protection")
        require(cookie["path"] in ("/", PREFIX, PREFIX + "/"), f"CSRF cookie path misses gateway: {raw!r}")
        token = (self.root / "var/supervisor-token").read_text().strip()
        require(cookie.value != token and token.encode() not in response[2], "browser was given private supervisor token")
        return {"Cookie": f"{cookie.key}={cookie.value}", "X-CSRF-Token": cookie.value,
                "Host": "127.0.0.1", "Origin": "https://nas.gateway.test",
                "Referer": "https://nas.gateway.test/app/qwenpaw/",
                "Sec-Fetch-Site": "same-origin"}

    def remember_tree(self):
        if self.identity is None or not alive(self.identity):
            return
        for identity in process_tree(self.identity[0]):
            require(Path(identity[2]).is_relative_to(self.root), f"unexpected descendant executable: {identity!r}")
            if identity not in self.owned:
                print(f"REGISTER descendant={identity!r}", flush=True)
            self.owned.add(identity)

    def snapshot(self):
        self.remember_tree()
        tree = process_tree(self.identity[0]) if self.identity is not None and alive(self.identity) else ()
        return filesystem_snapshot(self.root, DIAGNOSTIC_LOGS), tree

    def rejected(self, path, method="GET", headers=None, body=None, identity=ADMIN, status=403):
        before = self.snapshot()
        before_logs = diagnostic_log_snapshot(self.root)
        response = self.request(path, method, headers, body, identity)
        require(response[0] == status, f"{method} {path}: expected {status}, got {response[0]} {response[2][:300]!r}")
        after = self.snapshot()
        after_logs = diagnostic_log_snapshot(self.root)
        require(after == before, f"{method} {path}: rejected request changed files or process tree")
        assert_gateway_log_append_only(before_logs, after_logs)
        self.no_token_leak(b"".join(
            state[1] for _, state in after_logs if state is not None
        ))
        return response

    def diagnostics(self):
        data = "\n".join(f"{name}:\n{(self.root / name).read_text(errors='replace')[-10000:]}"
                         for name in ("process.log", "var/supervisor.log") if (self.root / name).is_file())
        self.no_token_leak(data)
        return data

    def stop(self, crash=False):
        if self.process is None:
            if self.identity is not None:
                require(not crash, "service fixture crash must be explicitly registered")
                identity = self.identity
                self.remember_tree()
                if alive(identity):
                    markers = (int((self.root / "var/supervisor.pid").read_text()),
                               int((self.root / "var/supervisor.pid.starttime").read_text()))
                    require(markers == identity[:2], "refusing service stop with changed registration")
                    result = self.run_cli("service", "stop")
                    require(result["code"] == 0, f"service stop failed: {result}")
                wait_for(lambda: not alive(identity), "service supervisor exit", timeout=5)
                pid, status = os.waitpid(identity[0], 0)
                require(pid == identity[0] and os.waitstatus_to_exitcode(status) == 0,
                        "adopted service supervisor did not exit cleanly")
                self.identity = None
            return
        self.remember_tree()
        if self.process.poll() is None:
            require(self.identity is not None, "cannot signal an unregistered supervisor")
            owned_signal(self.identity, signal.SIGKILL if crash else signal.SIGTERM)
            try:
                self.process.wait(timeout=28 if not crash else 5)
            except subprocess.TimeoutExpired:
                owned_signal(self.identity, signal.SIGKILL)
                self.process.wait(timeout=5)
                raise AssertionError("supervisor shutdown exceeded isolated test budget")
        code = self.process.returncode
        self.process = None
        self.identity = None
        self.log.close()
        self.log = None
        require(code == (-signal.SIGKILL if crash else 0), f"unexpected supervisor exit: {code}\n{self.diagnostics()}")

    def cleanup(self):
        errors = []
        try:
            self.stop()
        except Exception as error:
            errors.append(str(error))
        for identity in sorted(self.owned, reverse=True):
            try:
                if alive(identity):
                    owned_signal(identity, signal.SIGKILL)
                wait_for(lambda identity=identity: not alive(identity), f"owned PID exit {identity!r}", timeout=5)
                try:
                    pid, _ = os.waitpid(identity[0], os.WNOHANG)
                    require(pid == identity[0], "owned child was not reaped")
                except ChildProcessError:
                    pass
            except Exception as error:
                errors.append(str(error))
        if self.process is not None:
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired as error:
                errors.append(str(error))
        if self.log is not None:
            self.log.close()
        for reservation in self.ports:
            reservation.close()
        self.diagnostics()
        require(sha256(self.binary) == self.binary_sha, "fixture supervisor binary changed")
        require(sha256(self.helper) == self.runtime_sha, "fixture runtime binary changed")
        require(json.loads(self.ui.read_text())[".url"]["qwenpaw.console"] == self.console, "test changed Gateway console entry")
        print(f"CLEANUP root={self.root} registered={len(self.owned)} live={[identity for identity in self.owned if alive(identity)]}", flush=True)
        require(not errors, "; ".join(errors))


def mutation_routes(port):
    return [("/start", {}), ("/stop", {}), ("/restart", {}), ("/logs/clear", {}),
            ("/kernel/install", {"version": "2.0.0"}), ("/kernel/uninstall", {}),
            ("/kernel/pin", {"version": "3.1.4"}), ("/kernel/reset-auth", {}),
            ("/port", {"port": port}), ("/settings", {"port": port, "log_level": "debug"}),
            ("/mirror", {"mirror": "https://mirror.example/simple"})]


class GatewayUDSTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        require(os.geteuid() == 0, "peer validation requires UID 0 in an isolated user namespace")
        require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"), "identity-safe cleanup requires Linux pidfds")
        cls.libc = ctypes.CDLL(None, use_errno=True)
        previous = ctypes.c_int()
        require(cls.libc.prctl(37, ctypes.byref(previous), 0, 0, 0) == 0,
                "cannot inspect child-subreaper state")
        require(cls.libc.prctl(36, 1, 0, 0, 0) == 0, "cannot enable identity-safe adopted-child cleanup")
        cls.previous_subreaper = previous.value
        cls.addClassCleanup(lambda: require(
            cls.libc.prctl(36, cls.previous_subreaper, 0, 0, 0) == 0,
            "cannot restore child-subreaper state"))
        for binary in (SUPERVISOR, RUNTIME):
            require(binary.is_file() and os.access(binary, os.X_OK), f"build release binaries first: {binary}")
            with binary.open("rb") as stream:
                require(stream.read(4) == b"\x7fELF", f"not a native release binary: {binary}")
        cls.binary_sha = sha256(SUPERVISOR)
        cls.runtime_sha = sha256(RUNTIME)
        cls.brotli = BrotliCodec()
        print("SCOPE=local release UDS blackbox, root-UID Gateway peer + non-root test-only fallback + strict readiness identity; NOT fnOS identity/session acceptance", flush=True)
        print(f"TEST_SUPERVISOR={SUPERVISOR}\nTEST_SUPERVISOR_SHA256={cls.binary_sha}", flush=True)
        print(f"TEST_RUNTIME_HELPER={RUNTIME}\nTEST_RUNTIME_HELPER_SHA256={cls.runtime_sha}", flush=True)
        print(f"TEST_GATEWAY_PEER_UID={os.geteuid()} gid={os.getegid()} test_fallback_exe={os.readlink('/proc/self/exe')}", flush=True)
        print(f"TEST_UID_MAP={Path('/proc/self/uid_map').read_text().strip()}", flush=True)

    @classmethod
    def tearDownClass(cls):
        require(sha256(SUPERVISOR) == cls.binary_sha, "release supervisor changed during suite; rebuild and rerun")
        require(sha256(RUNTIME) == cls.runtime_sha, "release runtime changed during suite; rebuild and rerun")

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="qpg-")
        self.root = Path(temporary.name).resolve()
        self.addCleanup(temporary.cleanup)
        self.fixture = Fixture(self.root, self.binary_sha, self.runtime_sha, self.brotli)
        self.addCleanup(self.fixture.cleanup)
        print(f"TEST_ROOT={self.root} case={self._testMethodName}", flush=True)

    def assert_gateway_probe(self, f, *, gateway=None, state="ready", error=None):
        before = f.snapshot()
        status = f.run_cli("service", "status", gateway=gateway)
        self.assertEqual(status["code"], 0 if state == "ready" else 1, status)
        self.assertIn(f"management service ready (pid {f.identity[0]})", status["stdout"])
        self.assertIn(f"gateway {state}", status["stdout"])
        doctor = f.run_cli("doctor", "--json", gateway=gateway)
        self.assertEqual(doctor["code"], 0, doctor)
        report = json.loads(doctor["stdout"])
        self.assertEqual(report["manager"]["state"], "ready")
        self.assertEqual((report["manager"]["pid"], report["manager"]["starttime"]), f.identity[:2])
        self.assertEqual(report["gateway"]["state"], state, report)
        if error is None:
            self.assertIsNone(report["gateway"]["error"])
        else:
            self.assertIn(error, report["gateway"]["error"])
            self.assertIn(error, status["stdout"])
        self.assertEqual(f.snapshot(), before, "status/doctor changed files or manager descendants")
        return status, doctor

    def test_service_start_status_doctor_readiness(self):
        f = self.fixture
        f.start_service()
        self.assertFalse(f.python.exists())
        identity = f.identity
        self.assert_gateway_probe(f)
        self.assertEqual(f.request(PREFIX + "/")[0], 200)
        f.rejected("/__qwenpaw_gateway_ready", headers={"X-Supervisor-Token": f.token().decode()}, identity=None)
        before = f.snapshot()
        started = f.run_cli("service", "start")
        self.assertEqual(started["code"], 0, started)
        self.assertIn(f"already ready (pid {identity[0]})", started["stdout"])
        self.assertEqual(f.snapshot(), before, "idempotent start changed the manager")
        f.stop()
        self.assertFalse(alive(identity))
        with self.assertRaises(ChildProcessError):
            os.waitpid(identity[0], os.WNOHANG)
        for path in (f.gateway, f.root / "var/supervisor.pid", f.root / "var/supervisor.pid.starttime"):
            self.assertFalse(path.exists(), f"service stop left {path.name}")
        control = f.control.lstat()
        self.assertTrue(stat.S_ISSOCK(control.st_mode), "stop must not unlink the separate private control socket")
        self.assertEqual((stat.S_IMODE(control.st_mode), control.st_uid), (0o600, os.geteuid()))
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as private:
            with self.assertRaises(OSError):
                private.connect(str(f.control))
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", f.sup_port), timeout=1)

    def test_service_start_stop_restart_reconnects_gateway(self):
        f = self.fixture
        sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(180)"],
                                    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.DEVNULL, start_new_session=True)
        wait_for(lambda: process_info(sentinel.pid) is not None, "service-restart sentinel identity")
        sentinel_identity = process_info(sentinel.pid)["identity"]
        print(f"REGISTER service-restart-sentinel={sentinel_identity!r}", flush=True)

        def cleanup_sentinel():
            try:
                self.assertTrue(alive(sentinel_identity), "service stop/restart killed the unrelated sentinel")
            finally:
                owned_signal(sentinel_identity, signal.SIGTERM)
                sentinel.wait(timeout=5)

        self.addCleanup(cleanup_sentinel)

        def assert_stopped(identity):
            self.assertFalse(alive(identity))
            self.assertEqual(service_manager_identities(f.binary), ())
            with self.assertRaises(ChildProcessError):
                os.waitpid(identity[0], os.WNOHANG)
            for path in (f.gateway, f.root / "var/supervisor.pid", f.root / "var/supervisor.pid.starttime"):
                self.assertFalse(path.exists(), f"service stop left {path.name}")
            control = f.control.lstat()
            self.assertTrue(stat.S_ISSOCK(control.st_mode), "stop must retain the private control socket")
            self.assertEqual((stat.S_IMODE(control.st_mode), control.st_uid), (0o600, os.geteuid()))
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as private:
                with self.assertRaises(OSError):
                    private.connect(str(f.control))
            with self.assertRaises(OSError):
                socket.create_connection(("127.0.0.1", f.sup_port), timeout=1)
            self.assertTrue(alive(sentinel_identity), "service stop/restart killed the unrelated sentinel")

        f.start_service()
        first_identity = f.identity
        self.assertEqual(service_manager_identities(f.binary), (first_identity,))
        self.assertTrue(alive(sentinel_identity))
        f.check_private()

        before = f.snapshot()
        started = f.run_cli("service", "start", capture_service=True)
        self.assertEqual(started["code"], 0, started)
        self.assertIn(f"already ready (pid {first_identity[0]})", started["stdout"])
        self.assertEqual(f.identity, first_identity)
        self.assertEqual(f.snapshot(), before, "idempotent start changed the manager")
        self.assertEqual(service_manager_identities(f.binary), (first_identity,),
                         "repeated service start spawned or replaced a manager")
        f.check_private()

        old_connection, status, headers = f.open_event_stream()
        self.addCleanup(old_connection.close)
        self.assertEqual(status, 200)
        self.assertTrue(headers.get("content-type", "").lower().startswith("text/event-stream"), headers)
        self.assertEqual(headers.get("transfer-encoding", "").lower(), "chunked", headers)
        f.stop()
        assert_stopped(first_identity)
        deadline = time.monotonic() + 3
        while True:
            remaining = deadline - time.monotonic()
            self.assertGreater(remaining, 0, "Gateway kept the pre-stop SSE connection alive")
            old_connection.settimeout(remaining)
            try:
                trailing = old_connection.recv(4096)
            except (ConnectionResetError, BrokenPipeError):
                break
            except socket.timeout:
                self.fail("Gateway kept the pre-stop SSE connection alive")
            if not trailing:
                break
            f.no_token_leak(trailing)

        f.start_service()
        second_identity = f.identity
        self.assertNotEqual(second_identity, first_identity, "restart reused the old manager identity")
        self.assertEqual(service_manager_identities(f.binary), (second_identity,))
        f.check_private()
        response = f.request(PREFIX + "/")
        self.assertEqual(response[0], 200)
        self.assertEqual(response[2], PAGE)
        self.assertTrue(alive(sentinel_identity))
        f.stop()
        assert_stopped(second_identity)

    def test_readiness_requires_supervisor_identity_and_rejects_forged_trim_identity(self):
        f = self.fixture
        f.start()
        self.assertEqual(f.request(PREFIX + "/")[0], 200)
        token = f.token().decode()
        cases = [("GET", {}), ("GET", {"X-Supervisor-Token": "forged-internal-token"}),
                 ("GET", {"X-Supervisor-Token": token})]
        for name, value in ADMIN.items():
            cases.append(("GET", {"X-Supervisor-Token": token, name: value}))
        cases.extend((method, dict(ADMIN, **{"X-Supervisor-Token": token}))
                     for method in ("GET", "HEAD", "POST", "OPTIONS"))
        for method, headers in cases:
            with self.subTest(method=method, headers=tuple(headers)):
                response = f.rejected("/__qwenpaw_gateway_ready", method, headers, identity=None)
                self.assertIsNone(header(response, "Set-Cookie"))
                self.assertIsNone(header(response, "X-Supervisor-Token"))
        f.rejected(PREFIX + "/__qwenpaw_gateway_ready", status=404)
        self.assert_gateway_probe(f)

    def test_readiness_wrong_token_from_supervisor_peer_is_not_ready(self):
        f = self.fixture
        f.start()
        self.assert_gateway_probe(f)
        token_file = f.root / "var/supervisor-token"
        original = token_file.read_bytes()
        f.token()
        forged = b"f" * 64
        self.assertNotEqual(original.strip(), forged)
        try:
            token_file.write_bytes(forged + b"\n")
            self.assert_gateway_probe(f, state="not_ready", error="did not return a verified HTTP 200")
            response = http_request(http.client.HTTPConnection("127.0.0.1", f.sup_port, timeout=3),
                                    "/status", headers={"X-Supervisor-Token": original.strip().decode()})
            f.no_token_leak(json.dumps(response[1]).encode() + response[2])
            self.assertTrue(response_json(response)["success"], "manager must stay healthy during wrong-token rejection")
        finally:
            token_file.write_bytes(original)
        self.assert_gateway_probe(f)

    def test_readiness_requires_supervisor_executable_identity(self):
        f = self.fixture
        f.start()
        token = f.token().decode()
        peer = self.root / "helpers/readiness-peer"
        shutil.copy2(Path(os.readlink("/proc/self/exe")), peer)
        request = {"socket": str(f.gateway), "path": "/__qwenpaw_gateway_ready",
                   "method": "GET", "headers": {"X-Supervisor-Token": token}, "body": None}
        worker = subprocess.Popen([str(peer), str(Path(__file__).resolve()), "--peer-request"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            env=dict(f.env, PYTHONHOME=sys.base_prefix), cwd=f.root)
        expected_peer_exe = str(peer)
        try:
            wait_for(lambda: process_info(worker.pid) is not None
                     and process_info(worker.pid)["identity"][2] == expected_peer_exe
                     and bool(process_info(worker.pid)["argv"]),
                     "capture readiness peer identity before probing")
            peer_identity = process_info(worker.pid)["identity"]
            f.owned.add(peer_identity)
            stdout, stderr = worker.communicate(json.dumps(request), timeout=15)
        except subprocess.TimeoutExpired:
            current = process_info(worker.pid)
            require(current is not None and current["identity"] in f.owned,
                    "refusing to signal readiness peer without registered identity")
            owned_signal(current["identity"], signal.SIGKILL)
            worker.wait(timeout=5)
            raise AssertionError(f"readiness peer request timed out: {f.diagnostics()}")
        self.assertEqual(worker.returncode, 0, stderr)
        result = json.loads(stdout)
        self.assertEqual((result["pid"], result["starttime"], result["exe"]), peer_identity)
        self.assertEqual(result["uid"], os.geteuid())
        self.assertEqual(result["status"], 403, result)
        f.no_token_leak(json.dumps(result["headers"]).encode() + result["body"].encode() + stderr.encode())

    def test_readiness_rejects_non_0600_live_socket_without_claiming_ready(self):
        f = self.fixture
        f.start()
        original = stat.S_IMODE(f.gateway.lstat().st_mode)
        self.assertEqual(original, 0o600)
        try:
            for mode in (0o640, 0o666, 0o700, 0o1600):
                with self.subTest(mode=oct(mode)):
                    f.gateway.chmod(mode)
                    self.assert_gateway_probe(f, state="abnormal", error="unsafe gateway probe socket")
        finally:
            f.gateway.chmod(original)
        self.assert_gateway_probe(f)

    def test_readiness_sends_no_bytes_to_same_euid_wrong_socket_peer(self):
        f = self.fixture
        f.start()
        path = f.root / "unexpected-peer.sock"
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(str(path))
            path.chmod(0o600)
            listener.listen(4)
            listener.settimeout(2)
            records = []
            failures = []

            def collect_probes():
                try:
                    for _ in range(2):
                        connection, _ = listener.accept()
                        with connection:
                            connection.settimeout(2)
                            credentials = connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
                            pid, uid, _ = (int.from_bytes(credentials[offset:offset + 4], sys.byteorder, signed=True)
                                           for offset in (0, 4, 8))
                            records.append((pid, uid, connection.recv(4096)))
                except Exception as error:
                    failures.append(error)

            collector = threading.Thread(target=collect_probes, daemon=True)
            collector.start()
            try:
                results = self.assert_gateway_probe(f, gateway=path, state="abnormal",
                                                    error="unexpected supervisor peer")
                collector.join(timeout=3)
                self.assertFalse(collector.is_alive(), "readiness probe did not close both impostor connections")
                self.assertFalse(failures, f"could not observe readiness peer: {failures!r}")
                self.assertEqual(len(records), len(results))
                expected_pids = {result["pid"] for result in results}
                for pid, uid, received in records:
                    self.assertIn(pid, expected_pids, "unexpected client connected to the isolated impostor")
                    expected_pids.remove(pid)
                    self.assertEqual(uid, os.geteuid())
                    f.no_token_leak(received)
                    self.assertEqual(received, b"", "readiness wrote to an unverified listener")
                self.assertFalse(expected_pids)
            finally:
                listener.close()
                collector.join(timeout=3)
        self.assert_gateway_probe(f)

    def test_readiness_unsafe_socket_and_file_placeholder_never_receive_a_probe(self):
        f = self.fixture
        f.start()
        path = f.root / "unsafe-probe.sock"
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(str(path))
            listener.listen(4)
            listener.settimeout(0.1)
            for mode in (0o640, 0o666, 0o1600):
                with self.subTest(mode=oct(mode)):
                    path.chmod(mode)
                    self.assert_gateway_probe(f, gateway=path, state="abnormal",
                                              error="unsafe gateway probe socket")
                    with self.assertRaises(socket.timeout):
                        listener.accept()
        placeholder = f.root / "probe-placeholder"
        placeholder.write_bytes(b"ordinary probe placeholder must remain\n")
        placeholder.chmod(0o600)
        self.assert_gateway_probe(f, gateway=placeholder, state="abnormal",
                                  error="unsafe gateway probe socket")
        self.assertEqual(placeholder.read_bytes(), b"ordinary probe placeholder must remain\n")
        self.assert_gateway_probe(f)

    def test_sse_connections_cannot_starve_management_or_readiness(self):
        f = self.fixture
        f.start()
        streams = []
        try:
            responses = []
            for _ in range(32):
                connection, status, headers = f.open_event_stream()
                responses.append((status, headers))
                if status == 200:
                    streams.append(connection)
                else:
                    connection.close()
            self.assertEqual(sum(status == 200 for status, _ in responses), 8, responses)
            self.assertEqual(sum(status == 503 for status, _ in responses), 24, responses)
            self.assertTrue(all(headers.get("retry-after") == "1"
                                for status, headers in responses[8:]), responses[8:])
            self.assertTrue(response_json(f.request(API + "/status"))["success"])
            self.assert_gateway_probe(f)
            f.rejected("/__qwenpaw_gateway_ready",
                       headers={"X-Supervisor-Token": f.token().decode()}, identity=None)
        finally:
            for connection in streams:
                connection.close()

    def test_no_kernel_page_status_and_private_socket(self):
        f = self.fixture
        f.start()
        self.assertFalse(f.python.exists())
        for path in (PREFIX, PREFIX + "/", PREFIX + "/?from=uds", PREFIX + "/index.html"):
            response = f.request(path)
            self.assertEqual((response[0], response[2]), (200, PAGE), path)
        result = response_json(f.request(API + "/status?case=no-kernel"))
        self.assertTrue(result["success"])
        self.assertFalse(result["installed"])
        self.assertFalse(result["running"])
        f.check_private()

    def test_admin_reads_and_real_kernel_start_stop(self):
        f = self.fixture
        f.install_runtime()
        f.start()
        csrf = f.csrf()
        for path in ("/status", "/logs", "/kernel/install-status", "/kernel/install-log", "/mirror"):
            self.assertEqual(f.request(API + path)[0], 200, path)
        self.assertTrue(response_json(f.request(API + "/start", "POST", csrf, {}))["success"])
        wait_for(lambda: response_json(f.request(API + "/status"))["running"], "real helper kernel running")
        f.remember_tree()
        connection = http.client.HTTPConnection("127.0.0.1", f.port, timeout=3)
        self.assertEqual(response_json(http_request(connection, "/api/version"))["version"], "1.0.0")
        self.assertTrue(response_json(f.request(API + "/stop", "POST", csrf, {}))["success"])
        wait_for(lambda: not response_json(f.request(API + "/status"))["running"], "kernel stopped through gateway")
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", f.port), timeout=1)
        self.assertEqual(f.request(PREFIX + "/")[2], PAGE)
        f.check_private()

    def test_mirror_api_roundtrip_and_rejections_are_side_effect_free(self):
        f = self.fixture
        f.start()
        csrf = f.csrf()
        initial = response_json(f.request(API + "/mirror"))
        self.assertTrue(initial["success"], initial)
        self.assertEqual(initial["mirror"], (f.root / "var/pip-mirror").read_text().splitlines()[0])

        mirror = "https://mirror.example/custom/simple"
        saved = response_json(f.request(API + "/mirror", "POST", csrf, {"mirror": mirror}))
        self.assertTrue(saved["success"], saved)
        self.assertEqual((f.root / "var/pip-mirror").read_text(), mirror + "\n")
        self.assertEqual(response_json(f.request(API + "/mirror"))["mirror"], mirror)

        f.rejected(API + "/mirror", identity=USER)
        f.rejected(API + "/mirror", "POST", csrf, {"mirror": "https://denied.example/simple"}, identity=USER)
        f.rejected(API + "/mirror", "POST", {}, {"mirror": "https://denied.example/simple"})
        f.rejected(API + "/mirror", "PUT", csrf, {"mirror": "https://denied.example/simple"}, status=405)

        before_bytes = (f.root / "var/pip-mirror").read_bytes()
        before = f.snapshot()
        invalid = f.rejected(
            API + "/mirror", "POST", csrf,
            {"mirror": "https://mirror.example/simple;touch invalid"}, status=400)
        self.assertEqual(f.snapshot(), before)
        self.assertEqual((f.root / "var/pip-mirror").read_bytes(), before_bytes)
        self.assertTrue(json.loads(invalid[2])["message"])

    def test_role_matrix_rejects_missing_and_ordinary_identity_without_side_effects(self):
        f = self.fixture
        f.install_runtime()
        (f.root / "var/qwenpaw.log").write_text("logs must survive rejection\n")
        auth = f.root / "share/.qwenpaw.secret/auth.json"
        auth.parent.mkdir(parents=True)
        auth.write_text("isolated-auth-must-survive")
        f.start()
        csrf = f.csrf()
        writes = mutation_routes(f.reserve_port())
        reads = [PREFIX + "/"] + [API + path for path in
            ("/status", "/logs", "/events", "/kernel/install-status", "/kernel/install-log", "/kernel/versions", "/mirror")]
        for identity in ({}, USER):
            for path in reads:
                with self.subTest(identity=identity, path=path):
                    f.rejected(path, identity=identity)
            for path, body in writes:
                with self.subTest(identity=identity, path=path):
                    f.rejected(API + path, "POST", csrf, body, identity=identity)

    def test_forged_trim_headers_do_not_elevate_loopback_or_incomplete_identity(self):
        f = self.fixture
        f.start()
        csrf = f.csrf()
        for identity in ({"X-Trim-Isadmin": "true"},
                         {"X-Trim-Userid": "1000", "X-Trim-Username": "forged"},
                         dict(USER, **{"X-Trim-Isadmin": "false"})):
            f.rejected(API + "/status", identity=identity)
        for path, method, body in (("/status", "GET", None), ("/settings", "POST", {"log_level": "debug"}),
                                   ("/mirror", "GET", None), ("/mirror", "POST", {"mirror": "https://denied.example/simple"})):
            before = f.snapshot()
            headers = dict(ADMIN, **csrf)
            response = http_request(http.client.HTTPConnection("127.0.0.1", f.sup_port, timeout=5),
                                    path, method, headers, body)
            self.assertEqual(response[0], 401, response)
            self.assertEqual(f.snapshot(), before)

    def test_get_head_compression_assets_and_path_traversal(self):
        f = self.fixture
        f.start()
        for accept, expected in (("gzip", "gzip"), ("br", "br"),
                ("gzip;q=0, br;q=0", None), ("br;q=0, gzip;q=1", "gzip")):
            with self.subTest(accept=accept):
                get = f.request(PREFIX + "/", headers={"Accept-Encoding": accept})
                self.assertEqual(get[0], 200)
                self.assertEqual(header(get, "Content-Encoding"), expected)
                self.assertIn("accept-encoding", header(get, "Vary").lower())
                self.assertEqual(header(get, "Content-Type"), "text/html; charset=utf-8")
                self.assertEqual(int(header(get, "Content-Length")), len(get[2]))
                decoded = gzip.decompress(get[2]) if expected == "gzip" else (
                    self.brotli.decompress(get[2], len(PAGE)) if expected == "br" else get[2])
                self.assertEqual(decoded, PAGE)
                head = f.request(PREFIX + "/", "HEAD", {"Accept-Encoding": accept})
                self.assertEqual((head[0], head[2]), (200, b""))
                for name in ("Content-Length", "Content-Encoding", "Content-Type", "Vary"):
                    self.assertEqual(header(head, name), header(get, name), name)
        css = f.request(PREFIX + "/assets/site.css")
        self.assertEqual((css[0], css[2]), (200, CSS))
        self.assertIn("text/css", header(css, "Content-Type"))
        for tail in ("/../secret.txt", "/%2e%2e/secret.txt", "/%252e%252e/secret.txt",
                "/assets/%2e%2e/%2e%2e/secret.txt", "/assets%2fescape.css",
                "/assets/%5cescape.css", "/assets/%00.css", "/assets/%zz.css",
                "/assets/escape.css", "/assets/escape/secret.txt", "/.hidden",
                "/var/supervisor-token", "/assets/missing.css"):
            for method in ("GET", "HEAD"):
                with self.subTest(tail=tail, method=method):
                    response = f.request(PREFIX + tail, method)
                    self.assertIn(response[0], (400, 404), response)
                    self.assertNotIn(SECRET, response[2])
                    self.assertNotIn(str(self.root).encode(), response[2])
                    if method == "HEAD":
                        self.assertEqual(response[2], b"")

    def test_csrf_origin_rejections_and_host_rewrite_success(self):
        f = self.fixture
        f.install_runtime()
        f.start()
        csrf = f.csrf()
        cross_site_without_fetch = dict(csrf, **{"Origin": "https://attacker.invalid"})
        cross_site_without_fetch.pop("Sec-Fetch-Site")
        cross_site_without_fetch.pop("Referer")
        alternate = f.reserve_port()
        for path, body in mutation_routes(alternate):
            for label, values in (("missing", {}),
                    ("cookie only", {"Cookie": csrf["Cookie"]}),
                    ("token only", {"X-CSRF-Token": csrf["X-CSRF-Token"]}),
                    ("wrong token", dict(csrf, **{"X-CSRF-Token": "incorrect-csrf"})),
                    ("forged matching pair", dict(csrf, **{
                        "Cookie": csrf["Cookie"].split("=", 1)[0] + "=forged-pair",
                        "X-CSRF-Token": "forged-pair"})),
                    ("cross-site", dict(csrf, **{"Origin": "https://attacker.invalid", "Sec-Fetch-Site": "cross-site"})),
                    ("cross-site without Fetch metadata or Referer", cross_site_without_fetch)):
                with self.subTest(path=path, csrf=label):
                    f.rejected(API + path, "POST", values, body)
        self.assertIn("[Gateway] forbidden: invalid origin", f.diagnostics())
        self.assertNotIn("attacker.invalid", f.diagnostics())
        self.assertNotEqual(csrf["Host"], csrf["Origin"].split("://", 1)[1])
        response = f.request(API + "/settings", "POST", csrf, {"log_level": "debug"})
        self.assertTrue(response_json(response)["success"], response)
        self.assertEqual((f.root / "var/qwenpaw-log-level").read_text().strip(), "debug")
        forwarded = dict(csrf, **{
            "Origin": "http://nas.gateway.test",
            "X-Forwarded-Host": "nas.gateway.test",
            "X-Forwarded-Proto": "http",
        })
        forwarded.pop("Referer")
        response = f.request(API + "/settings", "POST", forwarded, {"log_level": "info"})
        self.assertTrue(response_json(response)["success"], response)
        self.assertEqual((f.root / "var/qwenpaw-log-level").read_text().strip(), "info")
        malicious_forwarded = dict(forwarded, **{"X-Forwarded-Host": "attacker.invalid"})
        f.rejected(API + "/settings", "POST", malicious_forwarded, {"log_level": "debug"})
        f.remember_tree()
        self.assertTrue(response_json(f.request(API + "/status"))["running"])
        connection = http.client.HTTPConnection("127.0.0.1", f.port, timeout=3)
        self.assertEqual(response_json(http_request(connection, "/api/version"))["version"], "1.0.0")
        f.check_private()

    def assert_failed_start_left_no_manager(self, f):
        self.assertIsNone(f.process, "failed start left a supervisor process registered")
        self.assertIsNone(f.identity, "failed start left a manager identity registered")
        for path in (f.control, f.root / "var/supervisor.pid", f.root / "var/supervisor.pid.starttime",
                     f.root / "var/supervisor-token"):
            self.assertFalse(path.exists(), f"failed start left manager state at {path}")

    def test_regular_file_gateway_placeholder_is_never_overwritten(self):
        f = self.fixture
        f.gateway.write_bytes(b"ordinary-file-must-remain\n")
        before = f.gateway.lstat(), f.gateway.read_bytes()
        self.assertNotEqual(f.expect_start_failure(), 0)
        self.assert_failed_start_left_no_manager(f)
        self.assertEqual(f.gateway.read_bytes(), before[1])
        after = f.gateway.lstat()
        self.assertTrue(stat.S_ISREG(after.st_mode))
        self.assertEqual((after.st_dev, after.st_ino, after.st_mode, after.st_mtime_ns),
                         (before[0].st_dev, before[0].st_ino, before[0].st_mode, before[0].st_mtime_ns))

    def test_independently_stale_gateway_socket_is_reclaimed_without_crash(self):
        f = self.fixture
        stale = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        stale.bind(str(f.gateway))
        f.gateway.chmod(0o600)
        original = f.gateway.lstat()
        old_inode = os.open(f.gateway, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC)
        stale.close()
        try:
            f.start()
            current = f.gateway.lstat()
            self.assertTrue(stat.S_ISSOCK(current.st_mode))
            self.assertEqual(current.st_uid, os.geteuid())
            self.assertNotEqual((current.st_dev, current.st_ino), (original.st_dev, original.st_ino))
            self.assertEqual(response_json(f.request(API + "/status"))["success"], True)
            f.stop()
            self.assertFalse(f.gateway.exists(), "graceful stop left the reclaimed Gateway socket")
        finally:
            os.close(old_inode)

    def test_active_gateway_socket_is_rejected_and_remains_active(self):
        f = self.fixture
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(str(f.gateway))
            f.gateway.chmod(0o600)
            listener.listen(4)
            original = f.gateway.lstat()
            self.assertNotEqual(f.expect_start_failure(), 0)
            self.assert_failed_start_left_no_manager(f)
            current = f.gateway.lstat()
            self.assertTrue(stat.S_ISSOCK(current.st_mode))
            self.assertEqual((current.st_dev, current.st_ino, current.st_uid),
                             (original.st_dev, original.st_ino, original.st_uid))
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                client.connect(str(f.gateway))

    def test_gateway_symlink_placeholder_is_never_overwritten(self):
        f = self.fixture
        target = f.root / "symlink-sentinel"
        target.write_bytes(b"symlink target must remain\n")
        f.gateway.symlink_to(target)
        original = f.gateway.lstat()
        self.assertNotEqual(f.expect_start_failure(), 0)
        self.assert_failed_start_left_no_manager(f)
        current = f.gateway.lstat()
        self.assertTrue(stat.S_ISLNK(current.st_mode))
        self.assertEqual((current.st_dev, current.st_ino), (original.st_dev, original.st_ino))
        self.assertEqual(os.readlink(f.gateway), str(target))
        self.assertEqual(target.read_bytes(), b"symlink target must remain\n")

    def test_unknown_owner_stale_gateway_socket_is_rejected_when_chown_is_available(self):
        f = self.fixture
        stale = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        stale.bind(str(f.gateway))
        f.gateway.chmod(0o600)
        stale.close()
        original = f.gateway.lstat()
        unknown_uid = 65534 if os.geteuid() != 65534 else 65533
        try:
            os.chown(f.gateway, unknown_uid, -1)
        except OSError as error:
            self.skipTest(f"platform cannot create a socket with an unmapped foreign owner: {error}")
        original = f.gateway.lstat()
        self.assertNotEqual(original.st_uid, os.geteuid())
        self.assertNotEqual(f.expect_start_failure(), 0)
        self.assert_failed_start_left_no_manager(f)
        current = f.gateway.lstat()
        self.assertTrue(stat.S_ISSOCK(current.st_mode))
        self.assertEqual((current.st_dev, current.st_ino, current.st_uid),
                         (original.st_dev, original.st_ino, original.st_uid))

    def test_orphaned_kernel_is_adopted_after_supervisor_sigkill(self):
        f = self.fixture
        f.install_runtime()
        sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(180)"], start_new_session=True)
        wait_for(lambda: process_info(sentinel.pid) is not None, "orphan-adoption sentinel identity")
        sentinel_identity = process_info(sentinel.pid)["identity"]
        print(f"REGISTER orphan-adoption-sentinel={sentinel_identity!r}", flush=True)

        def cleanup_sentinel():
            try:
                self.assertTrue(alive(sentinel_identity), "unrelated sentinel was killed during kernel adoption")
            finally:
                owned_signal(sentinel_identity, signal.SIGTERM)
                sentinel.wait(timeout=5)

        self.addCleanup(cleanup_sentinel)
        f.start()
        csrf = f.csrf()
        started = f.request(API + "/start", "POST", csrf, {})
        self.assertTrue(response_json(started)["success"], started)
        wait_for(lambda: response_json(f.request(API + "/status"))["running"], "real kernel before supervisor crash")
        initial_status = response_json(f.request(API + "/status"))
        kernel_pid = int(initial_status["pid"])
        kernel_info = process_info(kernel_pid)
        self.assertIsNotNone(kernel_info, "running kernel PID was not visible in /proc")
        kernel_identity = kernel_info["identity"]
        self.assertEqual(kernel_info["uid"], os.geteuid())
        self.assertTrue(Path(kernel_identity[2]).is_relative_to(f.root), kernel_identity)
        f.remember_tree()
        self.assertIn(kernel_identity, f.owned)
        old_supervisor = f.identity

        f.stop(crash=True)
        self.assertFalse(alive(old_supervisor))
        self.assertTrue(alive(kernel_identity), "SIGKILL terminated the orphaned kernel")
        self.assertTrue(alive(sentinel_identity), "SIGKILL terminated the unrelated sentinel")

        f.start()
        supervisor_log = (f.root / "var/supervisor.log").read_text(errors="replace")
        self.assertIn(f"adopted running kernel pid={kernel_pid}", supervisor_log)
        f.remember_tree()
        recovered = response_json(f.request(API + "/status"))
        self.assertTrue(recovered["running"], recovered)
        self.assertEqual(int(recovered["pid"]), kernel_pid)
        self.assertEqual(process_info(kernel_pid)["identity"], kernel_identity)
        self.assertTrue(alive(sentinel_identity))
        f.stop()
        self.assertFalse(alive(kernel_identity), "graceful stop left the adopted kernel running")
        self.assertTrue(alive(sentinel_identity), "graceful stop terminated the unrelated sentinel")

    def test_sigkill_safe_socket_rebuild_and_unrelated_sentinel_survives(self):
        f = self.fixture
        sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(180)"], start_new_session=True)
        wait_for(lambda: process_info(sentinel.pid) is not None, "sentinel identity")
        identity = process_info(sentinel.pid)["identity"]
        print(f"REGISTER unrelated-sentinel={identity!r}", flush=True)

        def cleanup_sentinel():
            try:
                self.assertTrue(alive(identity), "unrelated sentinel was killed by supervisor")
            finally:
                owned_signal(identity, signal.SIGTERM)
                sentinel.wait(timeout=5)
            print("CLEANUP unrelated sentinel survived manager lifecycle and was reaped", flush=True)

        self.addCleanup(cleanup_sentinel)
        f.start()
        old_stat = f.gateway.lstat()
        old_identity = f.identity
        old_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        old_socket.settimeout(1)
        old_socket.connect(str(f.gateway))
        self.addCleanup(old_socket.close)
        f.stop(crash=True)
        self.assertFalse(alive(old_identity))
        stale = f.gateway.lstat()
        self.assertTrue(stat.S_ISSOCK(stale.st_mode))
        self.assertEqual((stale.st_dev, stale.st_ino), (old_stat.st_dev, old_stat.st_ino))
        with self.assertRaises(OSError):
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.connect(str(f.gateway))
        f.start()
        self.assertNotEqual(f.identity, old_identity)
        self.assertEqual(f.request(PREFIX + "/")[2], PAGE)
        self.assertTrue(response_json(f.request(API + "/status"))["success"])
        f.check_private()
        self.assertTrue(alive(identity))
        f.stop()
        self.assertFalse(f.gateway.exists(), "graceful stop left its gateway socket behind")
        self.assertTrue(alive(identity))


class GatewayPermissionTests(unittest.TestCase):
    def test_nonroot_cannot_bind_gateway_in_unwritable_directory(self):
        if os.geteuid() == 0:
            self.skipTest("permission-denied acceptance requires the outer non-root UID")
        for binary in (SUPERVISOR, RUNTIME):
            require(binary.is_file() and os.access(binary, os.X_OK), f"build release binaries first: {binary}")
            with binary.open("rb") as stream:
                require(stream.read(4) == b"\x7fELF", f"not a native release binary: {binary}")
        binary_sha = sha256(SUPERVISOR)
        runtime_sha = sha256(RUNTIME)
        temporary = tempfile.TemporaryDirectory(prefix="qpg-permission-")
        root = Path(temporary.name).resolve()
        fixture = Fixture(root, binary_sha, runtime_sha, BrotliCodec())
        gateway_dir = root / "gateway-dir"
        gateway_dir.mkdir()
        fixture.gateway = gateway_dir / "gateway.sock"
        fixture.env["QWENPAW_GATEWAY_SOCKET"] = str(fixture.gateway)
        (root / "var/supervisor.lock").write_bytes(b"")
        os.chmod(root / "var/supervisor.lock", 0o600)
        print(f"PERMISSION_SCOPE=outer non-root UID {os.geteuid()} using release supervisor SHA256={binary_sha}",
              flush=True)
        try:
            gateway_dir.chmod(0o500)
            self.assertNotEqual(fixture.expect_start_failure(), 0)
            diagnostics = fixture.diagnostics()
            self.assertIn("bind gateway socket", diagnostics)
            self.assertIn("Permission denied", diagnostics)
            for path in (fixture.gateway, fixture.control, root / "var/supervisor.pid",
                         root / "var/supervisor.pid.starttime", root / "var/supervisor-token"):
                self.assertFalse(path.exists(), f"permission-denied start left manager state at {path}")
        finally:
            gateway_dir.chmod(0o700)
            fixture.cleanup()
            temporary.cleanup()


def peer_request():
    request = json.load(sys.stdin)
    response = http_request(UnixHTTPConnection(request["socket"]), request["path"],
                            request["method"], request["headers"], request["body"])
    info = process_info(os.getpid())
    require(info is not None and info["uid"] == os.geteuid(), "wrong-peer helper identity changed")
    print(json.dumps({"status": response[0], "headers": response[1],
                      "body": base64.b64encode(response[2]).decode(), "pid": os.getpid(),
                      "starttime": info["identity"][1], "exe": os.readlink("/proc/self/exe"),
                      "uid": os.geteuid()}))


def main():
    if sys.argv[1:] == ["--peer-request"]:
        peer_request()
        return
    arguments = sys.argv[1:]
    if arguments == ["--permission-only"]:
        unittest.main(argv=[sys.argv[0],
                            "GatewayPermissionTests.test_nonroot_cannot_bind_gateway_in_unwritable_directory"],
                      verbosity=2)
        return
    mapped = "--isolated-uid" in arguments
    if mapped:
        arguments.remove("--isolated-uid")
    if os.geteuid() != 0:
        require(not mapped, "user namespace did not map the test UID to 0")
        unshare = shutil.which("unshare")
        require(unshare is not None, "non-root test needs unshare --user --map-root-user (never sudo)")
        print(f"ISOLATION=unprivileged user namespace, outer uid={os.geteuid()} gid={os.getegid()}; no host account changes", flush=True)
        os.execv(unshare, [unshare, "--user", "--map-root-user", sys.executable,
                           str(Path(__file__).resolve()), "--isolated-uid", *arguments])
    unittest.main(argv=[sys.argv[0], *arguments], verbosity=2)


if __name__ == "__main__":
    main()
