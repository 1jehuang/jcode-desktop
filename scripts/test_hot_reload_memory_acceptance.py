"""Pure/helper tests. No Desktop host, builds, reloads, or live display access."""
import contextlib
import io
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import hot_reload_memory_acceptance as harness


class GrowthTests(unittest.TestCase):
    def metrics(self, values):
        return harness.growth_metrics([{"seconds": i, "rss_mib": value}
                                       for i, value in enumerate(values)])

    def test_stable_linked_control_passes(self):
        metrics = self.metrics([239 + (i % 3) * .1 for i in range(31)])
        self.assertEqual(harness.growth_failures(metrics, 1, 32), [])
        self.assertEqual(metrics["duration_seconds"], 30)

    def test_observed_five_mib_per_second_leak_fails_both_limits(self):
        metrics = self.metrics([239 + 5 * i for i in range(31)])
        self.assertEqual(metrics["slope_mib_per_second"], 5)
        self.assertEqual(metrics["growth_mib"], 150)
        self.assertEqual(len(harness.growth_failures(metrics, 1, 32)), 2)

    def test_generation_mapping_cost_is_separate(self):
        for baseline in (240, 310, 380):
            self.assertEqual(harness.growth_failures(self.metrics([baseline] * 31), 1, 32), [])

    def test_peak_growth_not_hidden_by_later_collection(self):
        metrics = self.metrics([240] * 15 + [280] + [240] * 15)
        failures = harness.growth_failures(metrics, 1, 32)
        self.assertEqual(len(failures), 1)
        self.assertIn("growth", failures[0])

    def test_exact_threshold_fails(self):
        self.assertEqual(len(harness.growth_failures(
            {"slope_mib_per_second": 1, "growth_mib": 32}, 1, 32)), 2)

    def test_invalid_samples_rejected(self):
        for rows in ([], [{"seconds": 0, "rss_mib": 1}] * 2):
            with self.assertRaises(ValueError):
                harness.growth_metrics(rows)


class IsolationTests(unittest.TestCase):
    def test_host_diagnostics_waits_for_redirected_log(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "app.log").write_text("not the host activation log")
            self.assertEqual(harness.host_diagnostics(root), "")
            log = root / "logs/jcode-desktop/jcode-desktop.log"
            log.parent.mkdir(parents=True)
            expected = "activated UI generation 1 from fixture\nrolled back UI to fixture\n"
            log.write_text(expected)
            self.assertEqual(harness.host_diagnostics(root), expected)

    def test_mapping_count_deduplicates_elf_segments(self):
        lines = ["001-002 r-xp 0 00:00 5 /private/jcode-desktop-ui-abc/jcode-desktop-ui-0001.so",
                 "002-003 rw-p 0 00:00 5 /private/jcode-desktop-ui-abc/jcode-desktop-ui-0001.so",
                 "004-005 r-xp 0 00:00 6 /private/jcode-desktop-ui-abc/jcode-desktop-ui-0002.so (deleted)",
                 "006-007 r-xp 0 00:00 7 /private/pair/libjcode_desktop_ui.so",
                 "008-009 rw-p 0 00:00 0 [heap]"]
        paths = harness.plugin_mappings("\n".join(lines))
        self.assertEqual(len(paths), 2)
        self.assertTrue(paths[1].endswith("0002.so"))

    def test_pair_is_copied_and_pinned_not_linked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            host, plugin = root / "host", root / "plugin"
            host.write_bytes(b"release-host")
            plugin.write_bytes(b"release-plugin")
            host.chmod(0o755)
            manifest = harness.pin_pair(host, plugin, root / "pair")
            copied = root / "pair/jcode-desktop"
            self.assertNotEqual(copied.stat().st_ino, host.stat().st_ino)
            self.assertEqual(manifest["jcode-desktop"]["sha256"], harness.sha256(copied))
            self.assertTrue(os.access(copied, os.X_OK))
            host.write_bytes(b"changed source")
            self.assertEqual(copied.read_bytes(), b"release-host")

    def test_mutating_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            source.write_bytes(b"original")
            copy = harness.shutil.copy2

            def racing_copy(src, dst):
                result = copy(src, dst)
                src.write_bytes(b"replaced")
                return result

            with patch.object(harness.shutil, "copy2", side_effect=racing_copy):
                with self.assertRaisesRegex(RuntimeError, "changed while pinning"):
                    harness.pin_pair(source, source, root / "pair")

    def test_environment_does_not_inherit_display_or_secrets(self):
        with patch.dict(os.environ, {"DISPLAY": ":0", "SECRET_TOKEN": "private", "CARGO": "real-cargo"}):
            env = harness.isolated_env(Path("/private"))
        self.assertNotIn("DISPLAY", env)
        self.assertNotIn("SECRET_TOKEN", env)
        self.assertNotIn("CARGO", env)
        self.assertEqual(env["XDG_RUNTIME_DIR"], "/private/runtime")

    def test_cleanup_kills_uncooperative_child(self):
        child = subprocess.Popen([sys.executable, "-c",
                                  "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print('ready',flush=True); time.sleep(60)"],
                                 start_new_session=True, stdout=subprocess.PIPE)
        try:
            self.assertEqual(child.stdout.readline(), b"ready\n")
            harness.stop_processes([child])
            self.assertEqual(child.returncode, -signal.SIGKILL)
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            child.stdout.close()

    def test_cli_requires_no_build_and_real_acceptance_duration(self):
        base = ["--host", "/unused/host", "--plugin", "/unused/plugin"]
        for suffix in ([], ["--no-build", "--seconds", "1"],
                       ["--no-build", "--reloads", "1"],
                       ["--no-build", "--seconds", "nan"]):
            with self.subTest(suffix=suffix), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    harness.main(base + suffix)
                self.assertEqual(error.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
