import importlib.util
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().with_name("check-upstream.py")
SPEC = importlib.util.spec_from_file_location("check_upstream", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class UpstreamCheckTests(unittest.TestCase):
    def test_semver_update(self):
        report = MODULE.evaluate("Kimi Code", "2.7.3", "2.8.1", "https://example.com/feed")
        self.assertTrue(report["update_available"])
        self.assertEqual(report["status"], "update_available")

    def test_post_release_update(self):
        report = MODULE.evaluate("QwenPaw", "2.3.0", "2.3.0.post1", "https://example.com/feed")
        self.assertTrue(report["update_available"])

    def test_current_and_newer_local_versions_are_not_updates(self):
        for current, latest in (("2.3.0.post1", "2.3.0.post1"), ("2.4.0", "2.3.0.post1")):
            with self.subTest(current=current, latest=latest):
                self.assertFalse(MODULE.evaluate("QwenPaw", current, latest, "https://example.com/feed")["update_available"])

    def test_nested_feed_field(self):
        data = {"info": {"version": "2.3.0.post1"}}
        self.assertEqual(MODULE.feed_version(data, "info.version"), "2.3.0.post1")

    def test_prerelease_is_not_a_stable_input(self):
        with self.assertRaisesRegex(ValueError, "stable"):
            MODULE.version_key("2.4.0-rc.1")

    def test_current_lock_field(self):
        path = Path(self.enterContext(tempfile.TemporaryDirectory())) / "lock"
        path.write_text("# comment\nKIMI_VERSION=2.7.3\n", encoding="utf-8")
        self.assertEqual(MODULE.current_version(path, "KIMI_VERSION"), "2.7.3")


if __name__ == "__main__":
    unittest.main()
