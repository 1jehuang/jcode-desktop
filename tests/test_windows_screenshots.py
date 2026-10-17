"""Portable contract tests for the native Windows fixture integration checks.

Win32 calls and subprocesses are mocked. Native package workflows remain the
acceptance gate for actual HWND restoration, rendering, and process discovery.
"""
import ctypes
from contextlib import redirect_stdout
import importlib.util
import io
import json
from pathlib import Path
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    'windows_screenshots_under_test', ROOT / 'scripts/windows_screenshots.py')
NATIVE = importlib.util.module_from_spec(SPEC)
with patch.object(ctypes, 'WinDLL', return_value=Mock(), create=True), \
     patch.object(ctypes, 'WINFUNCTYPE', return_value=lambda function: function, create=True):
    SPEC.loader.exec_module(NATIVE)


class WindowsIntegrationChecksTests(unittest.TestCase):
    def exercise(self, *, restores=True, minimizes=True, replacement=False,
                 duplicate=False, version_code=0, second_code=0, version_text='Jcode Desktop 1.0\n'):
        hwnd, pid = 123456, 101
        state = {'minimized': False, 'launched': False}
        native = Mock()
        native.IsWindow.side_effect = lambda window: window == hwnd and not (
            replacement and state['launched'])
        native.IsWindowVisible.return_value = True
        native.IsIconic.side_effect = lambda window: state['minimized']

        def show(window, mode):
            self.assertEqual((window, mode), (hwnd, 6))
            if minimizes:
                state['minimized'] = True
        native.ShowWindow.side_effect = show

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / 'package with spaces 雪' / 'jcode-desktop.exe'
            binary.parent.mkdir()
            pe = bytearray(4096)
            struct.pack_into('<I', pe, 0x3C, 128)
            struct.pack_into('<H', pe, 128 + 24 + 68, 2)
            binary.write_bytes(pe)
            log = root / 'localappdata' / 'Jcode Desktop' / 'jcode-desktop.log'
            log.parent.mkdir(parents=True)
            log.write_text('jcode-desktop started\n[unix 0] DEBUG fixture ready\n')
            app = SimpleNamespace(pid=pid, poll=lambda: None)
            env = {'fixture': 'isolated'}

            def launch(args, **kwargs):
                self.assertEqual(kwargs['env'], env)
                self.assertEqual(kwargs['cwd'], root)
                if args == [str(binary)]:
                    self.assertEqual(state['minimized'], minimizes,
                                     'minimize must happen before the secondary starts')
                    state['launched'] = True
                    if restores:
                        state['minimized'] = False
                    log.write_text(log.read_text() + 'secondary handoff diagnostics\n')
                    return SimpleNamespace(returncode=second_code, stdout=b'secondary process output\n')
                self.assertEqual(args, [str(binary), '--version'])
                return SimpleNamespace(returncode=version_code, stdout=version_text)

            def windows(owner):
                self.assertEqual(owner, pid)
                return [hwnd + 1 if replacement and state['launched'] else hwnd]

            with patch.object(NATIVE, 'user32', native), \
                 patch.object(NATIVE, 'windows_of', side_effect=windows), \
                 patch.object(NATIVE, 'wait_for_condition', side_effect=lambda check: bool(check())), \
                 patch.object(NATIVE, 'app_process_ids', return_value=[pid, 202] if duplicate else [pid]), \
                 patch.object(NATIVE.subprocess, 'run', side_effect=launch), \
                 redirect_stdout(io.StringIO()):
                ok = NATIVE.integration_checks('windows-integration', binary, hwnd, app, env, root, root)
            report = (root / 'windows-integration-checks.txt').read_text()
            self.assertEqual((root / 'windows-integration-second-launch.log').read_bytes(),
                             b'secondary process output\n')
            self.assertIn('secondary handoff diagnostics',
                          (root / 'windows-integration-desktop.log').read_text())
        return ok, report

    def test_handoff_must_restore_original_minimized_window(self):
        ok, report = self.exercise()
        self.assertTrue(ok, report)
        self.assertIn('PASS original window minimizes before handoff', report)
        self.assertIn('PASS second launch restores the same window', report)
        self.assertIn('PASS exactly one package app process', report)

    def test_zero_exit_without_restoring_cannot_pass(self):
        ok, report = self.exercise(restores=False)
        self.assertFalse(ok)
        self.assertIn('PASS second launch exits', report)
        self.assertIn('FAIL second launch restores the same window', report)

    def test_a_replacement_window_cannot_count_as_same_handle(self):
        ok, report = self.exercise(replacement=True)
        self.assertFalse(ok)
        self.assertIn('FAIL second launch restores the same window', report)
        self.assertIn('FAIL still exactly the original app window', report)

    def test_failed_minimize_precondition_cannot_pass(self):
        ok, report = self.exercise(minimizes=False)
        self.assertFalse(ok)
        self.assertIn('FAIL original window minimizes before handoff', report)

    def test_duplicate_package_process_without_window_cannot_pass(self):
        ok, report = self.exercise(duplicate=True)
        self.assertFalse(ok)
        self.assertIn('PASS still exactly the original app window', report)
        self.assertIn('FAIL exactly one package app process', report)

    def test_version_text_cannot_hide_nonzero_exit(self):
        ok, report = self.exercise(version_code=9)
        self.assertFalse(ok)
        self.assertIn('PASS --version prints through a pipe', report)
        self.assertIn('FAIL --version exits successfully code=9', report)

    def test_version_must_still_print_with_successful_exit(self):
        ok, report = self.exercise(version_text='')
        self.assertFalse(ok)
        self.assertIn('FAIL --version prints through a pipe', report)

    def test_failed_second_launch_cannot_pass(self):
        ok, report = self.exercise(second_code=7)
        self.assertFalse(ok)
        self.assertIn('FAIL second launch exits code=7', report)


