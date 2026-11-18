#!/usr/bin/env python3
"""Read-only, standard-library HTTP benchmark. Results contain no request secrets/body.

Paths: UTF-8 file, one origin-relative GET path per line (blank/# lines ignored).
Headers: existing external JSON object of HTTP header names and string values.
Count mode runs requests per round; duration mode repeats batches until its deadline,
with requests as the minimum total. Interval is the minimum batch start spacing and
proc sampling interval. All redirects are rejected, including same-origin redirects.
Static bodies may be capped; --expect-json requires complete identity/gzip JSON.
Issuing stops at the deadline; in-flight requests drain within request-timeout.
Limits: clients<=128, requests<=1e6/batch, duration<=604800s, timeouts<=60s,
body<=64MiB; latency percentiles use a bounded 1ms histogram (overflow 60001ms).
"""
import argparse
from collections import Counter
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
import gzip
import hashlib
import http.client
import io
import json
import math
import os
from pathlib import Path
import queue
import re
import socket
import threading
import time
import zlib
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]

def external(path):
    resolved = Path(path).resolve()
    if resolved.is_relative_to(ROOT):
        raise ValueError("file must be outside the repository")
    return resolved

def load_file(path, limit=1024 * 1024):
    with Path(path).open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError("input file exceeds limit")
    return data.decode("utf-8")

def target(base, paths_file):
    url = urlsplit(base)
    if (url.scheme not in ("http", "https") or not url.hostname or url.username is not None
            or url.password is not None or url.path not in ("", "/") or url.query or url.fragment
            or any(ord(c) <= 32 or ord(c) == 127 for c in base) or "\\" in base):
        raise ValueError("base URL must be an HTTP(S) origin without credentials/path/query")
    port = url.port if url.port is not None else (443 if url.scheme == "https" else 80)
    if port == 0:
        raise ValueError("base URL port must be positive")
    paths = [line.strip() for line in load_file(paths_file).splitlines()
             if line.strip() and not line.lstrip().startswith("#")]
    if not 1 <= len(paths) <= 256:
        raise ValueError("paths file must contain 1..256 GET paths")
    for path in paths:
        parsed = urlsplit(path)
        decoded = unquote(path)
        if (not path.startswith("/") or path.startswith("//") or parsed.scheme or parsed.netloc
                or parsed.fragment or "\\" in decoded or decoded.startswith("//")
                or any(ord(c) <= 32 or ord(c) >= 127 for c in path)
                or any(ord(c) < 32 or ord(c) == 127 for c in decoded)):
            raise ValueError("paths must be ASCII origin-relative GET targets without fragments")
    host = f"[{url.hostname}]" if ":" in url.hostname else url.hostname
    return url.scheme, url.hostname, port, f"{url.scheme}://{host}:{port}", paths

def resolve_host(host, port, timeout):
    # getaddrinfo has no stdlib timeout. At most one daemon resolver is allowed to linger.
    result = queue.Queue(maxsize=1)
    def lookup():
        try:
            result.put(socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)[:8])
        except OSError:
            result.put(None)
    threading.Thread(target=lookup, daemon=True).start()
    try:
        addresses = result.get(timeout=timeout)
    except queue.Empty:
        raise ValueError("DNS preflight timed out") from None
    if not addresses:
        raise ValueError("DNS preflight failed")
    return addresses

def headers_file(path):
    if not path:
        return {}
    headers = json.loads(load_file(external(path), 65536))
    forbidden = {"host", "connection", "content-length", "transfer-encoding", "expect",
                 "proxy-authorization", "x-http-method-override", "x-method-override"}
    if not isinstance(headers, dict) or len(headers) > 64:
        raise ValueError("header file must be a JSON object with at most 64 headers")
    names = set()
    for name, value in headers.items():
        if (not re.fullmatch(r"[!#$%&'*+.^_`|~0-9A-Za-z-]+", name) or name.lower() in forbidden
                or name.lower() in names or not isinstance(value, str)
                or any(ord(c) < 32 or ord(c) > 255 or ord(c) == 127 for c in value)):
            raise ValueError("invalid or unsupported request header")
        names.add(name.lower())
    if sum(len(k) + len(v) for k, v in headers.items()) > 16384:
        raise ValueError("request headers exceed limit")
    return headers

