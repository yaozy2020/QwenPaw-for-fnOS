#!/usr/bin/env python3
"""Behavior regressions for QwenPaw's precompressed Console StaticFiles.

Run with an existing Python environment containing FastAPI, Starlette and httpx::

    python3 scripts/test-console-static.py --module-root /tmp/prepared/src/qwenpaw
    python3 scripts/test-console-static.py --module-root /venv/lib/python3.12/site-packages/qwenpaw -v

--module-root names the qwenpaw package directory, not its parent. Standard
unittest options, including -v, -q, -k and individual test names, are forwarded.
Only the static/base-path modules and their real dependencies are imported:
package shells bypass QwenPaw's __init__ files. Six route integration cases also
compile and execute the actual _app.py root/static route AST nodes; the full
application and its lifespan are never imported or started.
Requests use in-process ASGI transport. All fixture files live in a temporary
directory outside this repository; neither packages nor user data are changed.
"""

import argparse
import ast
import asyncio
import gzip
import hashlib
import importlib.machinery
import importlib.metadata
import importlib.util
import os
from pathlib import Path
import stat
import sys
import tempfile
import types
import unittest


ROOT = Path(__file__).resolve().parents[1]
BODY = b"hello"
# A complete Brotli stream containing a literal "hello" block. Keeping these
# valid bytes fixed exercises br even when the optional brotli decoder is absent.
BROTLI_BODY = b"\x0b\x02\x80hello\x03"
GZIP_BODY = gzip.compress(BODY, mtime=0)
HTML_BODY = b"<!doctype html><html><body>console fixture</body></html>\n"
SECRET_BODY = b"outside the console root: never serve this fixture\n"
HASHED_JS = "assets/main-a1B2c3D4.js"
HASHED_CSS = "assets/style-9876abcd.css"
BASE_PATH = "/app/qwenpaw/web"
ROUTE_INDEX_BODY = (
    b'<!doctype html><html><head><base href="/" />'
    b'<script>window.__qpBase = "";</script></head>'
    b'<body>isolated real Console routes</body></html>\n'
)
LOGO_BODY = b'<svg xmlns="http://www.w3.org/2000/svg"><title>route logo</title></svg>\n'


def load_static_class(module_root):
    """Load the supplied source without executing QwenPaw package initializers."""
    target = module_root / "app" / "console_static.py"
    if not target.is_file():
        raise FileNotFoundError(f"Console StaticFiles module is missing: {target}")
    if any(name == "qwenpaw" or name.startswith("qwenpaw.") for name in sys.modules):
        raise RuntimeError("run this script standalone, before importing QwenPaw")

    def package(name, directory):
        module = types.ModuleType(name)
        module.__package__ = name
        module.__path__ = [str(directory)]
        module.__spec__ = importlib.machinery.ModuleSpec(name, loader=None, is_package=True)
        module.__spec__.submodule_search_locations = module.__path__
        sys.modules[name] = module
        parent, _, child = name.rpartition(".")
        if parent:
            setattr(sys.modules[parent], child, module)

    package("qwenpaw", module_root)
    package("qwenpaw.app", module_root / "app")
    # Hub's concrete static-files module is still imported from real source;
    # only its package initializer is bypassed, just like qwenpaw/app above.
    for name, directory in (
        ("qwenpaw.hub", module_root / "hub"),
        ("qwenpaw.app.hub", module_root / "app" / "hub"),
    ):
        if directory.is_dir():
            package(name, directory)

    return load_source_module("qwenpaw.app.console_static", target).ConsoleStaticFiles, target


def load_source_module(name, target):
    """Execute one real source module inside the already constructed package shells."""
    spec = importlib.util.spec_from_file_location(name, target)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {target}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    parent, _, child = name.rpartition(".")
    setattr(sys.modules[parent], child, module)
    spec.loader.exec_module(module)
    return module


