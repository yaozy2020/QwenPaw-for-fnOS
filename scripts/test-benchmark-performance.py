#!/usr/bin/env python3
"""Standard-library behavior tests; HTTP and proc fixtures stay outside the project."""
import contextlib
import gzip
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SCRIPT = Path(__file__).with_name("benchmark-performance.py")
SPEC = importlib.util.spec_from_file_location("performance_benchmark", SCRIPT)
BENCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCH)
SECRET = "fixture-private-cookie-token-body"

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        route = self.path.split("?", 1)[0]
        with self.server.lock:
            count = self.server.counts.get(route, 0) + 1
            self.server.counts[route] = count
            self.server.received.append(dict(self.headers))
            self.server.active += 1
            self.server.peak = max(self.server.peak, self.server.active)
        try:
            if self.server.hook:
                self.server.hook(route, count)
            status = 503 if route == "/status" or (route == "/flaky" and count > 1) else 200
            body = ('{"ok":true,"private":"' + SECRET + '"}').encode()
            if route == "/static":
                body = b"<html>plain static</html>" * 20
            elif route == "/invalid":
                body = b'{"truncated":'
            elif route == "/nan":
                body = b'{"value":NaN}'
            if route == "/slow" and count > 1:
                time.sleep(.12)
            if route in {"/redirect", "/cross"}:
                status = 302
            if route == "/headers" and count > 1:
                self.wfile.write(b"HTTP/1.0 200 OK\r\n")
                self.wfile.flush()
                for _index in range(30):
                    self.wfile.write(b"X-Trickle: yes\r\n")
                    self.wfile.flush()
                    time.sleep(.015)
                self.wfile.write(b"\r\n")
                return
            self.send_response(status)
            if route in {"/gzip", "/bad-gzip", "/zipbomb"}:
                body = gzip.compress(b" " * 10000) if route == "/zipbomb" else gzip.compress(body)
                if route == "/bad-gzip":
                    body = body[:12] + b"corrupt-gzip"
                self.send_header("Content-Encoding", "gzip")
            self.send_header("Content-Length", str(len(body) + (20 if route == "/broken" else 0)))
            self.send_header("Cache-Control", f'private="{SECRET}", max-age=60, custom="{SECRET}"')
            self.send_header("Set-Cookie", "fixture=" + SECRET)
            if route in {"/redirect", "/cross"}:
                self.send_header("Location", self.server.redirect if route == "/cross" else "/ok")
            self.end_headers()
            if route == "/trickle" and count > 1:
                # Each byte arrives before the idle timeout; the absolute deadline must still win.
                for byte in body:
                    self.wfile.write(bytes([byte]))
                    self.wfile.flush()
                    time.sleep(.015)
            else:
                self.wfile.write(body)
            self.close_connection = True
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass
        finally:
            with self.server.lock:
                self.server.active -= 1

@contextlib.contextmanager
def http_fixture():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.lock = threading.Lock()
    server.counts, server.received, server.active, server.peak = {}, [], 0, 0
    server.hook, server.redirect = None, "http://127.0.0.1:1/never"
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": .01}, daemon=True)
    thread.start()
    try:
        yield server, f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)

class PerformanceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="qwenpaw-benchmark-")
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name).resolve()
        self.assertFalse(self.work.is_relative_to(BENCH.ROOT))
        self.paths, self.output = self.work / "paths.txt", self.work / "report.json"
        self.paths.write_text("/ok\n", encoding="utf-8")
        self.serial = 0

    def args(self, base="http://127.0.0.1:1", *extra, paths="/ok\n"):
        self.paths.write_text(paths, encoding="utf-8")
        self.serial += 1
        self.output = self.work / f"report-{self.serial}.json"
        return BENCH.arguments(["--base-url", base, "--paths-file", str(self.paths), "--output", str(self.output),
                                "--requests", "6", "--clients", "2", "--interval", ".01", *extra])

    def run_report(self, args, sampler=None):
        report = BENCH.benchmark(args, sampler=sampler)
        self.assertEqual(json.loads(Path(args.output).read_text(encoding="utf-8")), report)
        self.assertNotIn(SECRET, json.dumps(report))
        return report

    def proc(self, *, start=900, user=100, system=20, rss=7, threads=3, pid=42):
        root = self.work / "proc"
        proc = root / str(pid)
        (proc / "fd").mkdir(parents=True, exist_ok=True)
        for name in ("0", "1", "2", "3"):
            (proc / "fd" / name).touch()
        (proc / "exe").write_bytes(b"fixture executable")
        # Linux stat fields 1..24 with a comm containing whitespace and a closing parenthesis.
        (proc / "stat").write_text(f"{pid} (worker ) odd name) S 1 2 3 4 5 6 7 8 9 10 {user} {system} 0 0 0 0 {threads} 0 {start} 4096 {rss}\n")
        (proc / "io").write_text("rchar: 4096\nwchar: 5\nread_bytes: 999999\n")
        return BENCH.ProcSampler(pid, root=root, ticks=100, page_size=4096)

    def test_defaults_and_long_duration_are_explicit_and_bounded(self):
        args = BENCH.arguments(["--base-url", "http://127.0.0.1:3", "--paths-file", str(self.paths), "--output", str(self.output)])
        self.assertEqual((args.clients, args.requests, args.max_p95_ms, args.max_p99_ms), (10, 100, 500, 1000))
        self.assertEqual(self.args("http://127.0.0.1", "--duration", "86400").duration, 86400)
        for option, value in (("--duration", "nan"), ("--interval", "0"), ("--clients", "129"), ("--requests", "0")):
            with self.subTest(option=option), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.args("http://127.0.0.1", option, value)

    def test_real_get_counts_rounds_transfer_percentiles_and_sanitized_headers(self):
        with http_fixture() as (server, base):
            report = self.run_report(self.args(base, "--rounds", "2", paths="/ok?token=" + SECRET + "\n/static\n"))
            self.assertTrue(report["passed"], report)
            self.assertEqual(report["summary"]["completed"], 12)
            self.assertEqual(sum(server.counts.values()), 14)  # Two preflights, twelve measured GETs.
            expected = 6 * (len(('{"ok":true,"private":"' + SECRET + '"}').encode()) + len(b"<html>plain static</html>" * 20))
            self.assertEqual(report["summary"]["transfer_bytes"], expected)
            self.assertLessEqual(server.peak, 2)
            self.assertEqual(report["summary"]["statuses"], {"200": 12})
            self.assertEqual(report["summary"]["cache_control"], {"max-age=60,other,private": 12})
            self.assertLessEqual(report["summary"]["p50_ms"], report["summary"]["p95_ms"])
            self.assertLessEqual(report["summary"]["p95_ms"], report["summary"]["p99_ms"])
            self.assertIsNone(report["server_process"])
            self.assertGreaterEqual(report["client_cpu_seconds"], 0)

    def test_external_header_file_authenticates_requests_without_copying_secrets(self):
        headers = self.work / "headers.json"
        original = json.dumps({"Cookie": "ost=" + SECRET, "Authorization": "Bearer " + SECRET, "Accept-Encoding": "gzip"})
        headers.write_text(original)
        with http_fixture() as (server, base):
            report = self.run_report(self.args(base, "--header-file", str(headers), "--expect-json", paths="/gzip\n"))
            self.assertTrue(report["passed"])
            self.assertTrue(all(item["Authorization"] == "Bearer " + SECRET for item in server.received))
            self.assertEqual(report["summary"]["content_encoding"], {"gzip": 6})
            self.assertEqual(headers.read_text(), original)

    def test_static_and_invalid_json_are_allowed_only_without_json_probe(self):
        with http_fixture() as (_server, base):
            for route in ("/static", "/invalid", "/nan"):
                with self.subTest(route=route):
                    self.assertTrue(self.run_report(self.args(base, paths=route + "\n"))["passed"])
                    rejected = self.run_report(self.args(base, "--expect-json", paths=route + "\n"))
                    self.assertFalse(rejected["passed"])
                    self.assertEqual(rejected["summary"]["completed"], 0)

    def test_static_size_cap_is_a_reported_lower_bound_and_not_json_success(self):
        with http_fixture() as (_server, base):
            report = self.run_report(self.args(base, "--max-body-bytes", "8", paths="/static\n"))
            self.assertTrue(report["passed"])
            self.assertEqual(report["summary"]["capped_responses"], 6)
            self.assertEqual(report["summary"]["transfer_bytes"], 54)
            rejected = self.run_report(self.args(base, "--max-body-bytes", "8", "--expect-json"))
            self.assertFalse(rejected["passed"])

    def test_incomplete_wire_body_fails_even_without_json(self):
        with http_fixture() as (_server, base):
            report = self.run_report(self.args(base, paths="/broken\n"))
            self.assertFalse(report["passed"])
            self.assertEqual(report["preflight"]["failed"], 1)

    def test_http_errors_count_and_expected_status_can_be_selected(self):
        with http_fixture() as (_server, base):
            report = self.run_report(self.args(base, paths="/flaky\n"))
            self.assertFalse(report["passed"])
            self.assertEqual(report["summary"]["failed"], 6)
            self.assertEqual(report["summary"]["errors"], {"http_status": 6})
            allowed = self.run_report(self.args(base, "--expect-status", "503", paths="/status\n"))
            self.assertTrue(allowed["passed"])
            self.assertEqual(allowed["summary"]["statuses"], {"503": 6})

    def test_redirects_never_follow_or_leak_auth_to_another_origin(self):
        with http_fixture() as (other, destination), http_fixture() as (server, base):
            server.redirect = destination + "/ok"
            for route in ("/redirect", "/cross"):
                with self.subTest(route=route):
                    report = self.run_report(self.args(base, "--expect-status", "302", paths=route + "\n"))
                    self.assertFalse(report["passed"])
                    self.assertEqual(report["preflight"]["failed"], 1)
            self.assertEqual(other.counts, {})
            self.assertNotIn("/ok", server.counts)

    def test_idle_timeout_counts_all_attempts_as_failures(self):
        with http_fixture() as (_server, base):
            report = self.run_report(self.args(base, "--read-timeout", ".025", paths="/slow\n"))
            self.assertFalse(report["passed"])
            self.assertEqual(report["summary"]["errors"], {"timeout": 6})
            self.assertEqual(report["summary"]["completed"], 6)

    def test_trickle_body_and_headers_cannot_extend_absolute_deadline(self):
        with http_fixture() as (_server, base):
            for route in ("/trickle", "/headers"):
                started = time.perf_counter()
                report = self.run_report(self.args(base, "--requests", "1", "--clients", "1", "--request-timeout", ".07",
                                                    "--read-timeout", ".04", paths=route + "\n"))
                self.assertLess(time.perf_counter() - started, 1)
                self.assertEqual(report["summary"]["errors"], {"timeout": 1})

    def test_invalid_or_excessive_gzip_json_fails_closed(self):
        with http_fixture() as (_server, base):
            for route in ("/bad-gzip", "/zipbomb"):
                report = self.run_report(self.args(base, "--expect-json", "--max-body-bytes", "1000", paths=route + "\n"))
                self.assertFalse(report["passed"])
                self.assertEqual(report["preflight"]["errors"], {"invalid_json": 1})

    def test_latency_budgets_fail_with_successful_http(self):
        with http_fixture() as (_server, base):
            report = self.run_report(self.args(base, "--requests", "2", "--max-p95-ms", "20", "--max-p99-ms", "25", paths="/slow\n"))
            self.assertEqual(report["summary"]["failed"], 0)
            self.assertIn("p95_budget", report["failures"])
            self.assertIn("p99_budget", report["failures"])

    def test_duration_interval_runs_repeated_batches_until_deadline(self):
        with http_fixture() as (_server, base):
            report = self.run_report(self.args(base, "--requests", "1", "--duration", ".12", "--interval", ".04"))
            self.assertTrue(report["passed"], report)
            self.assertGreaterEqual(report["summary"]["completed"], 2)
            self.assertLess(report["summary"]["completed"], 8)
            self.assertGreaterEqual(report["summary"]["wall_seconds"], .1)

    def test_duration_cannot_pass_when_minimum_requests_are_unreached(self):
        with http_fixture() as (_server, base):
            report = self.run_report(self.args(base, "--requests", "100", "--duration", ".015", paths="/slow\n"))
            self.assertFalse(report["passed"])
            self.assertIn("requests_not_reached", report["failures"])
            self.assertLess(report["summary"]["completed"], 100)

    def test_unavailable_endpoint_and_missing_pid_fail_before_measured_work(self):
        with http_fixture() as (_server, base):
            missing = BENCH.ProcSampler(99, self.work, ticks=100, page_size=4096)
            report = self.run_report(self.args(base, "--pid", "99"), missing)
            self.assertFalse(report["passed"])
            self.assertEqual(report["preflight"]["completed"], 0)
            self.assertIn("zero_samples", report["failures"])
        report = self.run_report(self.args(base, "--connect-timeout", ".1"))
        self.assertFalse(report["passed"])
        self.assertIn("preflight_failed", report["failures"])

    def test_dns_preflight_is_bounded_and_prevents_gets(self):
        release = threading.Event()
        def blocked_lookup(*_args, **_kwargs):
            release.wait(timeout=1)
            return []
        with http_fixture() as (server, base), patch.object(BENCH.socket, "getaddrinfo", side_effect=blocked_lookup):
            try:
                started = time.perf_counter()
                report = self.run_report(self.args(base, "--connect-timeout", ".03"))
                self.assertLess(time.perf_counter() - started, .5)
                self.assertFalse(report["passed"])
                self.assertIn("preflight_failed", report["failures"])
                self.assertEqual(server.counts, {})
            finally:
                release.set()

    def test_unsafe_urls_paths_headers_and_repository_output_are_preflight_rejected(self):
        for base in ("ftp://127.0.0.1", "http://user:secret@127.0.0.1", "http://127.0.0.1/a", "http://127.0.0.1:0"):
            with self.subTest(base=base), self.assertRaises(ValueError):
                BENCH.benchmark(self.args(base))
        for path in ("//elsewhere/a", "http://elsewhere/a", "/a#fragment", "/%0aHeader:value", "/%2felsewhere", "/a\\b"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                BENCH.benchmark(self.args(paths=path + "\n"))
        args = self.args()
        args.output = BENCH.ROOT / "benchmark-must-not-create.json"
        with self.assertRaises(ValueError):
            BENCH.benchmark(args)
        self.assertFalse(args.output.exists())
        for headers in ({"X-HTTP-Method-Override": "POST"}, {"Cookie": SECRET + "\r\nInjected: yes"}):
            source = self.work / "headers.json"
            source.write_text(json.dumps(headers))
            with self.subTest(headers=list(headers)), self.assertRaises(ValueError):
                BENCH.benchmark(self.args("http://127.0.0.1", "--header-file", str(source)))

    def test_report_does_not_overwrite_existing_files(self):
        args = self.args()
        Path(args.output).write_text("sentinel")
        with self.assertRaises(ValueError):
            BENCH.benchmark(args)
        self.assertEqual(Path(args.output).read_text(), "sentinel")

    def test_baseline_comparison_is_ratios_only(self):
        with http_fixture() as (_server, base):
            baseline = self.run_report(self.args(base))
            old_path = self.output
            report = self.run_report(self.args(base, "--baseline-file", str(old_path)))
            comparison = report["baseline_comparison"]
            self.assertEqual(comparison["judgment"], "ratios only")
            self.assertTrue(comparison["same_config"])
            self.assertEqual(comparison["current_over_baseline"]["transfer_bytes"], 1)
            self.assertAlmostEqual(comparison["current_over_baseline"]["p95_ms"], report["summary"]["p95_ms"] / baseline["summary"]["p95_ms"])

    def test_proc_reads_real_field_positions_and_logical_read_counter(self):
        sampler = self.proc()
        point = sampler.sample()
        self.assertEqual((point["cpu_ticks"], point["rss_bytes"], point["threads"], point["fd"], point["rchar"]), (120, 28672, 3, 4, 4096))
        self.assertEqual(sampler.identity[1], 900)
        self.assertEqual((sampler.path / "exe").read_bytes(), b"fixture executable")

    def test_server_cpu_uses_proc_ticks_and_its_wall_time_not_client_cpu(self):
        sampler = self.proc()
        with http_fixture() as (server, base):
            def advance(_route, count):
                if count == 2:
                    stat = sampler.path / "stat"
                    stat.write_text(stat.read_text().replace("100 20 0 0", "125 45 0 0"))
                    (sampler.path / "io").write_text("rchar: 5096\nread_bytes: 999999\n")
            server.hook = advance
            report = self.run_report(self.args(base, "--requests", "1", "--pid", "42"), sampler)
            self.assertTrue(report["passed"], report)
            proc = report["server_process"]
            self.assertEqual(proc["cpu_seconds"], .5)
            self.assertAlmostEqual(proc["cpu_percent_one_core"], 50 / proc["wall_seconds"])
            self.assertEqual(proc["rchar_delta"], 1000)
            self.assertIn("not physical", proc["rchar_note"])
            self.assertNotEqual(proc["cpu_seconds"], report["client_cpu_seconds"])

    def test_pid_reuse_exec_replacement_and_disappearance_fail_the_run(self):
        for change in ("starttime", "executable", "in_place", "gone", "counter"):
            with self.subTest(change=change):
                sampler = self.proc()
                with http_fixture() as (server, base):
                    def mutate(_route, count):
                        if count != 2:
                            return
                        if change == "starttime":
                            stat = sampler.path / "stat"
                            stat.write_text(stat.read_text().replace("900 4096", "901 4096"))
                        elif change == "executable":
                            replacement = sampler.path / "new-exe"
                            replacement.write_bytes(b"new identity")
                            replacement.replace(sampler.path / "exe")
                        elif change == "in_place":
                            (sampler.path / "exe").write_bytes(b"new executable content")
                        elif change == "counter":
                            (sampler.path / "io").write_text("rchar: 0\n")
                        else:
                            (sampler.path / "stat").unlink()
                    server.hook = mutate
                    report = self.run_report(self.args(base, "--requests", "1", "--pid", "42"), sampler)
                    self.assertFalse(report["passed"])
                    self.assertIn("process_sampling_failed", report["failures"])
                    if change not in {"gone", "counter"}:
                        self.assertIn("process_identity_changed", report["failures"])

    def test_identity_is_rechecked_inside_each_proc_sample(self):
        sampler = self.proc()
        real_stat, calls = sampler.stat, []
        def changing_stat():
            calls.append(1)
            result = real_stat()
            if len(calls) == 2:
                return (42, 999, *result[0][2:]), result[1]
            return result
        with patch.object(sampler, "stat", side_effect=changing_stat), self.assertRaisesRegex(ValueError, "identity changed"):
            sampler.sample()

    def test_cli_pass_and_measured_error_exit_codes_and_json(self):
        with http_fixture() as (_server, base):
            for route, code in (("/ok", 0), ("/flaky", 1)):
                args = self.args(base, paths=route + "\n")
                command = [sys.executable, "-B", str(SCRIPT), "--base-url", base, "--paths-file", str(self.paths),
                           "--output", str(args.output), "--requests", "2", "--clients", "1"]
                result = subprocess.run(command, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, code, result.stdout + result.stderr)
                self.assertEqual(json.loads(result.stdout)["passed"], code == 0)
                self.assertEqual(json.loads(Path(args.output).read_text())["summary"]["completed"], 2)
                self.assertNotIn(SECRET, result.stdout + result.stderr)

    def test_cli_failure_exit_is_nonzero_and_configuration_error_is_sanitized(self):
        args = [sys.executable, "-B", str(SCRIPT), "--base-url", "http://user:" + SECRET + "@127.0.0.1", "--paths-file", str(self.paths), "--output", str(self.output)]
        result = subprocess.run(args, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertFalse(json.loads(result.stdout)["passed"])
        self.assertNotIn(SECRET, result.stdout + result.stderr)

if __name__ == "__main__":
    unittest.main(verbosity=2)