class DeadlineReader(io.RawIOBase):
    def __init__(self, sock, deadline, timeout):
        self.sock, self.deadline, self.timeout = sock, deadline, timeout
        self.raw = sock.makefile("rb", buffering=0)

    def readable(self):
        return True

    def close(self):
        self.raw.close()
        super().close()

    def readinto(self, buffer):
        remaining = self.deadline - time.perf_counter()
        if remaining <= 0:
            raise TimeoutError()
        self.sock.settimeout(min(self.timeout, remaining))
        return self.raw.readinto(buffer)

class DeadlineSocket:
    def __init__(self, sock, deadline, timeout):
        self.sock, self.deadline, self.timeout = sock, deadline, timeout

    def __getattr__(self, name):
        return getattr(self.sock, name)

    def makefile(self, *_args, **_kwargs):
        return io.BufferedReader(DeadlineReader(self.sock, self.deadline, self.timeout))

    def sendall(self, data):
        self.sock.settimeout(min(self.timeout, max(0.000001, self.deadline - time.perf_counter())))
        self.sock.sendall(data)

def cache_summary(value):
    directives = []
    for part in value.lower().split(",")[:32]:
        part = part.strip()
        if re.fullmatch(r"(?:s-maxage|max-age|stale-while-revalidate|stale-if-error)=\d{1,10}", part):
            directives.append(part)
        elif part.split("=", 1)[0] in {"public", "private", "no-cache", "no-store", "immutable", "no-transform",
                                      "must-revalidate", "proxy-revalidate"}:
            directives.append(part.split("=", 1)[0])
        elif part:
            directives.append("other")
    return ",".join(sorted(set(directives))) or "absent"

def fetch(address, path, headers, args, deadline):
    started = time.perf_counter()
    result = dict(status=None, error=None, transfer_bytes=0, capped=False,
                  content_encoding="absent", cache_control="absent")
    end = min(deadline, started + args.request_timeout)
    connection = None
    try:
        if end <= started:
            raise TimeoutError()
        kind = http.client.HTTPSConnection if address[0] == "https" else http.client.HTTPConnection
        connection = kind(address[1], address[2], timeout=min(args.connect_timeout, end - started))
        def connect_numeric(_address, timeout, *_args):
            connect_end, last = min(end, time.perf_counter() + timeout), None
            for family, socktype, protocol, _canonname, sockaddr in address[5]:
                sock = socket.socket(family, socktype, protocol)
                try:
                    remaining = connect_end - time.perf_counter()
                    if remaining <= 0:
                        raise TimeoutError()
                    sock.settimeout(remaining)
                    sock.connect(sockaddr)
                    sock.settimeout(min(args.read_timeout, max(.000001, end - time.perf_counter())))
                    return sock
                except OSError as exc:
                    sock.close()
                    last = exc
            raise last or OSError()
        connection._create_connection = connect_numeric
        connection.connect()
        connection.sock.settimeout(min(args.read_timeout, max(0.000001, end - time.perf_counter())))
        connection.sock = DeadlineSocket(connection.sock, end, args.read_timeout)
        connection.request("GET", path, headers={**headers, "Connection": "close"})
        with connection.getresponse() as response:
            result["status"] = response.status
            encoding = (response.getheader("Content-Encoding") or "absent").lower().strip()
            result["content_encoding"] = encoding if encoding in {"absent", "identity", "gzip", "br", "deflate"} else "other"
            result["cache_control"] = cache_summary(response.getheader("Cache-Control") or "")
            if response.status in (301, 302, 303, 307, 308):
                result["error"] = "redirect_refused"
            elif response.status not in args.expect_status:
                result["error"] = "http_status"
            chunks = []
            while result["transfer_bytes"] <= args.max_body_bytes:
                chunk = response.read1(min(65536, args.max_body_bytes + 1 - result["transfer_bytes"]))
                if not chunk:
                    break
                result["transfer_bytes"] += len(chunk)
                if args.expect_json:
                    chunks.append(chunk)
            result["capped"] = result["transfer_bytes"] > args.max_body_bytes
            if not result["capped"] and response.length not in (None, 0):
                result["error"] = result["error"] or "incomplete_body"
            if args.expect_json and not result["error"]:
                if result["capped"]:
                    raise ValueError()
                body = b"".join(chunks)
                if encoding == "gzip":
                    with gzip.GzipFile(fileobj=io.BytesIO(body)) as zipped:
                        body = zipped.read(args.max_body_bytes + 1)
                if encoding not in ("absent", "identity", "gzip") or len(body) > args.max_body_bytes:
                    raise ValueError()
                json.loads(body, parse_constant=lambda _value: (_ for _ in ()).throw(ValueError()))
            if time.perf_counter() > end:
                raise TimeoutError()
    except TimeoutError:
        result["error"] = result["error"] or "timeout"
    except (ValueError, UnicodeError, RecursionError, EOFError, gzip.BadGzipFile, zlib.error):
        result["error"] = result["error"] or "invalid_json"
    except (OSError, http.client.HTTPException):
        result["error"] = result["error"] or "transport"
    finally:
        if connection:
            connection.close()
    result["latency_ms"] = (time.perf_counter() - started) * 1000
    return result

