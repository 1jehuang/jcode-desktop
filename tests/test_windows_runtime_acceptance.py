"""Portable unit checks for the native Windows acceptance runner.

These test framing, failure detection and isolation contracts, not Windows itself.
The workflow is the native end-to-end gate.
"""
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('windows_runtime', ROOT / 'scripts/windows-runtime-acceptance.py')
NATIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NATIVE)


class WindowsRuntimeAcceptanceTests(unittest.TestCase):
    def client(self, replies):
        api = object.__new__(NATIVE.Api)
        api.file = io.BytesIO()
        api.sequence = 0
        api.transcript = []
        api.receive = lambda deadline: next(replies)
        return api

    def test_pipe_names_match_windows_path_separator_and_case_rules(self):
        self.assertEqual(NATIVE.pipe_name(r'C:\Users\Test\run\jcode-api.sock'),
                         NATIVE.pipe_name('c:/users/test/run/jcode-api.sock'))
        self.assertRegex(NATIVE.pipe_name(r'C:\run\jcode-api.sock'), r'^\\\\.\\pipe\\jcode-api-[a-f0-9]{16}$')

    def test_pipe_hash_isolates_homes(self):
        self.assertNotEqual(NATIVE.pipe_name(r'C:\one\jcode-api.sock'),
                            NATIVE.pipe_name(r'C:\two\jcode-api.sock'))

    def test_correlates_replies_and_skips_unsolicited_events(self):
        api = self.client(iter([{'v': 1, 'ev': 'session_updated'},
                                {'v': 1, 'reply_to': 1, 'ev': 'pong'}]))
        self.assertEqual(api.request('ping', 'pong')['ev'], 'pong')
        self.assertEqual(json.loads(api.file.getvalue()), {'v': 1, 'id': 1, 'req': 'ping'})

    def test_harness_error_cannot_count_as_success(self):
        api = self.client(iter([{'v': 1, 'reply_to': 1, 'ev': 'error', 'message': 'daemon unavailable'}]))
        with self.assertRaisesRegex(RuntimeError, 'daemon unavailable'):
            api.request('create_session', 'attached')

    def test_rejects_incompatible_version(self):
        api = self.client(iter([{'v': 2, 'reply_to': 1, 'ev': 'pong'}]))
        with self.assertRaisesRegex(RuntimeError, 'Unexpected API version'):
            api.request('ping', 'pong')

    def test_hello_negotiates_version_and_real_client(self):
        api = self.client(iter([{'v': 1, 'reply_to': 1, 'ev': 'hello_ok'}]))
        api.hello()
        hello = json.loads(api.file.getvalue())
        self.assertEqual(hello['req'], 'hello')
        self.assertEqual((hello['min_version'], hello['max_version']), (1, 1))
        self.assertFalse(hello['supports_user_response'])

    def test_wait_retries_transient_disconnect(self):
        results = iter([OSError('pipe closed'), 'connected'])
        def check():
            result = next(results)
            if isinstance(result, Exception):
                raise result
            return result
        with patch.object(NATIVE.time, 'sleep'):
            self.assertEqual(NATIVE.wait_until(check, 1), 'connected')

    def test_wait_does_not_hide_desktop_crash(self):
        with self.assertRaisesRegex(AssertionError, 'Desktop exited'):
            NATIVE.wait_until(lambda: (_ for _ in ()).throw(AssertionError('Desktop exited')), 1)

    def test_workflow_is_on_demand_read_only_and_native_both_architectures(self):
        workflow = (ROOT / '.github/workflows/windows-runtime-acceptance.yml').read_text()
        self.assertIn('workflow_dispatch:', workflow)
        self.assertNotIn('  push:', workflow)
        self.assertNotIn('contents: write', workflow)
        self.assertIn('windows-2025', workflow)
        self.assertIn('windows-11-arm', workflow)
        self.assertIn('jcode-transport', workflow)
        self.assertIn('include-hidden-files: true', workflow)
        self.assertIn('verify-release-package.py', workflow)
        self.assertIn('Get-FileHash', workflow)

    def test_runner_never_starts_companions_manually_or_uses_offline_fixtures(self):
        source = (ROOT / 'scripts/windows-runtime-acceptance.py').read_text()
        self.assertEqual(source.count('subprocess.Popen('), 1)
        self.assertIn('subprocess.Popen([str(package / "jcode-desktop.exe")]', source)
        self.assertNotIn('JCODE_DESKTOP_SCREENSHOT=', source)
        self.assertIn('JCODE_NO_AUTO_UPDATE="1"', source)
        self.assertIn('no_reply=True', source)


if __name__ == '__main__':
    unittest.main()