class ProcessCensusTests(unittest.TestCase):
    def test_census_only_counts_exact_executable_and_sorts_pids(self):
        binary = ROOT / 'package with spaces' / 'jcode-desktop.exe'
        items = [{'ProcessId': 202, 'ExecutablePath': str(binary)},
                 {'ProcessId': 101, 'ExecutablePath': str(binary)},
                 {'ProcessId': 303, 'ExecutablePath': str(ROOT / 'other' / 'jcode-desktop.exe')},
                 {'ProcessId': 404, 'ExecutablePath': None}]
        result = SimpleNamespace(returncode=0, stdout=json.dumps(items), stderr='')
        with patch.object(NATIVE.subprocess, 'run', return_value=result) as run:
            self.assertEqual(NATIVE.app_process_ids(binary, {'fixture': 'isolated'}, ROOT), [101, 202])
        self.assertEqual(run.call_args.kwargs['timeout'], 15)
        self.assertEqual(run.call_args.kwargs['env'], {'fixture': 'isolated'})
        self.assertEqual(run.call_args.kwargs['encoding'], 'utf-8')
        self.assertIn('Get-CimInstance Win32_Process', run.call_args.args[0][-1])

    def test_single_result_and_empty_census(self):
        binary = ROOT / 'jcode-desktop.exe'
        for stdout, expected in [('', []), ('\n', []),
                                 (json.dumps({'ProcessId': 101, 'ExecutablePath': str(binary)}), [101])]:
            with self.subTest(stdout=stdout):
                result = SimpleNamespace(returncode=0, stdout=stdout, stderr='')
                with patch.object(NATIVE.subprocess, 'run', return_value=result):
                    self.assertEqual(NATIVE.app_process_ids(binary, {}, ROOT), expected)

    def test_census_failure_surfaces_diagnostics_instead_of_passing(self):
        result = SimpleNamespace(returncode=1, stdout='', stderr='CIM failed')
        with patch.object(NATIVE.subprocess, 'run', return_value=result):
            with self.assertRaisesRegex(RuntimeError, 'CIM failed'):
                NATIVE.app_process_ids(ROOT / 'jcode-desktop.exe', {}, ROOT)