class ProcSampler:
    """Read only one PID, never descendants; root/tick/page injection supports fixtures."""
    def __init__(self, pid, root=Path("/proc"), ticks=None, page_size=None):
        self.pid, self.path, self.identity = pid, Path(root) / str(pid), None
        self.ticks = ticks if ticks is not None else os.sysconf("SC_CLK_TCK")
        self.page_size = page_size if page_size is not None else os.sysconf("SC_PAGE_SIZE")
        if self.ticks <= 0 or self.page_size <= 0:
            raise ValueError("invalid proc units")

    def stat(self):
        raw = load_file(self.path / "stat", 16384)
        prefix, tail = raw.rsplit(") ", 1)
        fields = tail.split()
        exe = (self.path / "exe").stat()
        identity = (int(prefix.split(" (", 1)[0]), int(fields[19]), exe.st_dev, exe.st_ino, exe.st_size, exe.st_mtime_ns)
        if identity[0] != self.pid or fields[0] in {"Z", "X", "x"}:
            raise ValueError("invalid process identity")
        return identity, fields

    def sample(self):
        identity, fields = self.stat()
        if self.identity is not None and self.identity != identity:
            raise ValueError("process identity changed")
        io_fields = dict(line.split(":", 1) for line in load_file(self.path / "io", 65536).splitlines())
        sample = dict(at_seconds=time.perf_counter(), cpu_ticks=int(fields[11]) + int(fields[12]),
                      rss_bytes=int(fields[21]) * self.page_size, threads=int(fields[17]),
                      fd=sum(1 for _ in (self.path / "fd").iterdir()), rchar=int(io_fields["rchar"]))
        if self.stat()[0] != identity:
            raise ValueError("process identity changed")
        if any(v < 0 for v in sample.values()):
            raise ValueError("invalid process counters")
        previous = getattr(self, "previous", None)
        if previous and any(sample[k] < previous[k] for k in ("cpu_ticks", "rchar")):
            raise ValueError("process counters decreased")
        sample["at_seconds"] = time.perf_counter()
        self.identity, self.previous = identity, sample
        return sample

def add(stats, item):
    stats["completed"] += 1
    stats["failed"] += bool(item["error"])
    stats["transfer_bytes"] += item["transfer_bytes"]
    stats["capped_responses"] += item["capped"]
    stats["latency_ms"][min(60001, math.ceil(item["latency_ms"]))] += 1
    for key, value in (("errors", item["error"]), ("statuses", str(item["status"])),
                       ("content_encoding", item["content_encoding"]), ("cache_control", item["cache_control"])):
        if value is not None:
            stats[key][value if value in stats[key] or len(stats[key]) < 32 else "other"] += 1

def summary(stats, wall):
    cumulative, percentiles = 0, {}
    for value, count in sorted(stats["latency_ms"].items()):
        cumulative += count
        for percentile in (50, 95, 99):
            if cumulative >= math.ceil(stats["completed"] * percentile / 100):
                percentiles.setdefault(f"p{percentile}_ms", value)
    return {**{k: v for k, v in stats.items() if k != "latency_ms"}, **percentiles,
            "wall_seconds": wall, "requests_per_second": stats["completed"] / wall if wall else 0,
            "latency_resolution_ms": 1, "latency_overflow_ms": 60001,
            "transfer_note": "encoded body bytes read, excludes headers; capped responses are lower bounds"}

