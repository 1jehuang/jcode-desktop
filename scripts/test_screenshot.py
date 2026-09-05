import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from screenshot import isolated_env


class ScreenshotIsolationTests(unittest.TestCase):
    def test_desktop_and_credentials_are_not_inherited(self):
        with patch.dict(os.environ, {
            "DISPLAY": ":0", "WAYLAND_DISPLAY": "wayland-live",
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/live/bus",
            "JCODE_HOME": "/private/jcode", "JCODE_DESKTOP_UI": "/private/ui.so",
            "OPENAI_API_KEY": "test-secret", "SSH_AUTH_SOCK": "/live/ssh",
        }):
            env = isolated_env(Path("/isolated"))
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "OPENAI_API_KEY",
                    "SSH_AUTH_SOCK", "JCODE_DESKTOP_UI"):
            self.assertNotIn(key, env)
        self.assertEqual(env["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/isolated/no-dbus")
        self.assertEqual(env["JCODE_HOME"], "/isolated/jcode")

    def test_all_mutable_state_is_private_and_fixture_is_offline(self):
        env = isolated_env(Path("/isolated"))
        for key in ("HOME", "XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME",
                    "XDG_DATA_HOME", "XDG_STATE_HOME", "JCODE_HOME", "JCODE_DESKTOP_STATE"):
            self.assertTrue(env[key].startswith("/isolated/"), key)
        self.assertEqual(env["JCODE_DESKTOP_SCREENSHOT"], "1")
        self.assertEqual(env["LIBGL_ALWAYS_SOFTWARE"], "1")


class ScreenshotArgumentTests(unittest.TestCase):
    def assert_rejected(self, arguments, message):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "should-not-exist.png"
            result = subprocess.run(
                [sys.executable, str(Path(__file__).with_name("screenshot.py")),
                 str(output), "--no-build", *arguments],
                text=True, capture_output=True, timeout=10,
            )
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn(message, result.stderr)
            self.assertFalse(output.exists())

    def test_invalid_panel_counts_are_rejected_before_launch(self):
        for count in (0, 7):
            with self.subTest(count=count):
                self.assert_rejected(["--panels", str(count)], "invalid choice")

    def test_unknown_theme_is_rejected_before_launch(self):
        self.assert_rejected(["--theme", "unknown"], "invalid choice")

    def test_unknown_transcript_is_rejected_before_launch(self):
        self.assert_rejected(["--transcript", "unknown"], "invalid choice")

    def test_focus_must_identify_a_displayed_panel(self):
        for index in (-1, 3):
            with self.subTest(index=index):
                self.assert_rejected(
                    ["--panels", "3", "--focus-panel", str(index)],
                    "focus-panel must identify",
                )

    @unittest.skipUnless(shutil.which("xdotool"), "native focus tool not installed")
    def test_focus_capture_rejects_offscreen_panel_coordinates(self):
        self.assert_rejected(
            ["--panels", "3", "--focus-panel", "0", "--size", "800x600"],
            "needs at least 320px",
        )


if __name__ == "__main__":
    unittest.main()