def compile_console_routes(target):
    """Select original registration nodes without importing or rewriting _app.py."""
    source = target.read_text(encoding="utf-8")
    tree = ast.parse(source, filename=str(target))
    markers = [number for number, line in enumerate(source.splitlines(), 1)
               if line.strip() == "# Console static files and SPA fallback"]
    if len(markers) != 1:
        raise RuntimeError(f"expected one Console static route marker in {target}")
    following = [node for node in tree.body if node.lineno > markers[0]]
    if not following or not isinstance(following[0], ast.If):
        raise RuntimeError(f"expected the Console static top-level if after the marker in {target}")
    roots = [node for node in tree.body
             if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "read_root"]
    cache_headers = [node for node in tree.body if isinstance(node, ast.Assign)
                     and any(isinstance(name, ast.Name) and name.id == "_INDEX_NO_CACHE_HEADERS"
                             for name in node.targets)]
    middleware = [
        node for node in tree.body
        if isinstance(node, ast.Expr) and isinstance(node.value, ast.Call)
        and isinstance(node.value.func, ast.Attribute) and node.value.func.attr == "add_middleware"
        and isinstance(node.value.func.value, ast.Name) and node.value.func.value.id == "app"
        and any(isinstance(arg, ast.Name) and arg.id == "BasePathMiddleware" for arg in node.value.args)
    ]
    if not (len(roots) == len(cache_headers) == len(middleware) == 1):
        raise RuntimeError(f"expected real root, cache headers and BasePathMiddleware registration in {target}")
    selected = [cache_headers[0], roots[0], following[0], middleware[0]]
    return compile(ast.Module(body=selected, type_ignores=[]), str(target), "exec")


