#!/usr/bin/env python3
"""Headless subprocess tests for the preview-state CLI, using private Unix sockets."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import unittest

SCRIPT = Path(__file__).with_name("preview-state.py")


@unittest.skipUnless(hasattr(socket, "AF_UNIX") and hasattr(os, "getuid"), "Unix sockets required")
class PreviewStateCliTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name) / "jcode-desktop-preview"
        self.directory.mkdir(mode=0o700)
        self.env = {**os.environ, "XDG_RUNTIME_DIR": self.temp.name}
        self.requests = []

    def server(self, pid=123, response=None):
        path = self.directory / f"{pid}.sock"
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(path))
        path.chmod(0o600)
        listener.listen()
        listener.settimeout(0.05)
        stop = threading.Event()
        errors = []

        def serve():
            while not stop.is_set():
                try:
                    connection, _ = listener.accept()
                except socket.timeout:
                    continue
                try:
                    with connection:
                        connection.settimeout(1)
                        with connection.makefile("rb") as source:
                            request = json.loads(source.readline())
                        self.requests.append(request)
                        result = response if response is not None else {"ok": True, "states": []}
                        connection.sendall(json.dumps(result).encode() + b"\n")
                except Exception as error:
                    errors.append(error)

        worker = threading.Thread(target=serve, daemon=True)
        worker.start()

        def cleanup():
            stop.set()
            worker.join(timeout=2)
            listener.close()
            self.assertFalse(worker.is_alive(), "fake endpoint failed to stop")
            self.assertEqual(errors, [])

        self.addCleanup(cleanup)
        return path

    def run_cli(self, *args):
        return subprocess.run(
            ["python3", str(SCRIPT), *args], env=self.env,
            capture_output=True, text=True, timeout=5,
        )

    def test_discovery_list_open_reset_and_explicit_pid(self):
        self.server()
        for args in [
            ["--list"], ["login-error", "--pid", "123"],
            ["--reset", "login-error", "--pid", "123"], ["--list", "--pid", "123"],
        ]:
            result = self.run_cli(*args)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(json.loads(result.stdout)["ok"])
        self.assertEqual(self.requests, [
            {"command": "list"}, {"command": "list"},
            {"command": "open", "state": "login-error"},
            {"command": "reset", "state": "login-error"}, {"command": "list"},
        ])

    def test_ambiguous_discovery_requires_explicit_pid(self):
        self.server(123)
        self.server(456)
        result = self.run_cli("--list")
        self.assertEqual(result.returncode, 1)
        self.assertIn("multiple Desktop endpoints", json.loads(result.stderr)["error"])
        result = self.run_cli("--list", "--pid", "456")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_unsafe_directory_is_rejected_before_connecting(self):
        self.server()
        self.directory.chmod(0o755)
        result = self.run_cli("--list", "--pid", "123")
        self.assertEqual(result.returncode, 1)
        self.assertIn("mode 0700", json.loads(result.stderr)["error"])
        self.assertEqual(self.requests, [])

    def test_unsafe_socket_is_rejected_before_connecting(self):
        path = self.server()
        path.chmod(0o666)
        result = self.run_cli("--list", "--pid", "123")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unsafe preview socket", json.loads(result.stderr)["error"])
        self.assertEqual(self.requests, [])

    def test_socket_symlink_is_rejected(self):
        path = self.server()
        (self.directory / "456.sock").symlink_to(path)
        result = self.run_cli("--list", "--pid", "456")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unsafe preview socket", json.loads(result.stderr)["error"])
        self.assertEqual(self.requests, [])

    def test_server_error_propagates_with_nonzero_exit(self):
        response = {"ok": False, "error": "unknown preview state"}
        self.server(response=response)
        result = self.run_cli("missing", "--pid", "123")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout), response)
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
