"""Portable tests of hosted-only Windows desktop isolation and restoration."""
import ctypes
from contextlib import ExitStack
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('windows_ci_desktop', ROOT / 'scripts/windows-ci-desktop.py')
NATIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NATIVE)
HOSTED = {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted', 'RUNNER_OS': 'Windows'}


class PrivateDesktopTests(unittest.TestCase):
    def test_all_native_ui_workflows_use_guarded_wrapper(self):
        for workflow in ('windows-runtime-acceptance.yml', 'cross-platform-release.yml',
                         'windows-screenshots.yml'):
            source = (ROOT / '.github/workflows' / workflow).read_text()
            with self.subTest(workflow=workflow):
                self.assertGreaterEqual(source.count('scripts/windows-ci-desktop.py --evidence'), 2)
                for step in source.split('      - '):
                    if 'scripts/windows-ci-desktop.py --evidence' in step:
                        self.assertNotIn('continue-on-error:', step)
        policy = (ROOT / 'scripts/auto-release.py').read_text()
        self.assertIn('"scripts/windows-ci-desktop.py"', policy)

    def execute(self, child=0, switch_failure=False, restore_failure=False):
        user, kernel = Mock(), Mock()
        user.GetThreadDesktop.return_value = 100
        user.OpenInputDesktop.return_value = 200
        user.CreateDesktopW.return_value = 300
        user.SwitchDesktop.side_effect = [not switch_failure, not restore_failure]
        user.SetThreadDesktop.return_value = True
        user.CloseDesktop.return_value = True
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Path(temporary) / 'evidence.json'
            with ExitStack() as stack:
                stack.enter_context(patch.object(NATIVE.os, 'name', 'nt'))
                stack.enter_context(patch.dict(NATIVE.os.environ, HOSTED, clear=True))
                stack.enter_context(patch.object(NATIVE.ctypes, 'WinDLL', side_effect=[user, kernel], create=True))
                stack.enter_context(patch.object(NATIVE.ctypes, 'get_last_error', return_value=5, create=True))
                stack.enter_context(patch.object(NATIVE.ctypes, 'WinError', side_effect=lambda code: OSError(code, 'native failure'), create=True))
                run = stack.enter_context(patch.object(NATIVE, 'run_child', side_effect=child if isinstance(child, Exception) else None,
                                                       return_value=child))
                if isinstance(child, Exception) or switch_failure or restore_failure:
                    with self.assertRaises((RuntimeError, OSError)):
                        NATIVE.run(['python', 'native-check.py'], evidence)
                else:
                    self.assertEqual(NATIVE.run(['python', 'native-check.py'], evidence), child)
            report = json.loads(evidence.read_text())
        self.assertEqual(user.SwitchDesktop.call_args_list[-1].args, (200,))
        user.SetThreadDesktop.assert_called_once_with(100)
        self.assertEqual([call.args for call in user.CloseDesktop.call_args_list], [(300,), (200,)])
        return report, run

    def test_runs_unchanged_checks_and_restores_both_desktops(self):
        report, run = self.execute()
        self.assertTrue(report['success'])
        self.assertEqual(run.call_args.args[1], ['python', 'native-check.py'])
        self.assertTrue(run.call_args.args[2].startswith('JcodeAcceptance_'))
        self.assertIn('restored_input_desktop', report['events'])
        self.assertIn('restored_thread_desktop', report['events'])

    def test_nonzero_child_status_is_preserved(self):
        report, _ = self.execute(child=7)
        self.assertFalse(report['success'])
        self.assertEqual(report['child_returncode'], 7)

    def test_exception_still_restores_original_desktop(self):
        report, _ = self.execute(child=RuntimeError('test failure'))
        self.assertFalse(report['success'])
        self.assertIn('test failure', report['error'])

    def test_switch_failure_never_runs_child(self):
        report, run = self.execute(switch_failure=True)
        self.assertFalse(report['success'])
        run.assert_not_called()

    def test_restore_failure_cannot_pass(self):
        report, _ = self.execute(restore_failure=True)
        self.assertFalse(report['success'])
        self.assertTrue(report['cleanup_errors'])

    def test_rejects_every_nonhosted_environment_before_win32_calls(self):
        for environment in ({}, {**HOSTED, 'RUNNER_ENVIRONMENT': 'self-hosted'},
                            {**HOSTED, 'GITHUB_ACTIONS': 'false'}, {**HOSTED, 'RUNNER_OS': 'Linux'}):
            with self.subTest(environment=environment), patch.dict(NATIVE.os.environ, environment, clear=True), \
                 patch.object(NATIVE.os, 'name', 'nt'), \
                 patch.object(NATIVE.ctypes, 'WinDLL', create=True) as dll:
                with self.assertRaisesRegex(RuntimeError, 'only on disposable'):
                    NATIVE.run(['python', 'check.py'], None)
                dll.assert_not_called()

    def test_native_child_explicitly_sets_desktop_and_preserves_handles(self):
        kernel = Mock()
        kernel.GetStdHandle.side_effect = [10, 11, 12]
        kernel.WaitForSingleObject.return_value = 0
        def launch(*args):
            startup, process = args[-2]._obj, args[-1]._obj
            self.assertEqual(startup.lpDesktop, 'owned-desktop')
            self.assertEqual((startup.hStdInput, startup.hStdOutput, startup.hStdError), (10, 11, 12))
            self.assertEqual(startup.dwFlags, 0x100)
            self.assertEqual(args[5], 0x08000000)
            self.assertIn('path with spaces', args[1].value)
            process.hProcess, process.hThread = 100, 200
            return True
        kernel.CreateProcessW.side_effect = launch
        kernel.GetExitCodeProcess.side_effect = lambda handle, code: setattr(code._obj, 'value', 7) or True
        self.assertEqual(NATIVE.run_child(kernel, ['python', 'path with spaces/check.py'], 'owned-desktop', 30), 7)
        self.assertEqual([call.args for call in kernel.CloseHandle.call_args_list], [(200,), (100,)])
        kernel.TerminateProcess.assert_not_called()

    def test_timeout_terminates_only_the_owned_test_child(self):
        kernel = Mock()
        kernel.GetStdHandle.return_value = 1
        kernel.WaitForSingleObject.side_effect = [258, 0]
        def launch(*args):
            args[-1]._obj.hProcess, args[-1]._obj.hThread = 100, 200
            return True
        kernel.CreateProcessW.side_effect = launch
        with self.assertRaisesRegex(TimeoutError, 'exceeded 30s'):
            NATIVE.run_child(kernel, ['python', 'check.py'], 'owned-desktop', 30)
        kernel.TerminateProcess.assert_called_once_with(100, 1)
        self.assertEqual([call.args for call in kernel.CloseHandle.call_args_list], [(200,), (100,)])

    def test_native_timeout_terminates_only_owned_child_and_closes_handles(self):
        kernel = Mock()
        kernel.GetStdHandle.return_value = 1
        kernel.WaitForSingleObject.return_value = 258
        def launch(*args):
            process = args[-1]._obj
            process.hProcess, process.hThread = 301, 302
            return True
        kernel.CreateProcessW.side_effect = launch
        with self.assertRaisesRegex(TimeoutError, 'exceeded 30s'):
            NATIVE.run_child(kernel, ['python', 'check.py'], 'owned-desktop', 30)
        kernel.TerminateProcess.assert_called_once_with(301, 1)
        self.assertEqual([call.args for call in kernel.CloseHandle.call_args_list], [(302,), (301,)])


if __name__ == '__main__':
    unittest.main()
