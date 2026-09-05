import os
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
