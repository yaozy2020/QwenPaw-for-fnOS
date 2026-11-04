#!/usr/bin/env python3
import base64
import ctypes
import ctypes.util
import gzip
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
CGI_SOURCE = ROOT / "app/ui/index.cgi"
SUPERVISOR_SOURCE = Path(os.environ.get(
    "QWENPAW_TEST_SUP_BIN", ROOT / "supervisor/target/release/qwenpaw-supervisor"
)).resolve()
NATIVE_COMMAND = ("web", "index-cgi")
URI_PREFIX = "/cgi/ThirdParty/qwenpaw/index.cgi"
PAGE = "<!doctype html><meta charset=utf-8><title>静态页</title>\n".encode("utf-8")
CSS = "body{color:#123456}\n".encode("ascii")
PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a4XcAAAAASUVORK5CYII="
)
SECRET = b"outside-tree-secret-must-never-be-served"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class BrotliCodec:
    def __init__(self):
        library = ctypes.util.find_library("brotlienc")
        decoder_library = ctypes.util.find_library("brotlidec")
        if not library or not decoder_library:
            raise RuntimeError("Brotli fixture generation requires libbrotlienc and libbrotlidec")
        self.encoder = ctypes.CDLL(library)
        self.decoder = ctypes.CDLL(decoder_library)
        self.encoder.BrotliEncoderMaxCompressedSize.argtypes = [ctypes.c_size_t]
        self.encoder.BrotliEncoderMaxCompressedSize.restype = ctypes.c_size_t
        self.encoder.BrotliEncoderCompress.argtypes = [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_size_t,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.c_void_p,
        ]
        self.encoder.BrotliEncoderCompress.restype = ctypes.c_int
        self.decoder.BrotliDecoderDecompress.argtypes = [
            ctypes.c_size_t,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.c_void_p,
        ]
        self.decoder.BrotliDecoderDecompress.restype = ctypes.c_int

    def compress(self, payload):
        capacity = self.encoder.BrotliEncoderMaxCompressedSize(len(payload))
        output = ctypes.create_string_buffer(capacity)
        output_size = ctypes.c_size_t(capacity)
        source = ctypes.create_string_buffer(payload)
        success = self.encoder.BrotliEncoderCompress(
            5, 22, 0, len(payload), source, ctypes.byref(output_size), output
        )
        if not success:
            raise RuntimeError("Brotli fixture compression failed")
        return output.raw[:output_size.value]

    def decompress(self, payload, expected_size):
        output = ctypes.create_string_buffer(max(expected_size, 1))
        output_size = ctypes.c_size_t(expected_size)
        source = ctypes.create_string_buffer(payload)
        success = self.decoder.BrotliDecoderDecompress(
            len(payload), source, ctypes.byref(output_size), output
        )
        if success != 1:
            raise AssertionError("CGI returned invalid Brotli bytes")
        return output.raw[:output_size.value]


def parse_cgi_response(stdout):
    separators = [
        (stdout.index(separator), separator)
        for separator in (b"\r\n\r\n", b"\n\n") if separator in stdout
    ]
    if not separators:
        raise AssertionError(f"CGI response has no header/body separator: {stdout!r}")
    offset, separator = min(separators)
    raw_headers, body = stdout[:offset], stdout[offset + len(separator):]
    status = 200
    headers = {}
    seen_status = False
    for line in raw_headers.replace(b"\r\n", b"\n").split(b"\n"):
        if b":" not in line or any(byte < 0x20 or byte == 0x7f for byte in line):
            raise AssertionError(f"invalid or unsafe CGI header: {line!r}")
        name, value = line.split(b":", 1)
        if not re.fullmatch(rb"[A-Za-z0-9-]+", name):
            raise AssertionError(f"invalid CGI header name: {name!r}")
        name = name.decode("ascii").lower()
        value = value.decode("latin-1").strip()
        if name == "status":
            match = re.fullmatch(r"(\d{3})(?: .*)?", value)
            if not match or seen_status:
                raise AssertionError(f"invalid or duplicate CGI Status header: {value!r}")
            status = int(match.group(1))
            seen_status = True
        else:
            headers.setdefault(name, []).append(value)
    if "content-type" not in headers:
        raise AssertionError("CGI response has no Content-Type header")
    return status, headers, body


