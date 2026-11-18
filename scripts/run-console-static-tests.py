#!/usr/bin/env python3
"""Run the unchanged 31 Console static regressions in an external, isolated venv.

    python3 scripts/run-console-static-tests.py
    python3 scripts/run-console-static-tests.py --work-dir /tmp/console-static-ci
    python3 scripts/run-console-static-tests.py --prepare-only --work-dir /tmp/console-static-src

The tiny hub fixture is unmodified SOURCE from the pinned upstream commit, not
an installed/production package. --hub-source may select that same file from an
external source checkout; its SHA256 must still match. Only the three payload
app modules needed by test-console-static.py are copied. No QwenPaw initializer
or application lifespan is imported. --wheelhouse supports an offline wheel
cache; otherwise pip uses the official index with every dependency pinned.
An explicit --work-dir is retained for inspection; automatic temporary roots
are removed on exit. Both modes refuse a work directory inside this repository.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import importlib.metadata
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PATCH_ROOT = ROOT / "patches" / "qwenpaw-2.3.0-base-path"
REQUIREMENTS = ROOT / "scripts" / "requirements-console-static.txt"
UPSTREAM_COMMIT = "0a21644e90d8ba37e20035b3bfb63880872cc2a2"
UPSTREAM_COMMITS = {UPSTREAM_COMMIT, "ee58cb1a64f2b21e825c525f73dc44bc7941edea"}
HUB_SHA256 = "5ab63571200b202323b8164a473ce87fe3f4cca11edc8c4e6193f1a99df10d3c"
HUB_FIXTURE = ROOT / "scripts" / "testdata" / "console-static" / "hub" / "static_files.py"
HUB_SOURCE_URL = (
    "https://github.com/agentscope-ai/QwenPaw/blob/"
    f"{UPSTREAM_COMMIT}/src/qwenpaw/hub/static_files.py"
)
APP_MODULES = ("app/console_static.py", "app/base_path.py", "app/_app.py")
EXPECTED_TESTS = 31


def checked_bytes(path: Path, digest: str) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"expected a regular source file: {path}")
    data = path.read_bytes()
    actual = hashlib.sha256(data).hexdigest()
    if actual != digest:
        raise ValueError(f"SHA256 mismatch for {path}: expected {digest}, got {actual}")
    return data


def locked_versions() -> dict[str, str]:
    versions = {}
    for raw in REQUIREMENTS.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        match = re.fullmatch(r"([a-z0-9-]+)==([0-9][a-zA-Z0-9.+]*)", line)
        if not match or match[1] in versions:
            raise ValueError(f"expected one exact, unique version pin: {raw}")
        versions[match[1]] = match[2]
    if not versions:
        raise ValueError("empty Console static test dependency lock")
    return versions


def prepare_package(work: Path, hub_source: Path, patch_root: Path) -> Path:
    manifest = json.loads((patch_root / "manifest.json").read_text(encoding="utf-8"))
    upstream_commit = manifest.get("upstream_commit")
    if upstream_commit not in UPSTREAM_COMMITS:
        raise ValueError("payload upstream_commit is not a supported Console source")
    files = manifest["files"]
    app_sources = {}
    for relative in APP_MODULES:
        entries = [entry for entry in files if entry.get("path") == relative]
        if len(entries) != 1 or not re.fullmatch(r"[0-9a-f]{64}", entries[0].get("sha256", "")):
            raise ValueError(f"missing, duplicate or invalid payload manifest entry: {relative}")
        app_sources[relative] = checked_bytes(patch_root / "payload" / relative, entries[0]["sha256"])
    hub = checked_bytes(hub_source, HUB_SHA256)
    package = work / "src" / "qwenpaw"
    for directory in (package, package / "app", package / "hub"):
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "__init__.py").write_bytes(b"")
    for relative, data in app_sources.items():
        (package / relative).write_bytes(data)
    (package / "hub" / "static_files.py").write_bytes(hub)
    print(f"UPSTREAM_COMMIT={upstream_commit}", flush=True)
    print(f"HUB_SOURCE={hub_source}", flush=True)
    print(f"HUB_SOURCE_URL={HUB_SOURCE_URL}", flush=True)
    print(f"HUB_SHA256={HUB_SHA256}", flush=True)
    print(f"REQUIREMENTS_SHA256={hashlib.sha256(REQUIREMENTS.read_bytes()).hexdigest()}", flush=True)
    print(f"PREPARED_MODULE_ROOT={package}", flush=True)
    return package


@contextmanager
def external_work_directory(requested: Path | None):
    if requested is not None:
        work = requested.resolve()
        if work.is_relative_to(ROOT):
            raise ValueError("--work-dir must be outside the repository")
        work.mkdir(parents=True, exist_ok=False)
        yield work
        return
    candidates = [Path(value) for key in ("RUNNER_TEMP", "TMPDIR", "TEMP", "TMP")
                  if (value := os.environ.get(key))]
    candidates.extend((Path("/tmp"), Path("/var/tmp"),
                       Path(os.environ.get("SystemRoot", "C:/Windows")) / "Temp"))
    for candidate in candidates:
        directory = candidate.resolve()
        if not directory.is_dir() or directory.is_relative_to(ROOT):
            continue
        try:
            temporary = tempfile.TemporaryDirectory(prefix="qwenpaw-console-static-ci-", dir=directory)
        except PermissionError:
            continue
        with temporary:
            yield Path(temporary.name)
        return
    raise ValueError("no writable temporary directory outside the repository")


def run_suite(package: Path) -> int:
    if sys.version_info[:2] != (3, 12) or sys.prefix == sys.base_prefix:
        raise ValueError("the suite must run in its own Python 3.12 venv")
    for name, version in locked_versions().items():
        actual = importlib.metadata.version(name)
        if actual != version:
            raise ValueError(f"dependency version mismatch: {name}=={actual}, expected {version}")
        print(f"LOCKED_DEPENDENCY={name}=={actual}", flush=True)
    checked_bytes(package / "hub" / "static_files.py", HUB_SHA256)
    target = ROOT / "scripts" / "test-console-static.py"
    spec = importlib.util.spec_from_file_location("console_static_regressions", target)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {target}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.ConsoleStaticFilesTests.module_root = package
    suite = unittest.defaultTestLoader.loadTestsFromModule(module)
    if suite.countTestCases() != EXPECTED_TESTS:
        raise ValueError(f"expected {EXPECTED_TESTS} Console static cases, got {suite.countTestCases()}")
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if result.testsRun != EXPECTED_TESTS or result.skipped or not result.wasSuccessful():
        print("Console static gate requires all 31 cases to pass with no skips", file=sys.stderr)
        return 1
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work-dir", type=Path, help="new external directory to retain after testing")
    parser.add_argument("--hub-source", type=Path, default=HUB_FIXTURE,
                        help="unmodified hub/static_files.py from the pinned upstream source checkout")
    parser.add_argument("--patch-root", type=Path, default=PATCH_ROOT,
                        help="version-matched payload to test")
    parser.add_argument("--prepare-only", action="store_true", help="assemble source only; requires --work-dir")
    parser.add_argument("--wheelhouse", type=Path, help="install only from this prepopulated wheel directory")
    parser.add_argument("--_run-suite", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args._run_suite is not None:
        return run_suite(args._run_suite.resolve())
    if args.prepare_only and args.work_dir is None:
        parser.error("--prepare-only requires --work-dir so the prepared source remains available")
    if sys.version_info[:2] != (3, 12):
        parser.error("use Python 3.12 to match the locked test environment")
    locked_versions()
    with external_work_directory(args.work_dir) as work:
        package = prepare_package(work, args.hub_source.resolve(), args.patch_root.resolve())
        if args.prepare_only:
            return 0
        if importlib.util.find_spec("venv") is None or importlib.util.find_spec("ensurepip") is None:
            raise ValueError("Python venv/ensurepip are required; use a complete Python 3.12 installation")
        environment = dict(os.environ)
        temporary = work / "tmp"
        temporary.mkdir()
        environment.update({"TMPDIR": str(temporary), "TEMP": str(temporary), "TMP": str(temporary),
                            "PYTHONDONTWRITEBYTECODE": "1"})
        venv = work / "venv"
        subprocess.run([sys.executable, "-I", "-B", "-m", "venv", str(venv)],
                       env=environment, cwd=work, check=True)
        python = venv / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        pip = [str(python), "-I", "-B", "-m", "pip", "--isolated", "--disable-pip-version-check"]
        source = ["--index-url", "https://pypi.org/simple"]
        if args.wheelhouse is not None:
            source = ["--no-index", "--find-links", str(args.wheelhouse.resolve())]
        subprocess.run([*pip, "install", *source, "--only-binary=:all:", "--no-deps", "-r", str(REQUIREMENTS)],
                       env=environment, cwd=work, check=True)
        subprocess.run([*pip, "check"], env=environment, cwd=work, check=True)
        return subprocess.run([str(python), "-I", "-B", str(Path(__file__).resolve()),
                               "--_run-suite", str(package)], env=environment, cwd=work,
                              check=False).returncode


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, subprocess.CalledProcessError, importlib.metadata.PackageNotFoundError) as error:
        print(f"Console static runner failed: {error}", file=sys.stderr)
        raise SystemExit(1) from None
