#!/usr/bin/env python3
"""Headless stdlib tests, with only private Unix sockets and a mocked launcher."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import threading
import time
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name("onboarding-desktop.py")
SPEC = importlib.util.spec_from_file_location("onboarding_desktop", SCRIPT)
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)
WELCOME = {"ok": True, "step": "welcome"}


@unittest.skipUnless(hasattr(socket, "SO_PEERCRED"), "Linux peer credentials required")
class OnboardingTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.runtime = Path(temp.name)
        self.directory = self.runtime / "jcode-desktop-preview"
        self.directory.mkdir(mode=0o700)
        self.addCleanup(mock.patch.stopall)
        mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.runtime)}).start()
        self.popen = mock.patch.object(launcher.subprocess, "Popen").start()
        self.requests = []

    def server(self, path, response, main=False):
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(path))
        path.chmod(0o600)
        listener.listen()
        listener.settimeout(0.02)
        stop = threading.Event()
        errors = []

        def serve():
            while not stop.is_set():
                try:
                    client, _ = listener.accept()
                except socket.timeout:
                    continue
                try:
                    with client:
                        client.settimeout(1)
                        data = bytearray()
                        while (not data) if main else (b"\n" not in data):
                            chunk = client.recv(4096)
                            if not chunk:
                                break
                            data.extend(chunk)
                        if not data:  # Peer validation can reject before sending.
                            continue
                        self.requests.append((path.name, bytes(data)))
                        chunks = response if isinstance(response, list) else [response]
                        for chunk in chunks:
                            client.sendall(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Size-limit/deadline checks deliberately close early.
                except Exception as error:
                    errors.append(error)

        worker = threading.Thread(target=serve, daemon=True)
        worker.start()

        def cleanup():
            stop.set()
            worker.join(2)
            listener.close()
            self.assertFalse(worker.is_alive())
            self.assertEqual(errors, [])

        self.addCleanup(cleanup)
        return path

    def main_server(self, response=b"ok\n"):
        return self.server(self.runtime / "jcode-desktop.sock", response, main=True)

    def preview_server(self, result=WELCOME, pid=None, raw=None):
        return self.server(
            self.directory / f"{os.getpid() if pid is None else pid}.sock",
            json.dumps(result).encode() + b"\n" if raw is None else raw,
        )

    def deadline(self):
        return time.monotonic() + 1

    def test_existing_main_restored_and_welcome_reset_every_time(self):
        self.main_server([b"o", b"k", b"\n"])
        self.preview_server()
        self.preview_server(pid=999999)
        for _ in range(2):
            self.assertEqual(launcher.open_onboarding(1), WELCOME)
        self.assertEqual(self.requests, [
            ("jcode-desktop.sock", b"S"),
            (f"{os.getpid()}.sock", b'{"command":"onboarding"}\n'),
        ] * 2)
        self.assertEqual(self.popen.call_count, 2)
        self.popen.assert_called_with(
            [str(Path.home() / ".local/bin/jcode-desktop")],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True,
        )

    def test_delayed_startup_and_preview_retry_without_relaunch(self):
        with mock.patch.object(launcher, "main_host_pid", side_effect=[
            FileNotFoundError("starting"), 42, 43,
        ]) as discover, mock.patch.object(launcher, "request_onboarding", side_effect=[
            ConnectionRefusedError("UI loading"), WELCOME,
        ]) as request:
            self.assertEqual(launcher.open_onboarding(1), WELCOME)
        self.assertEqual(discover.call_count, 3)
        self.assertEqual([call.args[1] for call in request.call_args_list], [42, 43])
        self.popen.assert_called_once()

    def test_auxiliary_preview_never_used_when_main_absent(self):
        self.preview_server(pid=999999)
        start = time.monotonic()
        with self.assertRaisesRegex(RuntimeError, "timed out.*main Desktop"):
            launcher.open_onboarding(0.05)
        self.assertLess(time.monotonic() - start, 1)
        self.assertEqual(self.requests, [])

    def test_missing_main_preview_never_falls_back_to_auxiliary(self):
        self.main_server()
        self.preview_server(pid=999999)
        with self.assertRaisesRegex(RuntimeError, "No auxiliary preview was selected"):
            launcher.open_onboarding(0.05)
        self.assertTrue(self.requests)
        self.assertTrue(all(name == "jcode-desktop.sock" for name, _ in self.requests))

    def test_bad_or_missing_show_acknowledgement(self):
        path = self.main_server(b"no\n")
        with self.assertRaisesRegex(RuntimeError, "invalid Show acknowledgement"):
            launcher.main_host_pid(self.runtime, self.deadline())
        path.unlink()
        self.main_server(b"")
        with self.assertRaisesRegex(RuntimeError, "closed before acknowledging Show"):
            launcher.main_host_pid(self.runtime, self.deadline())

    def test_preview_must_confirm_exact_welcome_success(self):
        for result in [None, [], {"ok": 1, "step": "welcome"},
                       {"ok": True, "step": "provider"}, {"ok": False, "error": "unsupported"}]:
            with self.subTest(result=result):
                path = self.preview_server(result)
                with self.assertRaisesRegex(RuntimeError, "did not confirm Welcome"):
                    launcher.request_onboarding(self.runtime, os.getpid(), self.deadline())
                path.unlink()

    def test_malformed_truncated_and_oversize_preview_responses(self):
        for raw, error, message in [
            (b"bad json\n", ValueError, ""),
            (b'{"ok":true}', RuntimeError, "closed before acknowledging"),
            (b"x" * 65537, RuntimeError, "response too large"),
        ]:
            with self.subTest(raw_size=len(raw)):
                path = self.preview_server(raw=raw)
                with self.assertRaisesRegex(error, message):
                    launcher.request_onboarding(self.runtime, os.getpid(), self.deadline())
                path.unlink()

    def test_unsafe_preview_directory_and_socket_rejected(self):
        path = self.preview_server()
        for target, mode in [(self.directory, 0o755), (path, 0o666)]:
            with self.subTest(target=target):
                target.chmod(mode)
                with self.assertRaises(launcher.UnsafeEndpoint):
                    launcher.request_onboarding(self.runtime, os.getpid(), self.deadline())
                target.chmod(0o700 if target == self.directory else 0o600)
        self.assertEqual(self.requests, [])

    def test_symlink_and_regular_file_rejected(self):
        real = self.preview_server(pid=999999)
        target = self.directory / f"{os.getpid()}.sock"
        target.symlink_to(real)
        with self.assertRaises(launcher.UnsafeEndpoint):
            launcher.request_onboarding(self.runtime, os.getpid(), self.deadline())
        target.unlink()
        target.touch(mode=0o600)
        with self.assertRaises(launcher.UnsafeEndpoint):
            launcher.request_onboarding(self.runtime, os.getpid(), self.deadline())
        self.assertEqual(self.requests, [])

    def test_wrong_preview_peer_pid_rejected(self):
        self.preview_server(pid=999999)
        with self.assertRaisesRegex(launcher.UnsafeEndpoint, "unexpected peer"):
            launcher.request_onboarding(self.runtime, 999999, self.deadline())
        self.assertEqual(self.requests, [])

    def test_wrong_peer_uid_rejected(self):
        peer = mock.MagicMock()
        peer.getsockopt.return_value = struct.pack("3i", 123, os.getuid() + 1, 123)
        with mock.patch.object(launcher, "validate_path"), mock.patch.object(
            launcher.socket, "socket", return_value=peer
        ):
            with self.assertRaisesRegex(launcher.UnsafeEndpoint, "unexpected peer"):
                launcher.main_host_pid(self.runtime, self.deadline())
        peer.sendall.assert_not_called()
        peer.close.assert_called_once()

    def test_wrong_owner_rejected(self):
        metadata = self.directory.lstat()
        with mock.patch.object(launcher.os, "getuid", return_value=metadata.st_uid + 1):
            with self.assertRaises(launcher.UnsafeEndpoint):
                launcher.validate_path(self.directory, directory=True)

    def test_unsafe_main_socket_fails_without_retrying(self):
        path = self.main_server()
        path.chmod(0o666)
        with mock.patch.object(launcher.time, "sleep") as sleep:
            with self.assertRaises(launcher.UnsafeEndpoint):
                launcher.open_onboarding(1)
        sleep.assert_not_called()
        self.assertEqual(self.requests, [])

    def test_missing_or_relative_runtime_and_invalid_timeout_do_not_launch(self):
        for value in ["", "relative"]:
            with mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": value}):
                with self.assertRaisesRegex(RuntimeError, "XDG_RUNTIME_DIR"):
                    launcher.open_onboarding()
        for value in [0, -1, float("inf"), float("nan"), 121]:
            with self.assertRaisesRegex(ValueError, "timeout"):
                launcher.open_onboarding(value)
        self.popen.assert_not_called()

    def test_launch_failure_has_executable_context(self):
        self.popen.side_effect = FileNotFoundError("not installed")
        with self.assertRaisesRegex(RuntimeError, "could not launch .*jcode-desktop.*not installed"):
            launcher.open_onboarding(1)

    def test_deadline_caps_each_io_timeout(self):
        with mock.patch.object(launcher.time, "monotonic", return_value=10):
            self.assertAlmostEqual(launcher.remaining(10.05), 0.05)
            self.assertEqual(launcher.remaining(20), 3)
            with self.assertRaises(TimeoutError):
                launcher.remaining(10)

    def test_fragmented_preview_response(self):
        self.preview_server(raw=[b'{"ok":', b'true,"step":', b'"welcome"}\n'])
        self.assertEqual(
            launcher.request_onboarding(self.runtime, os.getpid(), self.deadline()), WELCOME
        )

    def test_stalled_main_is_bounded_by_overall_deadline(self):
        # A listening socket that never accepts or acknowledges Show.
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(listener.close)
        path = self.runtime / "jcode-desktop.sock"
        listener.bind(str(path))
        path.chmod(0o600)
        listener.listen()
        start = time.monotonic()
        with self.assertRaisesRegex(RuntimeError, "timed out after 0.05s"):
            launcher.open_onboarding(0.05)
        self.assertLess(time.monotonic() - start, 0.5)

    def test_cli_help_documents_binding_without_launch(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), self.assertRaises(SystemExit) as result:
            launcher.main(["--help"])
        self.assertEqual(result.exception.code, 0)
        self.assertIn("Alt+Shift+5", output.getvalue())
        self.assertIn("--timeout", output.getvalue())
        self.popen.assert_not_called()

    def test_cli_success_and_error_json(self):
        for response, expected in [(WELCOME, 0), (RuntimeError("failed"), 1)]:
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.object(launcher, "open_onboarding") as invoke:
                if expected:
                    invoke.side_effect = response
                else:
                    invoke.return_value = response
                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    self.assertEqual(launcher.main([]), expected)
            if expected:
                self.assertEqual(json.loads(stderr.getvalue()), {"ok": False, "error": "failed"})
                self.assertEqual(stdout.getvalue(), "")
            else:
                self.assertEqual(json.loads(stdout.getvalue()), WELCOME)
                self.assertEqual(stderr.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