def header(headers, name):
    values = headers.get(name.lower(), [])
    return values[0] if values else None


class NativeIndexCgiIntegration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not SUPERVISOR_SOURCE.is_file() or not os.access(SUPERVISOR_SOURCE, os.X_OK):
            raise RuntimeError(f"build release supervisor before running this test: {SUPERVISOR_SOURCE}")
        if not CGI_SOURCE.is_file() or not os.access(CGI_SOURCE, os.X_OK):
            raise RuntimeError(f"index.cgi must exist and be executable: {CGI_SOURCE}")
        with SUPERVISOR_SOURCE.open("rb") as binary:
            if binary.read(4) != b"\x7fELF":
                raise RuntimeError(f"release supervisor must be a native ELF executable: {SUPERVISOR_SOURCE}")
        cls.binary_sha256 = sha256(SUPERVISOR_SOURCE)
        cls.cgi_sha256 = sha256(CGI_SOURCE)
        print(f"TEST_SUPERVISOR={SUPERVISOR_SOURCE}", flush=True)
        print(f"TEST_SUPERVISOR_SHA256={cls.binary_sha256}", flush=True)
        print(f"TEST_INDEX_CGI={CGI_SOURCE}", flush=True)
        print(f"TEST_INDEX_CGI_SHA256={cls.cgi_sha256}", flush=True)
        cls.brotli = BrotliCodec()

    @classmethod
    def tearDownClass(cls):
        if sha256(SUPERVISOR_SOURCE) != cls.binary_sha256:
            raise AssertionError("release supervisor changed during test")
        if sha256(CGI_SOURCE) != cls.cgi_sha256:
            raise AssertionError("index.cgi changed during test")

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="qwenpaw-native-index-cgi-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        if self.root.is_relative_to(Path("/var/apps/qwenpaw")):
            self.fail(f"refusing to use a real deployment path as test root: {self.root}")
        self.appdest = self.root / "target"
        self.www = self.appdest / "www"
        self.cgi = self.appdest / "ui/index.cgi"
        self.binary = self.appdest / "app/bin/qwenpaw-supervisor"
        self.outside = self.appdest / "www-outside"
        for directory in (self.www, self.cgi.parent, self.binary.parent, self.outside):
            directory.mkdir(parents=True)
        for name in ("home", "var", "etc", "share", "tmp", "empty-path"):
            (self.root / name).mkdir()

        shutil.copy2(CGI_SOURCE, self.cgi)
        shutil.copy2(SUPERVISOR_SOURCE, self.binary)
        self.assertEqual(sha256(self.cgi), self.cgi_sha256, "index.cgi changed while preparing fixture")
        self.assertEqual(sha256(self.binary), self.binary_sha256, "release binary changed while preparing fixture")
        self.addCleanup(lambda: self.assertEqual(self.binary_sha256, sha256(self.binary)))

        (self.www / "index.html").write_bytes(PAGE)
        (self.www / "index.html.br").write_bytes(self.brotli.compress(PAGE))
        (self.www / "index.html.gz").write_bytes(gzip.compress(PAGE, mtime=0))
        self.assertEqual(self.brotli.decompress((self.www / "index.html.br").read_bytes(), len(PAGE)), PAGE)
        self.assertEqual(gzip.decompress((self.www / "index.html.gz").read_bytes()), PAGE)
        (self.www / "assets").mkdir()
        (self.www / "assets/site.css").write_bytes(CSS)
        (self.www / "assets/icon.png").write_bytes(PNG)
        (self.outside / "secret.txt").write_bytes(SECRET)
        (self.www / "escape").symlink_to(self.outside, target_is_directory=True)
        self.env = {
            "PATH": str(self.root / "empty-path"),
            "HOME": str(self.root / "home"),
            "TMPDIR": str(self.root / "tmp"),
            "TRIM_PKGHOME": str(self.root / "home"),
            "TRIM_PKGVAR": str(self.root / "var"),
            "TRIM_PKGETC": str(self.root / "etc"),
            "TRIM_DATA_SHARE_PATHS": str(self.root / "share"),
            "TRIM_APPDEST": str(self.appdest),
            "SUPERVISOR_AUTOSTART": "0",
            "CONTENT_LENGTH": "0",
            "QUERY_STRING": "",
            "SCRIPT_NAME": URI_PREFIX,
        }

    def request(self, uri, method="GET", *, accept_encoding=None, cookie=None,
                https=False, request_scheme=None, forwarded_proto=None, native=False):
        env = dict(self.env, REQUEST_METHOD=method, REQUEST_URI=uri)
        if accept_encoding is not None:
            env["HTTP_ACCEPT_ENCODING"] = accept_encoding
        if cookie is not None:
            env["HTTP_COOKIE"] = cookie
        if https:
            env["HTTPS"] = "on"
        if request_scheme is not None:
            env["REQUEST_SCHEME"] = request_scheme
        if forwarded_proto is not None:
            env["HTTP_X_FORWARDED_PROTO"] = forwarded_proto
        command = [str(self.binary), *NATIVE_COMMAND] if native else [str(self.cgi)]
        process = subprocess.run(
            command, env=env, cwd=self.root, input=b"", stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, timeout=10, check=False,
        )
        self.assertEqual(
            process.returncode, 0,
            f"{command!r} exited {process.returncode}: stderr={process.stderr.decode(errors='replace')!r}",
        )
        self.assertEqual(process.stderr, b"", "CGI wrote unexpected diagnostics")
        response = parse_cgi_response(process.stdout)
        self.assertEqual(sha256(self.binary), self.binary_sha256, "fixture binary changed during test")
        self.assertEqual(sha256(self.cgi), self.cgi_sha256, "fixture index.cgi changed during test")
        return response

    def assert_not_disclosed(self, body):
        for forbidden in (SECRET, str(self.root).encode(), str(self.outside).encode()):
            self.assertNotIn(forbidden, body)

    def assert_representation(self, headers, body, encoding):
        self.assertEqual(header(headers, "content-type"), "text/html; charset=utf-8")
        self.assertEqual(header(headers, "content-encoding"), encoding)
        self.assertIn("accept-encoding", (header(headers, "vary") or "").lower())
        self.assertEqual(header(headers, "content-length"), str(len(body)))
        if encoding == "br":
            expected = (self.www / "index.html.br").read_bytes()
            decoded = self.brotli.decompress(body, len(PAGE))
        elif encoding == "gzip":
            expected = (self.www / "index.html.gz").read_bytes()
            decoded = gzip.decompress(body)
        else:
            expected = decoded = PAGE
        self.assertEqual(body, expected)
        self.assertEqual(decoded, PAGE)

    def test_thin_index_cgi_executes_the_real_release_binary(self):
        source = self.cgi.read_text(encoding="utf-8")
        active = "\n".join(
            line for line in source.splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        )
        self.assertRegex(active, r'exec\s+"\$candidate"\s+web\s+index-cgi')
        self.assertIn('"${APPDEST}/app/bin/qwenpaw-supervisor"', active)
        self.assertIsNotNone(
            re.search(r"\bqwenpaw-supervisor\b", active),
            "index.cgi must exec the release supervisor rather than implement static serving in Shell",
        )
        for command in ("cat", "curl", "python3", "awk", "grep", "sed"):
            self.assertIsNone(
                re.search(rf"\b{command}\b", active),
                f"thin index.cgi must not implement static serving with {command}",
            )
        self.assertEqual(sha256(self.binary), self.binary_sha256)

    def test_real_release_binary_and_thin_entry_serve_identical_bytes(self):
        for native in (False, True):
            with self.subTest(native=native):
                status, headers, body = self.request(f"{URI_PREFIX}/", native=native)
                self.assertEqual(status, 200)
                self.assert_representation(headers, body, None)

    def test_default_page_variants_and_query_string(self):
        for tail in ("", "/", "?cache=1", "/?cache=1", "/index.html?x=index.cgi/../secret"):
            with self.subTest(tail=tail):
                status, headers, body = self.request(f"{URI_PREFIX}{tail}")
                self.assertEqual(status, 200)
                self.assert_representation(headers, body, None)
                self.assertIn("no-store", header(headers, "cache-control") or "")
                self.assertEqual(header(headers, "x-content-type-options"), "nosniff")

    def test_html_csrf_issuance_is_random_and_secure_only_over_https(self):
        issued = set()
        for options, secure in (
            ({}, False), ({"https": True}, True),
            ({"request_scheme": "https"}, True), ({"forwarded_proto": "https"}, False),
        ):
            with self.subTest(options=options):
                status, headers, body = self.request(f"{URI_PREFIX}/", **options)
                self.assertEqual(status, 200)
                self.assertEqual(body, PAGE)
                cookies = headers.get("set-cookie", [])
                self.assertEqual(len(cookies), 1, cookies)
                cookie_name = "__Host-qwenpaw_csrf" if secure else "qwenpaw_csrf"
                self.assertRegex(cookies[0], rf"^{re.escape(cookie_name)}=[0-9a-f]{{32}}(?:;|$)")
                token = cookies[0].split(";", 1)[0].split("=", 1)[1]
                self.assertNotEqual(token, "0" * 32)
                self.assertNotIn(token, issued)
                issued.add(token)
                attributes = {part.strip().lower() for part in cookies[0].split(";")[1:]}
                self.assertIn("path=/", attributes)
                self.assertIn("samesite=lax", attributes)
                self.assertNotIn("domain=", attributes)
                self.assertNotIn("httponly", attributes, "double-submit CSRF must remain JS-readable")
                self.assertEqual("secure" in attributes, secure)
                if secure:
                    self.assertTrue(cookies[0].endswith("; Path=/; Secure; SameSite=Lax"))
                else:
                    self.assertTrue(cookies[0].endswith("; Path=/; SameSite=Lax"))

    def test_existing_scheme_scoped_csrf_cookie_is_preserved_without_reissuance(self):
        for cookie, https in (
            ("session=fixture; __Host-qwenpaw_csrf=host-token", True),
            ("session=fixture; qwenpaw_csrf=legacy-token", False),
        ):
            for encoding in (None, "br", "gzip"):
                with self.subTest(cookie=cookie, encoding=encoding):
                    status, headers, body = self.request(
                        f"{URI_PREFIX}/index.html", cookie=cookie,
                        https=https, accept_encoding=encoding,
                    )
                    self.assertEqual(status, 200)
                    self.assert_representation(headers, body, encoding)
                    self.assertNotIn("set-cookie", headers)

    def test_legacy_cookie_does_not_suppress_https_host_cookie_issuance(self):
        status, headers, body = self.request(
            f"{URI_PREFIX}/", cookie="session=fixture; qwenpaw_csrf=legacy-token", https=True
        )
        self.assertEqual(status, 200)
        self.assertEqual(body, PAGE)
        self.assertRegex(
            header(headers, "set-cookie") or "",
            r"^__Host-qwenpaw_csrf=[0-9a-f]{32}; Path=/; Secure; SameSite=Lax$",
        )

    def test_duplicate_csrf_cookie_name_is_reissued_for_each_scheme(self):
        for cookie, https, name in (
            (
                "__Host-qwenpaw_csrf=first; __Host-qwenpaw_csrf=second",
                True,
                "__Host-qwenpaw_csrf",
            ),
            ("qwenpaw_csrf=first; qwenpaw_csrf=second", False, "qwenpaw_csrf"),
        ):
            with self.subTest(cookie=cookie):
                status, headers, _body = self.request(f"{URI_PREFIX}/", cookie=cookie, https=https)
                self.assertEqual(status, 200)
                self.assertRegex(
                    header(headers, "set-cookie") or "",
                    rf"^{re.escape(name)}=[0-9a-f]{{32}};",
                )

    def test_empty_csrf_cookie_is_reissued(self):
        status, headers, body = self.request(f"{URI_PREFIX}/", cookie="qwenpaw_csrf=; session=fixture")
        self.assertEqual(status, 200)
        self.assertEqual(body, PAGE)
        self.assertRegex(header(headers, "set-cookie") or "", r"^qwenpaw_csrf=[0-9a-f]{32};")

    def test_css_and_image_mime_types_and_bytes(self):
        for path, mime, payload in (
            ("assets/site.css", "text/css; charset=utf-8", CSS),
            ("assets/icon.png", "image/png", PNG),
        ):
            with self.subTest(path=path):
                status, headers, body = self.request(f"{URI_PREFIX}/{path}")
                self.assertEqual(status, 200)
                self.assertEqual(header(headers, "content-type"), mime)
                self.assertEqual(header(headers, "content-length"), str(len(payload)))
                self.assertEqual(header(headers, "x-content-type-options"), "nosniff")
                self.assertEqual(body, payload)
                self.assertNotIn("set-cookie", headers)

    def test_head_matches_get_metadata_and_never_sends_body(self):
        for path, encoding in (
            ("index.html", None), ("index.html", "br"), ("index.html", "gzip"),
            ("assets/site.css", None), ("assets/icon.png", None), ("missing.css", None),
        ):
            with self.subTest(path=path, encoding=encoding):
                options = {"accept_encoding": encoding, "cookie": "qwenpaw_csrf=existing-token"}
                get_status, get_headers, get_body = self.request(f"{URI_PREFIX}/{path}", **options)
                head_status, head_headers, head_body = self.request(
                    f"{URI_PREFIX}/{path}", method="HEAD", **options
                )
                self.assertEqual(head_status, get_status)
                self.assertEqual(head_headers, get_headers)
                self.assertEqual(header(head_headers, "content-length"), str(len(get_body)))
                self.assertEqual(head_body, b"")

    def test_other_methods_are_rejected_with_get_head_allow(self):
        for method in ("POST", "PUT", "DELETE", "OPTIONS"):
            with self.subTest(method=method):
                status, headers, body = self.request(f"{URI_PREFIX}/", method=method)
                self.assertEqual(status, 405)
                self.assertEqual({part.strip() for part in (header(headers, "allow") or "").split(",")}, {"GET", "HEAD"})
                self.assertNotIn("set-cookie", headers)
                self.assertNotIn(PAGE, body)
                self.assert_not_disclosed(body)

    def test_missing_file_is_a_safe_404(self):
        status, headers, body = self.request(f"{URI_PREFIX}/missing-secret.txt")
        self.assertEqual(status, 404)
        self.assertEqual(header(headers, "x-content-type-options"), "nosniff")
        self.assertNotIn("set-cookie", headers)
        self.assertNotIn(PAGE, body)
        self.assert_not_disclosed(body)

    def test_unknown_existing_files_and_direct_sidecars_do_not_leak(self):
        for name in ("supervisor-token", "private.conf", "assets/private.key"):
            (self.www / name).write_bytes(SECRET)
        for path in ("supervisor-token", "private.conf", "assets/private.key", "index.html.br", "index.html.gz"):
            with self.subTest(path=path):
                status, headers, body = self.request(f"{URI_PREFIX}/{path}")
                self.assertEqual(status, 404)
                self.assertNotIn("content-encoding", headers)
                self.assert_not_disclosed(body)
                self.assertNotIn((self.www / "index.html.br").read_bytes(), body)
                self.assertNotIn((self.www / "index.html.gz").read_bytes(), body)

    def test_br_and_gzip_bytes_and_decompression_match_html(self):
        for encoding in ("br", "gzip"):
            with self.subTest(encoding=encoding):
                status, headers, body = self.request(f"{URI_PREFIX}/index.html", accept_encoding=encoding)
                self.assertEqual(status, 200)
                self.assert_representation(headers, body, encoding)
                self.assertRegex(header(headers, "set-cookie") or "", r"^qwenpaw_csrf=[0-9a-f]{32};")

    def test_q_zero_and_explicit_wildcard_precedence(self):
        for accept_encoding, expected in (
            ("br, gzip", "br"), ("BR;Q=0.5, gzip;q=0.8", "br"),
            ("br;q=0", None), ("gzip;q=0", None), ("*;q=0", None),
            ("br;q=0, gzip;q=0, *;q=1", None),
            ("br;q=0, *;q=1", "gzip"), ("*;q=1, br;q=0", "gzip"),
            ("gzip;q=0, *;q=1", "br"), ("*;q=1, gzip;q=0", "br"),
            ("*;q=0, br;q=1", "br"), ("*;q=0, gzip;q=1", "gzip"),
            ("*;q=1", "br"),
        ):
            with self.subTest(accept_encoding=accept_encoding):
                status, headers, body = self.request(f"{URI_PREFIX}/index.html", accept_encoding=accept_encoding)
                self.assertEqual(status, 200)
                self.assert_representation(headers, body, expected)

    def test_missing_compressed_siblings_fall_back_without_mislabeling(self):
        (self.www / "index.html.br").unlink()
        status, headers, body = self.request(f"{URI_PREFIX}/", accept_encoding="br, gzip")
        self.assertEqual(status, 200)
        self.assert_representation(headers, body, "gzip")
        (self.www / "index.html.gz").unlink()
        status, headers, body = self.request(f"{URI_PREFIX}/", accept_encoding="br, gzip")
        self.assertEqual(status, 200)
        self.assert_representation(headers, body, None)

    def test_valid_percent_encoding_is_decoded_once(self):
        status, headers, body = self.request(f"{URI_PREFIX}/%69ndex%2ehtml")
        self.assertEqual(status, 200)
        self.assert_representation(headers, body, None)

    def test_encoded_and_double_encoded_traversal_do_not_escape(self):
        tails = (
            "../www-outside/secret.txt", "%2e%2e/www-outside/secret.txt",
            "%2E%2E/www-outside/secret.txt", ".%2e/www-outside/secret.txt",
            "%252e%252e/www-outside/secret.txt", "%252E%252E%252fwww-outside%252fsecret.txt",
            "%2e%2e%2fwww-outside%2fsecret.txt", "..%5cwww-outside%5csecret.txt",
            "%2f" + str(self.outside / "secret.txt").lstrip("/").replace("/", "%2f"),
        )
        self.assertTrue((self.www / "../www-outside/secret.txt").is_file())
        for tail in tails:
            with self.subTest(tail=tail):
                status, _headers, body = self.request(f"{URI_PREFIX}/{tail}")
                self.assertIn(status, (400, 404))
                self.assertNotIn(PAGE, body)
                self.assert_not_disclosed(body)

    def test_nul_and_malformed_percent_paths_are_rejected(self):
        for tail in ("index.html%00", "%00index.html", "index.html%2500", "%", "%0", "%zz", "%ff"):
            with self.subTest(tail=tail):
                status, _headers, body = self.request(f"{URI_PREFIX}/{tail}")
                self.assertIn(status, (400, 404))
                self.assertNotIn(PAGE, body)
                self.assert_not_disclosed(body)

    def test_symlink_cannot_escape_static_root(self):
        page = self.www / "index.html"
        page.unlink()
        page.symlink_to(self.outside / "secret.txt")
        for tail in ("index.html", "escape/secret.txt"):
            with self.subTest(tail=tail):
                status, _headers, body = self.request(f"{URI_PREFIX}/{tail}")
                self.assertEqual(status, 404)
                self.assert_not_disclosed(body)

    def test_compressed_symlinks_cannot_escape_static_root(self):
        for suffix, encoding in (("br", "br"), ("gz", "gzip")):
            with self.subTest(encoding=encoding):
                sidecar = self.www / f"index.html.{suffix}"
                sidecar.unlink()
                sidecar.symlink_to(self.outside / "secret.txt")
                status, _headers, body = self.request(f"{URI_PREFIX}/", accept_encoding=encoding)
                self.assertEqual(status, 404)
                self.assert_not_disclosed(body)

    def test_control_characters_are_safe_in_404_responses(self):
        for tail in (
            "missing-\x01-name.txt", "missing-\x1b-name.txt", "missing-\x7f-name.txt",
            "missing-\r\nX-Injected: unsafe.txt", "missing-%0d%0aX-Injected%3a%20unsafe.txt",
        ):
            with self.subTest(tail=tail):
                status, headers, body = self.request(f"{URI_PREFIX}/{tail}")
                self.assertEqual(status, 404)
                self.assertNotIn("x-injected", headers)
                self.assertFalse(any(byte < 0x20 or byte == 0x7f for byte in body if byte not in (9, 10, 13)))
                self.assert_not_disclosed(body)


if __name__ == "__main__":
    result = unittest.main(verbosity=2, exit=False).result
    if result.wasSuccessful():
        print("PASS: isolated native index CGI integration suite; no services or real data paths used")
    raise SystemExit(0 if result.wasSuccessful() else 1)
