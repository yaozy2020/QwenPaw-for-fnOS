#!/usr/bin/env python3
"""Standard-library tests for isolated Console static source preparation."""
import hashlib
import importlib.util
from pathlib import Path
import unittest

TARGET = Path(__file__).with_name("run-console-static-tests.py")
SPEC = importlib.util.spec_from_file_location("console_static_runner", TARGET)
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class ConsoleStaticRunnerTests(unittest.TestCase):
    def test_source_fixture_has_the_pinned_upstream_digest(self):
        data = RUNNER.checked_bytes(RUNNER.HUB_FIXTURE, RUNNER.HUB_SHA256)
        self.assertEqual(hashlib.sha256(data).hexdigest(), RUNNER.HUB_SHA256)

    def test_preparation_copies_only_required_source_modules_into_external_shells(self):
        with RUNNER.external_work_directory(None) as work:
            self.assertFalse(work.is_relative_to(RUNNER.ROOT))
            package = RUNNER.prepare_package(work, RUNNER.HUB_FIXTURE)
            actual = {path.relative_to(package).as_posix() for path in package.rglob("*") if path.is_file()}
            self.assertEqual(actual, {*RUNNER.APP_MODULES, "hub/static_files.py", "__init__.py", "app/__init__.py", "hub/__init__.py"})
            for relative in RUNNER.APP_MODULES:
                self.assertEqual((package / relative).read_bytes(), (RUNNER.PATCH_ROOT / "payload" / relative).read_bytes())

    def test_modified_hub_fixture_is_rejected_before_package_creation(self):
        with RUNNER.external_work_directory(None) as work:
            source = work / "static_files.py"
            source.write_bytes(RUNNER.HUB_FIXTURE.read_bytes() + b"\n# altered source\n")
            with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                RUNNER.prepare_package(work, source)
            self.assertFalse((work / "src").exists())

    def test_work_directory_inside_repository_is_rejected_before_creation(self):
        target = RUNNER.ROOT / "console-static-runner-must-not-create"
        with self.assertRaisesRegex(ValueError, "outside the repository"):
            with RUNNER.external_work_directory(target):
                self.fail("repository directory was accepted")
        self.assertFalse(target.exists())

    def test_existing_work_directory_is_never_reused(self):
        with RUNNER.external_work_directory(None) as work:
            sentinel = work / "sentinel"
            sentinel.write_bytes(b"preserve")
            with self.assertRaises(FileExistsError):
                with RUNNER.external_work_directory(work):
                    self.fail("existing directory was reused")
            self.assertEqual(sentinel.read_bytes(), b"preserve")

    def test_requirements_lock_contains_only_exact_unique_pins(self):
        versions = RUNNER.locked_versions()
        self.assertEqual(versions["fastapi"], "0.163.1")
        self.assertEqual(versions["starlette"], "1.10.0")
        self.assertEqual(versions["httpx"], "0.28.1")
        self.assertEqual(versions["anyio"], "4.12.1")
        self.assertNotIn("qwenpaw", versions)
        self.assertNotIn("pytest", versions)


if __name__ == "__main__":
    unittest.main(verbosity=2)
