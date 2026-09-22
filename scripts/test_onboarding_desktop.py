#!/usr/bin/env python3
"""Headless isolation and process-lifecycle regression tests. No real credentials."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name("onboarding-desktop.py")
SPEC = importlib.util.spec_from_file_location("onboarding_desktop", SCRIPT)
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)


class EnvironmentTests(unittest.TestCase):
    def test_allowlist_excludes_credentials_config_socket_and_fixture_overrides(self):
        root = Path("/private/profile")
        source = {key: "sensitive-parent-value" for key in (
            "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "JCODE_API_KEY", "GITHUB_TOKEN",
            "AWS_ACCESS_KEY_ID", "SSH_AUTH_SOCK", "HTTPS_PROXY", "CLAUDE_CONFIG_DIR",
            "CODEX_HOME", "JCODE_DESKTOP_CONFIG", "JCODE_DESKTOP_UI", "JCODE_API_SOCKET",
            "JCODE_HOME", "HOME", "JCODE_DESKTOP_SCREENSHOT", "DBUS_SESSION_BUS_ADDRESS",
            "XDG_CONFIG_HOME", "PYTHONPATH", "LD_PRELOAD", "BROWSER",
        )}
        source.update(DISPLAY=":88", WAYLAND_DISPLAY="wayland-1", XDG_RUNTIME_DIR="/run/user/123")
        env = launcher.isolated_environment(root, source)
        self.assertNotIn("sensitive-parent-value", env.values())
        self.assertEqual(env["WAYLAND_DISPLAY"], "/run/user/123/wayland-1")
        self.assertEqual(env["DISPLAY"], ":88")
        self.assertEqual(env["HOME"], str(root / "home"))
        self.assertEqual(env["JCODE_HOME"], str(root / "jcode"))
        self.assertEqual(env["JCODE_SOCKET"], str(root / "runtime/jcode.sock"))
        self.assertEqual(env["JCODE_API_SOCKET"], str(root / "runtime/jcode-api.sock"))
        self.assertEqual(env["XDG_RUNTIME_DIR"], str(root / "runtime"))
        self.assertEqual(env["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/private/profile/no-session-bus")
        self.assertNotIn("JCODE_DESKTOP_SCREENSHOT", env)
        self.assertNotIn("JCODE_DESKTOP_CONFIG", env)

    def test_absolute_wayland_socket_is_preserved(self):
        env = launcher.isolated_environment(Path("/private"), {"WAYLAND_DISPLAY": "/run/display"})
        self.assertEqual(env["WAYLAND_DISPLAY"], "/run/display")

    def test_relative_wayland_requires_original_runtime(self):
        with self.assertRaisesRegex(RuntimeError, "WAYLAND_DISPLAY"):
            launcher.isolated_environment(Path("/private"), {"WAYLAND_DISPLAY": "wayland-1"})

    def test_invalid_timeouts_fail_before_launch(self):
        for timeout in (0, -1, 121, float("inf"), float("nan")):
            with self.subTest(timeout=timeout), mock.patch.object(launcher.subprocess, "Popen") as spawn:
                with self.assertRaises(ValueError):
                    launcher.open_onboarding(timeout)
                spawn.assert_not_called()

    def test_runtime_directory_must_be_private_and_not_symlink(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            launcher.private_directory(root)
            root.chmod(0o755)
            with self.assertRaises(RuntimeError):
                launcher.private_directory(root)
            root.chmod(0o700)
            link = root / "link"
            link.symlink_to(root, target_is_directory=True)
            with self.assertRaises(RuntimeError):
                launcher.private_directory(link)

    def test_private_browser_uses_explicit_fresh_profile_and_never_remote(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / "browser").mkdir()
            with mock.patch.object(launcher.shutil, "which", return_value="/usr/bin/firefox"), \
                    mock.patch.object(launcher.subprocess, "Popen") as spawn:
                launcher.open_private_url(root, "https://jcode.sh/account")
                args = spawn.call_args.args[0]
                self.assertIn("--no-remote", args)
                self.assertIn("--profile", args)
                self.assertTrue(Path(args[args.index("--profile") + 1]).is_relative_to(root / "browser"))
                self.assertEqual(args[-1], "https://jcode.sh/account")

    def test_browser_failure_is_handled_without_system_opener_fallback(self):
        with mock.patch.object(launcher, "open_private_url", side_effect=RuntimeError("unavailable")), \
                contextlib.redirect_stderr(io.StringIO()) as errors:
            self.assertEqual(launcher.main(["--open-url", "/private", "https://jcode.sh"]), 0)
            self.assertIn("unavailable", errors.getvalue())

    def test_browser_rejects_non_web_links(self):
        with tempfile.TemporaryDirectory() as name:
            for url in ("file:///home/personal", "--profile", "mailto:person@example.com"):
                with self.assertRaises(RuntimeError):
                    launcher.open_private_url(Path(name), url)

    def test_internal_supervision_never_removes_an_arbitrary_private_directory(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            sentinel = root / "keep"
            sentinel.write_text("not disposable")
            with self.assertRaisesRegex(RuntimeError, "non-onboarding"):
                launcher.supervise(root, Path("/usr/bin/false"), 1)
            self.assertEqual(sentinel.read_text(), "not disposable")

    def test_readiness_requires_render_and_matching_peer_not_just_a_socket(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / "runtime").mkdir()
            with socket.socket(socket.AF_UNIX) as server:
                server.bind(str(root / "runtime/jcode-desktop.sock"))
                server.listen(5)
                self.assertFalse(launcher.host_ready(root, os.getpid()))
                (root / "desktop-state.txt").touch()
                with self.assertRaisesRegex(RuntimeError, "unexpected process"):
                    launcher.host_ready(root, os.getpid() + 1)
                self.assertTrue(launcher.host_ready(root, os.getpid()))

    def test_explicit_companion_must_exist_and_be_executable(self):
        self.assertEqual(launcher.runtime_binary({}, "/usr/bin/true"), Path("/usr/bin/true"))
        with self.assertRaisesRegex(RuntimeError, "not executable"):
            launcher.runtime_binary({}, "/nonexistent-jcode")

    def test_help_describes_real_flow_without_launching(self):
        with contextlib.redirect_stdout(io.StringIO()) as output, \
                mock.patch.object(launcher, "open_onboarding") as spawn:
            with self.assertRaises(SystemExit):
                launcher.main(["--help"])
            spawn.assert_not_called()
            self.assertIn("Alt+Shift+5", output.getvalue())
            self.assertIn("REAL first-run", output.getvalue())


FAKE_HOST = '''#!/usr/bin/python3
import json, os, pathlib, socket, subprocess, sys, time
root = pathlib.Path(os.environ["HOME"]).parent
(root / "observed.json").write_text(json.dumps({"env":dict(os.environ), "argv":sys.argv, "cwd":os.getcwd()}))
child = subprocess.Popen(["/usr/bin/python3", "-c", "import time; time.sleep(600)"], start_new_session=True)
(root / "descendant.pid").write_text(str(child.pid))
server = socket.socket(socket.AF_UNIX)
server.bind(str(root / "runtime/jcode-desktop.sock"))
server.listen()
(root / "desktop-state.txt").write_text("rendered production root")
while True:
    client, _ = server.accept()
    client.close()
'''


@unittest.skipUnless(os.name == "posix" and hasattr(os, "pidfd_open"), "Linux supervisor required")
class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ob-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "parent-home"
        (self.home / ".local/bin").mkdir(parents=True)
        (self.home / ".local/bin/jcode").symlink_to("/usr/bin/true")
        self.sentinel = self.home / "credentials-sentinel"
        self.sentinel.write_text("parent credentials must not be touched")
        self.host = self.root / "fake-desktop"
        self.host.write_text(FAKE_HOST)
        self.host.chmod(0o700)
        self.source = {"HOME": str(self.home), "XDG_RUNTIME_DIR": str(self.root),
                       "ANTHROPIC_API_KEY": "must-not-inherit", "JCODE_DESKTOP_SCREENSHOT": "1",
                       "JCODE_API_SOCKET": "/never-touch/main-api.sock", "PATH": "/usr/bin:/bin"}
        self.launched = []
        self.addCleanup(self.stop_all)

    def start(self):
        result = launcher.open_onboarding(5, self.host, self.source)
        self.launched.append(result)
        return result

    def stop_all(self):
        for result in self.launched:
            try:
                os.kill(result["supervisor_pid"], signal.SIGTERM)
            except ProcessLookupError:
                pass
            self.wait_removed(Path(result["profile"]))
            try:
                os.waitpid(result["supervisor_pid"], 0)
            except ChildProcessError:
                pass

    def wait_removed(self, path):
        deadline = time.monotonic() + 10
        while path.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(path.exists(), "private profile was not cleaned up")

    def test_real_command_clean_environment_and_independent_repeated_launch(self):
        first, second = self.start(), self.start()
        self.assertNotEqual(first["pid"], second["pid"])
        self.assertNotEqual(first["profile"], second["profile"])
        for result in (first, second):
            self.assertEqual(result["mode"], "real-first-run")
            profile = Path(result["profile"])
            observed = json.loads((profile / "observed.json").read_text())
            self.assertEqual(observed["argv"][1:], ["--no-hot-reload"])
            self.assertEqual(observed["cwd"], str(profile / "home"))
            self.assertNotIn("ANTHROPIC_API_KEY", observed["env"])
            self.assertNotIn("JCODE_DESKTOP_SCREENSHOT", observed["env"])
            self.assertEqual(observed["env"]["JCODE_API_SOCKET"], str(profile / "runtime/jcode-api.sock"))
            self.assertFalse((profile / "home/credentials-sentinel").exists())
        self.assertEqual(self.sentinel.read_text(), "parent credentials must not be touched")

    def test_closing_desktop_stops_detached_descendant_and_removes_only_its_profile(self):
        result = self.start()
        profile = Path(result["profile"])
        descendant = int((profile / "descendant.pid").read_text())
        os.kill(result["pid"], signal.SIGTERM)
        self.wait_removed(profile)
        with self.assertRaises(ProcessLookupError):
            os.kill(descendant, 0)
        self.assertTrue(self.sentinel.is_file())

    def test_failed_startup_cleans_fresh_profile(self):
        with self.assertRaisesRegex(RuntimeError, "before rendering"):
            launcher.open_onboarding(5, "/usr/bin/false", self.source)
        self.assertEqual(list(self.root.glob("jcode-onboarding-*")), [])
        self.assertTrue(self.sentinel.is_file())

    def test_timeout_stops_only_its_children_and_preserves_parent_data(self):
        self.host.write_text("#!/usr/bin/python3\nimport time\ntime.sleep(600)\n")
        with self.assertRaisesRegex(RuntimeError, "startup timeout"):
            launcher.open_onboarding(0.2, self.host, self.source)
        self.assertEqual(list(self.root.glob("jcode-onboarding-*")), [])
        self.assertTrue(self.sentinel.is_file())


if __name__ == "__main__":
    unittest.main()