def benchmark(args, sampler=None):
    address = target(args.base_url, args.paths_file)
    headers = headers_file(args.header_file)
    output = external(args.output)
    if not output.parent.is_dir() or output.exists():
        raise ValueError("output parent must exist and output must be new")
    baseline = json.loads(load_file(args.baseline_file)) if args.baseline_file else None
    if baseline is not None and (not isinstance(baseline, dict) or baseline.get("schema_version") != 1
                                 or not isinstance(baseline.get("summary"), dict)):
        raise ValueError("unsupported baseline report")
    config = {k: getattr(args, k) for k in ("clients", "requests", "rounds", "duration", "interval",
              "connect_timeout", "read_timeout", "request_timeout", "max_body_bytes", "expect_json", "expect_status",
              "max_p95_ms", "max_p99_ms")}
    config.update(path_count=len(address[4]), paths_sha256=hashlib.sha256("\n".join(address[4]).encode()).hexdigest(),
                  headers_supplied=bool(headers))
    report = dict(schema_version=1, passed=False, target_origin=address[3], config=config,
                  preflight={"completed": 0, "failed": 0, "errors": Counter()}, failures=[], server_process=None)
    stats = dict(completed=0, failed=0, transfer_bytes=0, capped_responses=0,
                 **{k: Counter() for k in ("latency_ms", "errors", "statuses", "content_encoding", "cache_control")})
    started, client_started, trace, stage = time.perf_counter(), time.process_time(), [], "preflight"
    try:
        if args.pid and sampler is None:
            sampler = ProcSampler(args.pid)
        if sampler:
            sampler.sample()  # Identity and permission preflight, before any HTTP request.
        address = (*address, resolve_host(address[1], address[2], min(args.connect_timeout, args.request_timeout)))
        preflight_deadline = time.perf_counter() + 30
        for path in address[4]:
            probe = fetch(address, path, headers, args, preflight_deadline)
            report["preflight"]["completed"] += 1
            if probe["error"]:
                report["preflight"]["failed"] += 1
                report["preflight"]["errors"][probe["error"]] += 1
                raise ValueError("HTTP preflight failed")
        stage = "process_sampling"
        started, client_started = time.perf_counter(), time.process_time()
        deadline = started + (args.duration if args.duration is not None else 300)
        if sampler:
            trace.append(sampler.sample())
        next_sample, rounds, issued = started + args.interval, 0, 0
        with ThreadPoolExecutor(max_workers=args.clients) as pool:
            while time.perf_counter() < deadline and (args.duration is not None or rounds < args.rounds):
                round_start, batch_issued, pending = time.perf_counter(), 0, set()
                while pending or (batch_issued < args.requests and time.perf_counter() < deadline):
                    while len(pending) < args.clients and batch_issued < args.requests and time.perf_counter() < deadline:
                        pending.add(pool.submit(fetch, address, address[4][issued % len(address[4])], headers, args, float("inf")))
                        issued, batch_issued = issued + 1, batch_issued + 1
                    done, pending = wait(pending, timeout=min(args.interval, 1), return_when=FIRST_COMPLETED)
                    for future in done:
                        add(stats, future.result())
                    if sampler and time.perf_counter() >= next_sample:
                        trace.append(sampler.sample())
                        if len(trace) > 2048:
                            trace[1:] = trace[1::2]  # Preserve the initial sample; cap the recorded curve.
                        next_sample = time.perf_counter() + args.interval
                rounds += int(batch_issued == args.requests)
                if args.duration is not None or rounds < args.rounds:
                    time.sleep(max(0, min(round_start + args.interval, deadline) - time.perf_counter()))
        if sampler:
            trace.append(sampler.sample())
        report["rounds_completed"] = rounds
    except (OSError, ValueError, IndexError, KeyError, AttributeError) as exc:
        # Exception text may contain credentials, paths or server data: emit only our fixed categories.
        report["failures"].append(stage + "_failed")
        if isinstance(exc, ValueError) and str(exc) == "process identity changed":
            report["failures"].append("process_identity_changed")
    wall = time.perf_counter() - started
    report["summary"] = summary(stats, wall)
    report["client_cpu_seconds"] = time.process_time() - client_started
    report["client_cpu_note"] = "benchmark process only; not server CPU (in-process fixtures may share it)"
    if sampler and len(trace) >= 2:
        first, last = trace[0], trace[-1]
        elapsed, ticks = last["at_seconds"] - first["at_seconds"], last["cpu_ticks"] - first["cpu_ticks"]
        invalid = any(b[key] < a[key] for a, b in zip(trace, trace[1:]) for key in ("cpu_ticks", "rchar"))
        if invalid or elapsed <= 0:
            report["failures"].append("process_counters_invalid")
        else:
            report["server_process"] = dict(pid=sampler.pid, starttime_ticks=sampler.identity[1],
                clock_ticks_per_second=sampler.ticks, cpu_seconds=ticks / sampler.ticks,
                cpu_percent_one_core=100 * ticks / sampler.ticks / elapsed, wall_seconds=elapsed,
                rchar_delta=last["rchar"] - first["rchar"], rchar_note="logical bytes read, not physical disk IO",
                samples=[{**point, "at_seconds": point["at_seconds"] - first["at_seconds"]} for point in trace])
    minimum = args.requests * (args.rounds if args.duration is None else 1)
    for condition, failure in ((not stats["completed"], "zero_samples"), (stats["completed"] < minimum, "requests_not_reached"),
            (stats["failed"] > 0, "request_failures"), (report["summary"].get("p95_ms", 0) > args.max_p95_ms, "p95_budget"),
            (report["summary"].get("p99_ms", 0) > args.max_p99_ms, "p99_budget")):
        if condition:
            report["failures"].append(failure)
    report["passed"] = not report["failures"]
    if baseline:
        ratios = {}
        for key in ("p50_ms", "p95_ms", "p99_ms", "requests_per_second", "transfer_bytes"):
            old, current = baseline["summary"].get(key), report["summary"].get(key)
            if isinstance(old, (int, float)) and not isinstance(old, bool) and math.isfinite(old) and old >= 0 and current is not None:
                ratios[key] = current / old if old else None
        report["baseline_comparison"] = dict(current_over_baseline=ratios, same_config=baseline.get("config") == config,
                                             same_origin=baseline.get("target_origin") == address[3], judgment="ratios only")
    with output.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2, allow_nan=False)
        stream.write("\n")
    return report