class ForegroundAppTests(unittest.TestCase):
    def native(self, foreground=123, owner=101, topmost=False):
        native = Mock()
        native.GetWindowLongW.return_value = 8 if topmost else 0
        native.GetForegroundWindow.return_value = foreground
        native.GetWindowThreadProcessId.side_effect = lambda hwnd, pointer: setattr(pointer._obj, 'value', owner)
        native.IsWindowVisible.return_value = True
        native.IsIconic.return_value = False
        return native

    def test_requires_real_foreground_owner_and_records_evidence(self):
        native = self.native()
        with tempfile.TemporaryDirectory() as directory, patch.object(NATIVE, 'user32', native), \
             patch.object(NATIVE, 'windows_of', return_value=[123]):
            evidence = Path(directory) / 'foreground.json'
            guard = NATIVE.ForegroundApp(101, 123, evidence)
            guard.require()
            self.assertEqual(json.loads(evidence.read_text())['foreground_pid'], 101)
            guard.request_close()
            native.PostMessageW.assert_called_once_with(123, 0x0010, 0, 0)
            guard.close()
            guard.close()
            native.SetWindowPos.assert_called_once_with(123, -2, 0, 0, 0, 0, 0x13)

    def test_foreign_foreground_is_not_visual_evidence(self):
        native = self.native(foreground=999, owner=202)
        with tempfile.TemporaryDirectory() as directory, patch.object(NATIVE, 'user32', native), \
             patch.object(NATIVE, 'windows_of', return_value=[123]):
            guard = NATIVE.ForegroundApp(101, 123, Path(directory) / 'foreground.json')
            with self.assertRaisesRegex(RuntimeError, 'not the foreground'):
                guard.require()

    def test_recycled_window_is_never_closed_or_reordered(self):
        native = self.native()
        with patch.object(NATIVE, 'user32', native), patch.object(NATIVE, 'windows_of', return_value=[]):
            guard = NATIVE.ForegroundApp(101, 123, Path('unused.json'))
            with self.assertRaisesRegex(RuntimeError, 'unowned window'):
                guard.request_close()
            guard.close()
            native.PostMessageW.assert_not_called()
            native.SetWindowPos.assert_not_called()

    def test_preexisting_topmost_style_is_preserved(self):
        native = self.native(topmost=True)
        with patch.object(NATIVE, 'user32', native), patch.object(NATIVE, 'windows_of', return_value=[123]):
            NATIVE.ForegroundApp(101, 123, Path('unused.json')).close()
            native.SetWindowPos.assert_not_called()

    def test_failed_foreground_attempt_releases_only_app_topmost_style(self):
        native = self.native(foreground=999, owner=202)
        with tempfile.TemporaryDirectory() as directory, patch.object(NATIVE, 'user32', native), \
             patch.object(NATIVE, 'windows_of', return_value=[123]), \
             patch.object(NATIVE.time, 'monotonic', side_effect=[0, 0.1, 2]), \
             patch.object(NATIVE.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'Cannot foreground tested app'):
                NATIVE.foreground_app(101, Path(directory) / 'foreground.json', timeout=1)
            self.assertTrue(all(call.args[0] == 123 for call in native.SetWindowPos.call_args_list))
            self.assertEqual(native.SetWindowPos.call_args_list[-1].args[1], -2)
            native.PostMessageW.assert_not_called()


class WindowWaitTests(unittest.TestCase):
    def test_waits_for_asynchronous_native_restore(self):
        with patch.object(NATIVE.time, 'monotonic', side_effect=[0, 0.1, 0.2]), \
             patch.object(NATIVE.time, 'sleep') as sleep:
            check = Mock(side_effect=[False, True])
            self.assertTrue(NATIVE.wait_for_condition(check, timeout=1))
            self.assertEqual(check.call_count, 2)
            sleep.assert_called_once_with(0.05)

    def test_a_stuck_window_times_out(self):
        with patch.object(NATIVE.time, 'monotonic', side_effect=[0, 0.1, 1.1]), \
             patch.object(NATIVE.time, 'sleep'):
            self.assertFalse(NATIVE.wait_for_condition(lambda: False, timeout=1))


if __name__ == '__main__':
    unittest.main()