class ConsoleStaticFilesTests(unittest.IsolatedAsyncioTestCase):
    module_root = None
    route_code = None
    route_base_path = None

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        if cls.module_root is None:
            raise RuntimeError("--module-root must point to a prepared or installed qwenpaw package")
        # Disable bytecode writes even when inspecting a read-only site-packages.
        previous_bytecode_setting = sys.dont_write_bytecode
        sys.dont_write_bytecode = True
        cls.addClassCleanup(setattr, sys, "dont_write_bytecode", previous_bytecode_setting)
        cls.addClassCleanup(cls._remove_loaded_qwenpaw_modules)

        import httpx
        from fastapi import FastAPI
        from starlette.staticfiles import StaticFiles

        cls.httpx = httpx
        cls.fastapi = FastAPI
        cls.static_class, target = load_static_class(cls.module_root)
        if not issubclass(cls.static_class, StaticFiles):
            raise TypeError("ConsoleStaticFiles must implement the Starlette StaticFiles interface")
        bases = [base for base in cls.static_class.__mro__[1:]
                 if base.__name__ == "CompressedStaticFiles"
                 and base.__module__.startswith("qwenpaw.")]
        if not bases:
            raise TypeError("ConsoleStaticFiles must inherit the real hub.CompressedStaticFiles")
        print(f"CONSOLE_STATIC_MODULE={target}", flush=True)
        print(f"CONSOLE_STATIC_SHA256={hashlib.sha256(target.read_bytes()).hexdigest()}", flush=True)
        for base in bases:
            source = getattr(sys.modules.get(base.__module__), "__file__", None)
            print(f"COMPRESSED_STATIC_BASE={base.__module__}.{base.__name__} source={source}", flush=True)
        print(f"PYTHON_VERSION={sys.version.split()[0]}", flush=True)
        for distribution in ("fastapi", "starlette", "httpx"):
            print(f"{distribution.upper()}_VERSION={importlib.metadata.version(distribution)}", flush=True)

    @staticmethod
    def _remove_loaded_qwenpaw_modules():
        for name in tuple(sys.modules):
            if name == "qwenpaw" or name.startswith("qwenpaw."):
                del sys.modules[name]

    async def asyncSetUp(self):
        # Do not let tempfile fall back to the current working directory or
        # probe a TMPDIR inside the project before checking its boundary.
        candidates = [Path(value) for key in ("TMPDIR", "TEMP", "TMP")
                      if (value := os.environ.get(key))]
        candidates.extend((Path("/tmp"), Path("/var/tmp"),
                           Path(os.environ.get("SystemRoot", "C:/Windows")) / "Temp"))
        temporary = None
        for candidate in candidates:
            resolved = candidate.resolve()
            if (candidate.is_dir() and not resolved.is_relative_to(ROOT)
                    and not resolved.is_relative_to(self.module_root)):
                try:
                    temporary = tempfile.TemporaryDirectory(
                        prefix="qwenpaw-console-static-", dir=resolved,
                    )
                except PermissionError:
                    continue
                break
        if temporary is None:
            self.fail("no temporary directory outside the repository and module root is available")
        self.addCleanup(temporary.cleanup)
        self.fixture_root = Path(temporary.name)
        self.static_root = self.fixture_root / "console"
        self.static_root.mkdir()
        self.outside = self.fixture_root / "console-outside"
        self.outside.mkdir()
        (self.outside / "secret.js").write_bytes(SECRET_BODY)
        (self.outside / "secret.js.gz").write_bytes(gzip.compress(SECRET_BODY, mtime=0))
        (self.outside / "hello.br").write_bytes(BROTLI_BODY)

        for relative in (HASHED_JS, HASHED_CSS, "main.js", "assets/main-1234567.js"):
            self.write_file(relative, BODY, compressed=True)
        self.write_file("gzip-only.js", BODY)
        self.write_file("gzip-only.js.gz", GZIP_BODY)
        self.write_file("tiny.js", b"x\n")
        self.write_file("favicon.svg", b'<svg xmlns="http://www.w3.org/2000/svg"/>\n')
        self.write_file("favicon.svg.gz", gzip.compress((self.static_root / "favicon.svg").read_bytes(), mtime=0))
        for relative in ("index.html", "page-a1B2c3D4.html"):
            self.write_file(relative, HTML_BODY)
            self.write_file(relative + ".gz", gzip.compress(HTML_BODY, mtime=0))
        # Orphan sidecars must not make a missing original into a public resource.
        self.write_file("orphan.js.gz", GZIP_BODY)
        self.write_file("orphan.js.br", BROTLI_BODY)

        app = self.fastapi(openapi_url=None, docs_url=None, redoc_url=None)
        app.mount("/console", self.static_class(directory=str(self.static_root)), name="console")
        self.client = self.httpx.AsyncClient(
            transport=self.httpx.ASGITransport(app=app),
            base_url="http://console-static.test",
            trust_env=False,
            follow_redirects=False,
        )
        # httpx normally supplies Accept-Encoding even when the caller omits it.
        self.client.headers.pop("accept-encoding", None)
        self.addAsyncCleanup(self.client.aclose)

    def write_file(self, relative, body, *, compressed=False):
        path = self.static_root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)
        if compressed:
            if body != BODY:
                raise ValueError("the fixed Brotli fixture represents BODY only")
            Path(str(path) + ".gz").write_bytes(GZIP_BODY)
            Path(str(path) + ".br").write_bytes(BROTLI_BODY)
        return path

    async def request(self, path=HASHED_JS, *, encoding=None, method="GET", headers=None):
        request_headers = dict(headers or {})
        if encoding is not None:
            request_headers["Accept-Encoding"] = encoding

        async def exchange():
            async with self.client.stream(method, "/console/" + path, headers=request_headers) as response:
                # aread()/content would let httpx decompress the response and
                # could hide a wrong sidecar or accidental double compression.
                body = b"".join([chunk async for chunk in response.aiter_raw()])
                return response, body

        return await asyncio.wait_for(exchange(), timeout=5)

    def assert_representation(self, response, body, encoding, expected=None):
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.headers.get("content-encoding"), encoding)
        if expected is None:
            expected = {None: BODY, "gzip": GZIP_BODY, "br": BROTLI_BODY}[encoding]
        self.assertEqual(body, expected)
        self.assertEqual(int(response.headers["content-length"]), len(expected))
        if encoding == "gzip":
            self.assertEqual(gzip.decompress(body), BODY)

    def assert_vary(self, response):
        vary = [value.strip().lower() for value in response.headers.get("vary", "").split(",")]
        self.assertEqual(vary.count("accept-encoding"), 1, response.headers)

    def cache_directives(self, response):
        return {part.strip().lower() for part in response.headers.get("cache-control", "").split(",")}

    async def test_brotli_wins_equal_quality_regardless_of_header_order(self):
        for header in ("gzip, br", "br, gzip", "br;q=0.7, gzip;q=0.7, identity;q=0.1"):
            with self.subTest(header=header):
                response, body = await self.request(encoding=header)
                self.assert_representation(response, body, "br")

    async def test_gzip_quality_wins_and_implicit_identity_is_only_a_fallback(self):
        for header in ("br;q=0.4, gzip;q=0.9, identity;q=0.1", "gzip;q=0.5"):
            with self.subTest(header=header):
                response, body = await self.request(encoding=header)
                self.assert_representation(response, body, "gzip")

    async def test_explicit_identity_with_higher_quality_wins(self):
        response, body = await self.request(encoding="br;q=0.4, gzip;q=0.7, identity;q=1")
        self.assert_representation(response, body, None)

    async def test_zero_invalid_and_duplicate_quality_values_disable_the_coding(self):
        cases = (
            ("br;q=0, gzip;q=1, identity;q=0.1", "gzip"),
            ("br;q=NaN, gzip;q=1, identity;q=0", "gzip"),
            ("br;q=inf, gzip;q=1, identity;q=0", "gzip"),
            ("br;q=-0.1, gzip;q=1, identity;q=0", "gzip"),
            ("br;q=1.1, gzip;q=1, identity;q=0", "gzip"),
            ("br;q=garbage, gzip;q=1, identity;q=0", "gzip"),
            ("br;q=1, BR;q=0, gzip;q=0.5, identity;q=0", "gzip"),
            ("BR;q=0, br;q=1, gzip;q=0.5, identity;q=0", "gzip"),
            ("br;q=NaN, BR;q=1, gzip;q=0.5, identity;q=0", "gzip"),
            ("br;q=0.9, BR;q=0.2, gzip;q=0.5, identity;q=0", "gzip"),
            ("br;q=1;q=0, gzip;q=0.5, identity;q=0", "gzip"),
            ("br;q=1, gzip;q=0.5, identity;q=1, IDENTITY;q=0", "br"),
        )
        for header, expected in cases:
            with self.subTest(header=header):
                response, body = await self.request(encoding=header)
                self.assert_representation(response, body, expected)

    async def test_excluded_compressions_fall_back_to_identity(self):
        response, body = await self.request(encoding="br;q=0, gzip;q=0")
        self.assert_representation(response, body, None)

    async def test_all_representations_excluded_returns_406(self):
        response, _ = await self.request(encoding="br;q=0, gzip;q=0, identity;q=0")
        self.assertEqual(response.status_code, 406)
        self.assert_vary(response)

    async def test_identity_excluded_and_missing_sidecars_returns_406(self):
        for method in ("GET", "HEAD"):
            with self.subTest(method=method):
                response, body = await self.request("tiny.js", encoding="br, gzip, identity;q=0", method=method)
                self.assertEqual(response.status_code, 406)
                if method == "HEAD":
                    self.assertEqual(body, b"")

    async def test_wildcard_does_not_override_explicit_exclusion(self):
        for header in ("*;q=0.8, br;q=0, identity;q=0", "br;q=0, identity;q=0, *;q=0.8"):
            with self.subTest(header=header):
                response, body = await self.request(encoding=header)
                self.assert_representation(response, body, "gzip")

    async def test_wildcard_zero_allows_only_explicitly_accepted_coding(self):
        response, body = await self.request(encoding="*;q=0, gzip;q=0.5")
        self.assert_representation(response, body, "gzip")
        response, _ = await self.request(encoding="*;q=0")
        self.assertEqual(response.status_code, 406)

    async def test_codings_and_quality_parameter_are_case_insensitive(self):
        response, body = await self.request(encoding=" bR ; Q=0.7 , GZIP ; q=1 , IDENTITY ; q=0.2 ")
        self.assert_representation(response, body, "gzip")

    async def test_missing_and_empty_accept_encoding_serve_identity(self):
        for header in (None, ""):
            with self.subTest(header=header):
                response, body = await self.request(encoding=header)
                if header is None:
                    self.assertNotIn("accept-encoding", response.request.headers)
                self.assert_representation(response, body, None)
                self.assert_vary(response)

    async def test_missing_brotli_sidecar_falls_back_to_gzip(self):
        response, body = await self.request("gzip-only.js", encoding="br, gzip;q=0.7, identity;q=0.1")
        self.assert_representation(response, body, "gzip")

    async def test_small_original_without_sidecars_is_served_uncompressed(self):
        response, body = await self.request("tiny.js", encoding="br, gzip")
        self.assert_representation(response, body, None, expected=b"x\n")
        self.assertFalse((self.static_root / "tiny.js.gz").exists())
        self.assertFalse((self.static_root / "tiny.js.br").exists())

    async def test_orphan_sidecars_do_not_resurrect_missing_original(self):
        for method in ("GET", "HEAD"):
            for header in ("br", "gzip", "br, gzip, identity;q=0"):
                with self.subTest(method=method, header=header):
                    response, body = await self.request("orphan.js", encoding=header, method=method)
                    self.assertEqual(response.status_code, 404)
                    if method == "HEAD":
                        self.assertEqual(body, b"")

    async def test_eight_character_hashed_assets_are_immutable(self):
        for path in (HASHED_JS, HASHED_CSS, HASHED_JS + "?v=123"):
            for header in ("identity", "gzip", "br"):
                with self.subTest(path=path, header=header):
                    response, _ = await self.request(path, encoding=header)
                    self.assertEqual(response.status_code, 200)
                    directives = self.cache_directives(response)
                    self.assertIn("immutable", directives)
                    self.assertIn("public", directives)
                    self.assertIn("max-age=31536000", directives)

    async def test_unhashed_and_short_hash_assets_cache_for_300_seconds(self):
        for path in ("main.js", "assets/main-1234567.js", "tiny.js", "favicon.svg"):
            with self.subTest(path=path):
                response, _ = await self.request(path, encoding="gzip, br")
                self.assertEqual(response.status_code, 200)
                directives = self.cache_directives(response)
                self.assertIn("public", directives)
                self.assertIn("max-age=300", directives)
                self.assertNotIn("immutable", directives)

    async def test_html_including_hashed_html_is_no_store(self):
        for path in ("index.html", "page-a1B2c3D4.html"):
            for header in ("identity", "gzip"):
                with self.subTest(path=path, header=header):
                    response, body = await self.request(path, encoding=header)
                    self.assertEqual(response.status_code, 200)
                    self.assertIn("no-store", self.cache_directives(response))
                    self.assertNotIn("immutable", self.cache_directives(response))
                    self.assertEqual(response.headers["content-type"].split(";")[0], "text/html")
                    decoded = (gzip.decompress(body)
                               if response.headers.get("content-encoding") == "gzip" else body)
                    self.assertEqual(decoded, HTML_BODY)

    async def test_head_has_get_metadata_and_an_empty_body(self):
        for header in ("identity", "gzip", "br"):
            with self.subTest(header=header):
                get_response, get_body = await self.request(encoding=header)
                head_response, head_body = await self.request(encoding=header, method="HEAD")
                self.assertEqual(get_response.status_code, 200)
                self.assertEqual(head_response.status_code, 200)
                self.assertEqual(head_body, b"")
                self.assertEqual(int(head_response.headers["content-length"]), len(get_body))
                for name in ("content-encoding", "content-type", "etag", "last-modified", "cache-control", "vary"):
                    self.assertEqual(head_response.headers.get(name), get_response.headers.get(name), name)

    async def test_etag_revalidation_returns_304_with_cache_and_vary(self):
        for header in ("identity", "gzip", "br"):
            with self.subTest(header=header):
                original, _ = await self.request(encoding=header)
                self.assertEqual(original.status_code, 200)
                etag = original.headers.get("etag")
                self.assertTrue(etag, "each representation needs an ETag")
                for method in ("GET", "HEAD"):
                    response, body = await self.request(encoding=header, method=method, headers={"If-None-Match": etag})
                    self.assertEqual(response.status_code, 304)
                    self.assertEqual(body, b"")
                    self.assertEqual(response.headers.get("etag"), etag)
                    self.assertEqual(response.headers.get("cache-control"), original.headers.get("cache-control"))
                    self.assert_vary(response)

    async def test_etag_is_specific_to_the_selected_representation(self):
        etags = {}
        for header in ("identity", "gzip", "br"):
            response, _ = await self.request(encoding=header)
            self.assertEqual(response.status_code, 200)
            etags[header] = response.headers["etag"]
        self.assertEqual(len(set(etags.values())), 3)
        for header, other in (("br", "identity"), ("identity", "gzip"), ("gzip", "br")):
            with self.subTest(header=header, previous=other):
                response, body = await self.request(encoding=header, headers={"If-None-Match": etags[other]})
                self.assert_representation(response, body, None if header == "identity" else header)

    async def test_compressed_responses_keep_original_mime_and_vary(self):
        for path, allowed_types in (
            (HASHED_JS, ("text/javascript", "application/javascript")),
            (HASHED_CSS, ("text/css",)),
            ("favicon.svg", ("image/svg+xml",)),
        ):
            identity, _ = await self.request(path, encoding="identity")
            self.assertEqual(identity.status_code, 200)
            self.assertIn(identity.headers["content-type"].split(";")[0], allowed_types)
            self.assert_vary(identity)
            for header in (("gzip",) if path.endswith(".svg") else ("gzip", "br")):
                with self.subTest(path=path, header=header):
                    response, _ = await self.request(path, encoding=header)
                    self.assertEqual(response.status_code, 200)
                    self.assertEqual(response.headers.get("content-encoding"), header)
                    self.assertEqual(response.headers["content-type"], identity.headers["content-type"])
                    self.assert_vary(response)

    async def test_traversal_encoded_separators_and_dangerous_paths_do_not_escape(self):
        paths = (
            "%2e%2e/console-outside/secret.js",
            "%2e%2e%2fconsole-outside%2fsecret.js",
            "%252e%252e%252fconsole-outside%252fsecret.js",
            "%2e%2e%5cconsole-outside%5csecret.js",
            "%2f%2e%2e%2fconsole-outside%2fsecret.js",
            "assets/%2e%2e/%2e%2e/console-outside/secret.js",
            "main.js%00",
            "%3b%24%28ignored%29/../console-outside/secret.js",
        )
        for path in paths:
            with self.subTest(path=path):
                response, body = await self.request(path, encoding="br, gzip")
                self.assertIn(response.status_code, (400, 404))
                self.assertNotIn(SECRET_BODY, body)
        self.assertEqual((self.outside / "secret.js").read_bytes(), SECRET_BODY)

    async def test_original_and_sidecar_symlinks_cannot_escape_root(self):
        links = (
            (self.static_root / "linked.js", self.outside / "secret.js", False),
            (self.static_root / "linked-dir", self.outside, True),
            (self.static_root / "inside.js.gz", self.outside / "secret.js.gz", False),
            (self.static_root / "inside.js.br", self.outside / "hello.br", False),
        )
        self.write_file("inside.js", b"inside the console root\n")
        self.write_file("linked.js.gz", GZIP_BODY)
        try:
            for link, target, is_directory in links:
                link.symlink_to(target, target_is_directory=is_directory)
        except OSError as error:
            if error.errno in (1, 13, 38) or getattr(error, "winerror", None) == 1314:
                self.skipTest(f"filesystem does not permit symlink fixtures: {error}")
            raise
        for path in ("linked.js", "linked-dir/secret.js"):
            with self.subTest(path=path):
                response, _ = await self.request(path, encoding="br, gzip")
                self.assertIn(response.status_code, (400, 404))
        for header in ("br, identity;q=0", "gzip, identity;q=0"):
            with self.subTest(header=header):
                response, _ = await self.request("inside.js", encoding=header)
                self.assertIn(response.status_code, (404, 406))
        self.assertEqual((self.outside / "secret.js").read_bytes(), SECRET_BODY)

    async def test_direct_and_repeated_compressed_suffix_requests_are_404(self):
        for suffix, original in ((".gz", GZIP_BODY), (".br", BROTLI_BODY)):
            path = HASHED_JS + suffix
            self.write_file(path + ".gz", gzip.compress(original, mtime=0))
            self.write_file(path + ".br", BROTLI_BODY)
            for requested in (path, path + ".gz", path + ".br"):
                for method in ("GET", "HEAD"):
                    with self.subTest(path=requested, method=method):
                        response, body = await self.request(requested, encoding="br, gzip", method=method)
                        self.assertEqual(response.status_code, 404)
                        self.assertIsNone(response.headers.get("content-encoding"))
                        if method == "HEAD":
                            self.assertEqual(body, b"")

    async def test_other_http_methods_are_rejected(self):
        for method in ("POST", "PUT", "DELETE"):
            with self.subTest(method=method):
                response, _ = await self.request(method=method, encoding="br, gzip")
                self.assertEqual(response.status_code, 405)
        self.assertEqual((self.static_root / HASHED_JS).read_bytes(), BODY)


    async def real_route_client(self):
        cls = type(self)
        if cls.route_code is None:
            target = self.module_root / "app" / "_app.py"
            base_path_target = self.module_root / "app" / "base_path.py"
            cls.route_code = compile_console_routes(target)
            cls.route_base_path = load_source_module("qwenpaw.app.base_path", base_path_target)
            for label, path in (("CONSOLE_ROUTE_SOURCE", target), ("BASE_PATH_MODULE", base_path_target)):
                print(f"{label}={path}", flush=True)
                print(f"{label}_SHA256={hashlib.sha256(path.read_bytes()).hexdigest()}", flush=True)

        from fastapi import HTTPException
        from fastapi.responses import FileResponse
        from fastapi.staticfiles import StaticFiles

        self.write_file("index.html", ROUTE_INDEX_BODY)
        # The application renders the original index before injecting base_path;
        # a stale sidecar must never replace that response.
        self.write_file("index.html.gz", gzip.compress(ROUTE_INDEX_BODY, mtime=0))
        self.write_file("logo.svg", LOGO_BODY)
        app = self.fastapi(openapi_url=None, docs_url=None, redoc_url=None)
        route_globals = {
            "__name__": "qwenpaw.app._console_route_regression",
            "__package__": "qwenpaw.app",
            "app": app,
            "os": os,
            "stat": stat,
            "Path": Path,
            "HTTPException": HTTPException,
            "FileResponse": FileResponse,
            "StaticFiles": StaticFiles,
            "ConsoleStaticFiles": self.static_class,
            "ConsoleIndexResponse": cls.route_base_path.ConsoleIndexResponse,
            "BasePathMiddleware": cls.route_base_path.BasePathMiddleware,
            "_CONSOLE_STATIC_DIR": str(self.static_root),
            "_CONSOLE_INDEX": self.static_root / "index.html",
            "_BASE_PATH": BASE_PATH,
            "PROJECT_NAME": "QwenPaw static route fixture",
        }
        # These are the original AST nodes, including decorators, mount and
        # middleware calls. No route handler or registration is replaced.
        exec(cls.route_code, route_globals)
        client = self.httpx.AsyncClient(
            transport=self.httpx.ASGITransport(app=app),
            base_url="http://console-routes.test",
            trust_env=False,
            follow_redirects=False,
        )
        client.headers.pop("accept-encoding", None)
        self.addAsyncCleanup(client.aclose)
        return client

    async def route_request(self, client, path, *, encoding=None, method="GET", headers=None):
        request_headers = dict(headers or {})
        if encoding is not None:
            request_headers["Accept-Encoding"] = encoding

        async def exchange():
            async with client.stream(method, path, headers=request_headers) as response:
                body = b"".join([chunk async for chunk in response.aiter_raw()])
                return response, body

        return await asyncio.wait_for(exchange(), timeout=5)

    async def test_routes_root_index_deep_links_and_console_alias_inject_base_path_no_store(self):
        client = await self.real_route_client()
        expected = ROUTE_INDEX_BODY.replace(
            b'<base href="/" />', f'<base href="{BASE_PATH}/">'.encode(),
        ).replace(b'window.__qpBase = "";', f'window.__qpBase = "{BASE_PATH}";'.encode())
        for path in ("/", "/index.html", "/settings/models", "/console/deep"):
            with self.subTest(path=path):
                response, body = await self.route_request(client, BASE_PATH + path, encoding="br, gzip")
                self.assertEqual(response.status_code, 200)
                self.assertEqual(response.headers["content-type"].split(";")[0], "text/html")
                self.assertIn("no-store", self.cache_directives(response))
                self.assertNotIn("immutable", self.cache_directives(response))
                self.assertIsNone(response.headers.get("content-encoding"))
                self.assertEqual(body, expected)
                self.assertEqual(int(response.headers["content-length"]), len(expected))
        self.assertEqual((self.static_root / "index.html").read_bytes(), ROUTE_INDEX_BODY)

    async def test_routes_plain_logo_uses_short_cache_and_preserves_head_contract(self):
        client = await self.real_route_client()
        response, body = await self.route_request(client, BASE_PATH + "/logo.svg", encoding="br, gzip")
        self.assert_representation(response, body, None, expected=LOGO_BODY)
        self.assertEqual(response.headers["content-type"].split(";")[0], "image/svg+xml")
        self.assertIn("public", self.cache_directives(response))
        self.assertIn("max-age=300", self.cache_directives(response))
        self.assertNotIn("immutable", self.cache_directives(response))
        head, head_body = await self.route_request(client, BASE_PATH + "/logo.svg", method="HEAD")
        # FastAPI @app.get currently answers HEAD with 405; an explicit HEAD
        # handler may later provide the standard empty successful response.
        self.assertIn(head.status_code, (200, 405))
        self.assertEqual(head_body, b"")
        if head.status_code == 200:
            self.assertEqual(head.headers.get("content-length"), response.headers["content-length"])
            self.assertEqual(head.headers.get("cache-control"), response.headers["cache-control"])

    async def test_routes_missing_assets_are_404_and_never_spa_html(self):
        client = await self.real_route_client()
        for method in ("GET", "HEAD"):
            with self.subTest(method=method):
                response, body = await self.route_request(
                    client, BASE_PATH + "/assets/missing.js", method=method, encoding="br, gzip",
                )
                self.assertEqual(response.status_code, 404)
                self.assertNotEqual(response.headers.get("content-type", "").split(";")[0], "text/html")
                self.assertNotIn(b"<!doctype html>", body)
                if method == "HEAD":
                    self.assertEqual(body, b"")

    async def test_routes_unknown_api_paths_remain_404(self):
        client = await self.real_route_client()
        for path in ("/api", "/api/missing", "/api/settings/models"):
            with self.subTest(path=path):
                response, body = await self.route_request(client, BASE_PATH + path, encoding="br, gzip")
                self.assertEqual(response.status_code, 404)
                self.assertNotEqual(response.headers.get("content-type", "").split(";")[0], "text/html")
                self.assertNotIn(b"<!doctype html>", body)

    async def test_routes_asset_mount_uses_real_console_compression_and_cache(self):
        client = await self.real_route_client()
        for header, expected in (("br, gzip", "br"), ("gzip", "gzip"), ("identity", None)):
            with self.subTest(header=header):
                response, body = await self.route_request(client, BASE_PATH + "/" + HASHED_JS, encoding=header)
                self.assert_representation(response, body, expected)
                self.assertIn(response.headers["content-type"].split(";")[0],
                              ("text/javascript", "application/javascript"))
                self.assertIn("public", self.cache_directives(response))
                self.assertIn("max-age=31536000", self.cache_directives(response))
                self.assertIn("immutable", self.cache_directives(response))
                self.assert_vary(response)

    async def test_routes_asset_mount_keeps_head_and_etag_304(self):
        client = await self.real_route_client()
        path = BASE_PATH + "/" + HASHED_JS
        for header in ("br", "gzip"):
            with self.subTest(header=header):
                response, body = await self.route_request(client, path, encoding=header)
                self.assert_representation(response, body, header)
                head, head_body = await self.route_request(client, path, encoding=header, method="HEAD")
                self.assertEqual(head.status_code, 200)
                self.assertEqual(head_body, b"")
                for name in ("content-encoding", "content-type", "content-length", "etag", "cache-control", "vary"):
                    self.assertEqual(head.headers.get(name), response.headers.get(name), name)
                etag = response.headers.get("etag")
                self.assertTrue(etag)
                conditional, conditional_body = await self.route_request(
                    client, path, encoding=header, headers={"If-None-Match": etag},
                )
                self.assertEqual(conditional.status_code, 304)
                self.assertEqual(conditional_body, b"")
                self.assertEqual(conditional.headers.get("etag"), etag)
                self.assertEqual(conditional.headers.get("cache-control"), response.headers["cache-control"])
                self.assert_vary(conditional)


def main(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
        allow_abbrev=False,
    )
    parser.add_argument("--module-root", type=Path, required=True,
                        help="prepared src/qwenpaw or installed site-packages/qwenpaw directory")
    args, unittest_args = parser.parse_known_args(argv)
    ConsoleStaticFilesTests.module_root = args.module_root.resolve()
    program = unittest.main(module=__name__, argv=[sys.argv[0], *unittest_args], verbosity=2, exit=False)
    return 0 if program.result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
