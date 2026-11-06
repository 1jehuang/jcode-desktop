#!/usr/bin/env python3
"""GPUI-free mocked Apple retries, included in release-policy discovery."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1] / 'scripts'
NETWORK = 'Error Domain=NSURLErrorDomain Code=-1005 "connection lost"'
OFFLINE = 'Error Domain=NSURLErrorDomain Code=-1009 "offline"'


class AppleRetryTests(unittest.TestCase):
    def run_mock(self, responses, command='apple_network_retry xcrun stapler validate "app with spaces.app"', sleep_status=0):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'responses').write_text(json.dumps(responses))
            mock = root / 'xcrun'
            mock.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ['MOCK_ROOT'])
with (root / 'calls').open('a') as log:
    log.write(json.dumps(sys.argv[1:]) + '\\n')
if sys.argv[1:3] == ['notarytool', 'log']:
    print('notary rejection detail', file=sys.stderr)
    sys.exit(99)
count_file = root / 'count'
count = int(count_file.read_text()) if count_file.exists() else 0
responses = json.loads((root / 'responses').read_text())
status, out, err = responses[min(count, len(responses) - 1)]
count_file.write_text(str(count + 1))
print(out, end='')
print(err, end='', file=sys.stderr)
sys.exit(status)
''')
            mock.chmod(0o755)
            sleep = root / 'sleep'
            sleep.write_text(f'#!/bin/bash\nprintf "%s\\n" "$1" >> "$MOCK_ROOT/sleeps"\nexit {sleep_status}\n')
            sleep.chmod(0o755)
            package = (SCRIPTS / 'package-macos.sh').read_text()
            notarize = package[package.index('notarize() {'):package.index('# Release tags')]
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', MOCK_ROOT=directory,
                       TMPDIR=directory, APPLE_NOTARY_PROFILE='test-profile')
            result = subprocess.run(['bash', '-c', f'set -euo pipefail\nsource "{SCRIPTS}/apple-network-retry.sh"\n{notarize}\n{command}\necho unexpected-continuation >&2'], env=env, capture_output=True, text=True)
            calls = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
            sleeps = (root / 'sleeps').read_text().splitlines() if (root / 'sleeps').exists() else []
            leftovers = list(root.glob('jcode-*'))
            self.assertEqual(leftovers, [])
            return result, calls, sleeps

    def test_transient_then_success_all_operations(self):
        for operation in ['notarytool submit', 'stapler staple', 'stapler validate']:
            for error in [NETWORK, OFFLINE, 'CloudKit: NetworkUnavailable', 'CKErrorDomain Code=4 network failure']:
                with self.subTest(operation=operation, error=error):
                    result, calls, sleeps = self.run_mock([(65, 'first output', error), (0, '{"status":"Accepted"}', '')], f'apple_network_retry xcrun {operation} "app with spaces.app"')
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout, '{"status":"Accepted"}')
                    self.assertEqual(len(calls), 2)
                    self.assertEqual(calls[0][-1], 'app with spaces.app')
                    self.assertEqual(sleeps, ['5'])

    def test_permanent_and_unknown_fail_once_exact_exit(self):
        for error in ['invalid signature', 'CloudKit record not found', 'HTTP 401 Unauthorized', 'unknown error', 'stapler exit 65', NETWORK + '\ninvalid signature']:
            with self.subTest(error=error):
                result, calls, sleeps = self.run_mock([(73, '', error)])
                self.assertEqual(result.returncode, 73)
                self.assertEqual(len(calls), 1)
                self.assertEqual(sleeps, [])
                self.assertNotIn('unexpected-continuation', result.stderr)

    def test_immediate_success_no_sleep(self):
        result, calls, sleeps = self.run_mock([(0, 'validated', '')])
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, 'validated')
        self.assertEqual(len(calls), 1)
        self.assertEqual(sleeps, [])

    def test_transient_then_rejection_stops(self):
        result, calls, sleeps = self.run_mock([(65, '', NETWORK), (77, '{"status":"Rejected"}', NETWORK)])
        self.assertEqual(result.returncode, 77)
        self.assertEqual(len(calls), 2)
        self.assertEqual(sleeps, ['5'])

    def test_notarize_exhaustion_preserves_failure_and_log(self):
        result, calls, sleeps = self.run_mock([(68, '{"id":"pending","status":"In Progress"}', NETWORK)], 'notarize "artifact.zip"')
        self.assertEqual(result.returncode, 68)
        self.assertEqual(len(calls), 6)
        self.assertEqual(calls[-1][:3], ['notarytool', 'log', 'pending'])
        self.assertEqual(sleeps, ['5', '10', '20', '40'])

    def test_exhaustion_preserves_last_exit(self):
        result, calls, sleeps = self.run_mock([(1, '', NETWORK), (2, '', NETWORK), (3, '', OFFLINE), (4, '', NETWORK), (69, '', NETWORK)])
        self.assertEqual(result.returncode, 69)
        self.assertEqual(len(calls), 5)
        self.assertEqual(sleeps, ['5', '10', '20', '40'])

    def test_interruption_does_not_retry(self):
        result, calls, sleeps = self.run_mock([(130, '', NETWORK)])
        self.assertEqual(result.returncode, 130)
        self.assertEqual(len(calls), 1)
        self.assertEqual(sleeps, [])

    def test_sleep_failure_is_preserved(self):
        result, calls, sleeps = self.run_mock([(65, '', NETWORK)], sleep_status=23)
        self.assertEqual(result.returncode, 23)
        self.assertEqual(len(calls), 1)
        self.assertEqual(sleeps, ['5'])

    def test_notarize_rejection_fetches_log_without_retry(self):
        for status in [0, 42]:
            with self.subTest(status=status):
                result, calls, sleeps = self.run_mock([(status, '{"id":"submission-id","status":"Invalid"}', NETWORK)], 'notarize "artifact.zip"')
                self.assertEqual(result.returncode, status or 1)
                self.assertEqual(len(calls), 2)
                self.assertEqual(calls[1][:3], ['notarytool', 'log', 'submission-id'])
                self.assertIn('notary rejection detail', result.stderr)
                self.assertEqual(sleeps, [])

    def test_notarize_retry_success_keeps_json(self):
        result, calls, sleeps = self.run_mock([(65, '{"id":"old"}', NETWORK), (0, '{"id":"new","status":"Accepted"}', '')], 'notarize "artifact.zip"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['id'], 'new')
        self.assertEqual(len(calls), 2)
        self.assertEqual(sleeps, ['5'])

    def test_scripts_wrap_only_expected_apple_operations(self):
        package = (SCRIPTS / 'package-macos.sh').read_text()
        verify = (SCRIPTS / 'verify-macos-package.sh').read_text()
        self.assertEqual(package.count('apple_network_retry xcrun notarytool submit'), 1)
        self.assertEqual(package.count('apple_network_retry xcrun stapler '), 4)
        self.assertEqual(verify.count('apple_network_retry xcrun stapler validate'), 2)
        for text in [package, verify]:
            self.assertNotIn('apple_network_retry codesign', text)
            self.assertNotIn('apple_network_retry spctl', text)
            self.assertIn('codesign --verify --deep --strict', text)
        self.assertIn('spctl --assess --type execute', verify)
        self.assertIn('spctl --assess --type open', verify)


if __name__ == '__main__':
    unittest.main()