def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for flag, help_text in (("base-url", "required HTTP(S) origin; no credentials or path"), ("paths-file", "GET paths, one per line"),
                            ("output", "new JSON file outside repository; parent must exist")):
        parser.add_argument("--" + flag, required=True, help=help_text)
    parser.add_argument("--header-file", help="existing external JSON header object; values never reported")
    parser.add_argument("--baseline-file", help="prior benchmark JSON; report ratios without improvement claims")
    parser.add_argument("--expect-json", action="store_true", help="require complete valid JSON (identity/gzip)")
    parser.add_argument("--expect-status", type=int, action="append", help="allowed status, repeatable; default 200")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--rounds", type=int, default=1, help="count-mode rounds; maximum wall time 300s")
    group.add_argument("--duration", type=float, help="repeat batches for seconds, e.g. 86400; requests is minimum total")
    for flag, default, kind in (("clients", 10, int), ("requests", 100, int), ("interval", 1., float),
            ("connect-timeout", 2., float), ("read-timeout", 5., float), ("request-timeout", 10., float),
            ("max-body-bytes", 8 * 1024 * 1024, int), ("max-p95-ms", 500., float), ("max-p99-ms", 1000., float)):
        parser.add_argument("--" + flag, type=kind, default=default)
    parser.add_argument("--pid", type=int, help="same-host Linux PID; exact starttime/executable identity required")
    args = parser.parse_args(argv)
    args.expect_status = args.expect_status or [200]
    limits = {"clients": 128, "requests": 1000000, "rounds": 100000, "duration": 604800,
              "interval": 60, "connect_timeout": 60, "read_timeout": 60, "request_timeout": 60,
              "max_body_bytes": 64 * 1024 * 1024, "max_p95_ms": 60000, "max_p99_ms": 60000, "pid": 2 ** 31 - 1}
    if any(getattr(args, key) is not None and (not math.isfinite(getattr(args, key)) or not 0 < getattr(args, key) <= limit)
           for key, limit in limits.items()) or any(not 100 <= status <= 599 for status in args.expect_status):
        parser.error("numeric options must be positive, finite and within documented bounds")
    return args

if __name__ == "__main__":
    try:
        result = benchmark(arguments())
        print(json.dumps({"passed": result["passed"], "failures": result["failures"], "completed": result["summary"]["completed"]}))
        raise SystemExit(0 if result["passed"] else 1)
    except (OSError, ValueError, TypeError) as error:
        print(json.dumps({"passed": False, "failures": ["invalid_configuration_or_output"], "error_type": type(error).__name__}))
        raise SystemExit(2)
