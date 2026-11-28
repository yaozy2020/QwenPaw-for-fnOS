#!/usr/bin/env python3
"""Regression tests for official fnpack discovery and dual-build identity."""

import hashlib
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
from unittest.mock import patch

spec = spec_from_file_location("fnpack", Path(__file__).resolve().parents[1] / "tools/resolve-fnpack.py")
fnpack = module_from_spec(spec)
spec.loader.exec_module(fnpack)


def linux_url(version):
    return f"https://static2.fnnas.com/fnpack/fnpack-{version}-linux-amd64"


class FnpackTests(unittest.TestCase):
    def test_latest_official_stable_linux_amd64(self):
        links = [linux_url("1.9.9"), linux_url("1.10.0"), linux_url("2.0.0-rc.1"),
                 linux_url("3.0.0").replace("amd64", "arm64"),
                 linux_url("9.0.0").replace("static2.fnnas.com", "example.com")]
        html = "".join(f'<a href="{link}">Download</a>' for link in links)
        self.assertEqual(fnpack.latest_release(html), ("1.10.0", linux_url("1.10.0")))

    def test_new_release_records_observed_identity(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(fnpack, "build_opener") as opener:
            directory = Path(tmp)
            url = linux_url("1.4.0")
            binary = b"fixture"
            opener.return_value.open.side_effect = [io.BytesIO(f'<a href="{url}"></a>'.encode()), io.BytesIO(binary)]
            with redirect_stdout(io.StringIO()), patch.object(fnpack.subprocess, "check_output", return_value="Version 1.4.0\n"):
                fnpack.resolve(directory)
            self.assertEqual((directory / "fnpack").read_bytes(), binary)
            self.assertEqual(json.loads((directory / "fnpack.json").read_text()), {
                "version": "1.4.0", "url": url, "sha256": hashlib.sha256(binary).hexdigest(),
            })

    def test_missing_link(self):
        with self.assertRaisesRegex(ValueError, "缺少"):
            fnpack.latest_release("<html>Temporarily unavailable</html>")

    def test_tool_drift_between_builds(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            expected = directory / "previous.json"
            for version, digest in [("1.3.3", "any"), ("1.4.0", "changed")]:
                with self.subTest(version=version), patch.object(fnpack, "build_opener") as opener:
                    expected.write_text(json.dumps({"version": version, "url": linux_url(version), "sha256": digest}))
                    opener.return_value.open.side_effect = [
                        io.BytesIO(f'<a href="{linux_url("1.4.0")}"></a>'.encode()),
                        io.BytesIO(b"different bytes"),
                    ]
                    with self.assertRaisesRegex(ValueError, "重新构建"):
                        fnpack.resolve(directory, expected)
                    self.assertFalse((directory / "fnpack").exists())

    def test_known_version_digest(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(fnpack, "build_opener") as opener:
            opener.return_value.open.side_effect = [
                io.BytesIO(f'<a href="{linux_url("1.3.3")}"></a>'.encode()), io.BytesIO(b"changed")]
            with self.assertRaisesRegex(ValueError, "SHA256"):
                fnpack.resolve(Path(tmp))
            self.assertFalse((Path(tmp) / "fnpack").exists())


if __name__ == "__main__":
    unittest.main()
