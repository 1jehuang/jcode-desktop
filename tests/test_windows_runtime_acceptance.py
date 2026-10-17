"""Portable unit checks for the native Windows acceptance runner.

These test framing, failure detection and isolation contracts, not Windows itself.
The workflow is the native end-to-end gate.
"""
import importlib.util
import io
import json
from pathlib import Path
import unittest
import tempfile
from contextlib import ExitStack
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
        api = self.client(iter([{'v': 1, 'reply_to': 1, 'ev': 'hello_ok',
                                'capabilities': ['session_connections']}]))
        api.hello()
        hello = json.loads(api.file.getvalue())
        self.assertEqual(hello['req'], 'hello')
        self.assertEqual((hello['min_version'], hello['max_version']), (1, 1))
        self.assertFalse(hello['supports_user_response'])

    def test_observer_rejects_incomplete_presence_capability(self):
        for capabilities in ([], ['sessions']):
            api = self.client(iter([{'v': 1, 'reply_to': 1, 'ev': 'hello_ok',
                                    'capabilities': capabilities}]))
            with self.assertRaisesRegex(RuntimeError, 'session_connections'):
                api.hello()

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
        self.assertIn('group: windows-runtime-acceptance-${{ github.sha }}', workflow)
        self.assertIn('windows-11-arm', workflow)
        self.assertIn('jcode-transport', workflow)
        self.assertIn('include-hidden-files: true', workflow)
        self.assertIn('verify-release-package.py', workflow)
        self.assertIn('Get-FileHash', workflow)
        self.assertIn('inputs.runtime_ref || inputs.artifact_run || inputs.release_tag', workflow)
        self.assertIn('cancel-in-progress: false', workflow)

    def test_runner_never_starts_companions_manually_or_uses_offline_fixtures(self):
        source = (ROOT / 'scripts/windows-runtime-acceptance.py').read_text()
        self.assertEqual(source.count('subprocess.Popen('), 1)
        self.assertIn('args = [str(package / "jcode-desktop.exe")]', source)
        self.assertIn('subprocess.Popen(args, cwd=work', source)
        self.assertNotIn('JCODE_DESKTOP_SCREENSHOT=', source)
        self.assertIn('JCODE_NO_AUTO_UPDATE="1"', source)
        self.assertIn('no_reply=True', source)

    def test_source_build_runs_real_instance_regressions_before_packaging(self):
        workflow = (ROOT / '.github/workflows/windows-runtime-acceptance.yml').read_text()
        gate = workflow.split('      - name: Test native Desktop single-instance handoff\n', 1)[1]
        gate = gate.split('      - name: Build the actual Windows package', 1)[0]
        self.assertIn('if: inputs.build_source', gate)
        self.assertIn('--bin jcode-desktop', gate)
        self.assertIn("--target '${{ matrix.target }}'", gate)
        self.assertIn('host::instance::platform::tests', gate)
        self.assertIn('if ($LASTEXITCODE)', gate)
        self.assertNotIn('continue-on-error', gate)

    def test_release_gate_exercises_extracted_zip_before_upload(self):
        workflow = (ROOT / '.github/workflows/cross-platform-release.yml').read_text()
        gate = workflow.split('      - name: Smoke-test Windows package\n', 1)[1]
        gate = gate.split('      - name: Upload Windows runtime acceptance diagnostics', 1)[0]
        self.assertIn('Expand-Archive', gate)
        self.assertIn('windows-runtime-acceptance.py', gate)
        self.assertNotIn('Start-Sleep -Seconds 8', gate)
        self.assertNotIn('continue-on-error:', gate)
        self.assertIn('if ($LASTEXITCODE)', gate)
        self.assertIn('--states caption,caption-light,windows-integration,chat,diff,applet', gate)
        self.assertLess(workflow.index('windows-runtime-acceptance.py'),
                        workflow.index('Upload verified platform packages to private draft'))

    def test_on_demand_reuses_built_artifacts_and_separates_fixture_ui(self):
        workflow = (ROOT / '.github/workflows/windows-runtime-acceptance.yml').read_text()
        self.assertIn('artifact_run:', workflow)
        self.assertIn('gh run download', workflow)
        self.assertIn('source-run.json', workflow)
        self.assertIn('--states caption,caption-light,windows-integration,chat,diff,applet', workflow)
        self.assertIn("if: always() && inputs.ui_integration && env.PACKAGE_DIR != ''", workflow)
        self.assertLess(workflow.index('windows-runtime-acceptance.py'),
                        workflow.index('windows_screenshots.py'))

    def exercise_runner(self, fail_history=False, fail_foreground=False):
        launches, requests = [], []
        state = {'bridge': 20, 'cleanup_calls': 0, 'foregrounds': 0, 'visibility_checks': 0,
                 'window_closes': 0, 'normal_closes': 0, 'terminations': 0}
        apps = []
        class AppWindow:
            def require(self):
                state['visibility_checks'] += 1
            def close(self):
                state['window_closes'] += 1
            def request_close(self):
                state['normal_closes'] += 1
                apps[-1].returncode = 0
        def foreground(pid, evidence):
            self.assertEqual(pid, 30)
            state['foregrounds'] += 1
            if fail_foreground:
                raise RuntimeError('foreground blocked by another application')
            return AppWindow()
        class App:
            pid = 30
            def __init__(self, args, **kwargs):
                launches.append((args, kwargs))
                apps.append(self)
                self.returncode = None
            def poll(self):
                return self.returncode
            def terminate(self):
                state['terminations'] += 1
                self.returncode = 0
            def wait(self, **kwargs):
                return self.returncode
        class Client:
            def __init__(self, path, transcript):
                self.attached = False
            def close(self):
                pass
            def hello(self):
                return {'v': 1, 'ev': 'hello_ok', 'version': 1}
            def request(self, request, expected, **fields):
                requests.append((request, fields))
                session = {'session_id': 'real-session', 'user_connected': len(launches) >= 2}
                if request == 'list_sessions':
                    return {'sessions': [session]}
                if request == 'create_session':
                    self.attached = True
                    session['working_dir'] = fields['working_dir']
                    return {'session': session}
                if request == 'attach_session':
                    self.attached = True
                if request == 'detach_session':
                    self.attached = False
                if request == 'get_history':
                    if not self.attached:
                        raise RuntimeError('observer API is not attached')
                    if fail_history:
                        raise RuntimeError('primary history failure')
                    return {'messages': ['Windows native acceptance']}
                return {'ev': expected}
        def stop(pid):
            state['bridge'] = 21
        def census():
            if fail_history and launches:
                state['cleanup_calls'] += 1
                if state['cleanup_calls'] == 1:
                    raise RuntimeError('secondary cleanup failure')
            return []
        real_open = open
        def open_file(file, *args, **kwargs):
            if str(file).startswith('\\\\.\\pipe\\'):
                return io.BytesIO()
            return real_open(file, *args, **kwargs)
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            package = Path(directory) / 'package with spaces'
            package.mkdir()
            output = Path(directory) / 'evidence'
            for target, value in [('Api', Client), ('screenshot', lambda path: None),
                                  ('foreground_app', foreground),
                                  ('processes', lambda package: census()), ('stop', stop),
                                  ('runtime_processes', lambda package: ({'ProcessId': 10}, {'ProcessId': state['bridge']}))]:
                stack.enter_context(patch.object(NATIVE, target, value))
            stack.enter_context(patch.object(NATIVE.subprocess, 'Popen', App))
            stack.enter_context(patch.object(NATIVE.time, 'sleep', lambda seconds: None))
            stack.enter_context(patch('builtins.open', open_file))
            if fail_foreground:
                with self.assertRaisesRegex(RuntimeError, 'foreground blocked'):
                    NATIVE.run(package, output, 1)
            elif fail_history:
                with self.assertRaisesRegex(RuntimeError, 'primary history failure'):
                    NATIVE.run(package, output, 1)
            else:
                NATIVE.run(package, output, 1)
            report = json.loads((output / 'result.json').read_text())
            if report.get('success'):
                self.assertEqual(state['foregrounds'], 3)
                self.assertEqual(state['window_closes'], 3)
                self.assertEqual(state['visibility_checks'], 6)
                self.assertEqual(state['normal_closes'], 2)
                self.assertEqual(state['terminations'], 1)  # final cleanup only
        return report, launches, requests

    def test_complete_orchestration_uses_real_session_id_and_ui_attach(self):
        report, launches, requests = self.exercise_runner()
        self.assertTrue(report['success'])
        self.assertEqual(len(launches), 3)
        self.assertIn('--session=real-session', launches[1][0])
        self.assertTrue(any(item['name'] == 'Desktop UI attaches to requested session on warm relaunch'
                            for item in report['checks']))
        messages = [fields for name, fields in requests if name == 'send_message']
        self.assertEqual(len(messages), 1)
        self.assertTrue(messages[0]['no_reply'])
        self.assertEqual(messages[0]['session_id'], 'real-session')

    def test_cleanup_cannot_mask_primary_failure(self):
        report, _, _ = self.exercise_runner(fail_history=True)
        self.assertFalse(report['success'])
        self.assertIn('primary history failure', report['error'])
        self.assertTrue(any('secondary cleanup failure' in error for error in report['cleanup_errors']))

    def test_foreign_foreground_cannot_count_as_app_visual_evidence(self):
        report, launches, requests = self.exercise_runner(fail_foreground=True)
        self.assertFalse(report['success'])
        self.assertIn('foreground blocked', report['error'])
        self.assertEqual(len(launches), 1)
        self.assertFalse(any(name == 'create_session' for name, _ in requests))
        self.assertEqual(report['cleanup_errors'], [])

    def test_preexisting_package_processes_are_never_terminated(self):
        with tempfile.TemporaryDirectory() as directory:
            package = Path(directory) / 'package'
            output = Path(directory) / 'evidence'
            existing = [{'ProcessId': 123}]
            with patch.object(NATIVE, 'processes', return_value=existing), \
                 patch.object(NATIVE, 'stop') as stop, \
                 patch.object(NATIVE, 'screenshot'), \
                 patch.object(NATIVE.subprocess, 'Popen') as launch:
                with self.assertRaisesRegex(AssertionError, 'processes existed'):
                    NATIVE.run(package, output, 1)
                stop.assert_not_called()
                launch.assert_not_called()
            report = json.loads((output / 'result.json').read_text())
            self.assertFalse(report['success'])
            self.assertEqual(report['final_processes'], existing)


if __name__ == '__main__':
    unittest.main()
