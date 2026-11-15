import importlib.util
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location('perf_wayland_suite', Path(__file__).with_name('perf-suite.py'))
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)


class Clock:
    now = 0.0

    def sleep(self, seconds):
        self.now += seconds

    def sample(self, pid):
        return {'unix_ms': round(self.now * 1000)}


class WaylandHarnessTests(unittest.TestCase):
    def test_schedule_has_no_round_to_passes_or_duration_overshoot(self):
        keys = [('super+h', .35), ('super+l', .35)]
        schedule = suite.App._schedule(keys, 1)
        self.assertEqual(len(schedule), 3)
        self.assertAlmostEqual(sum(p for _, p in schedule), 1)
        self.assertEqual(suite.App._schedule([], 1), [])
        with self.assertRaises(ValueError):
            suite.App._schedule([('x', 0)], 1)

    def test_wtype_releases_modifiers_in_reverse_order(self):
        command = suite.App._wtype([('super+shift+h', .08)])
        self.assertEqual(command, ['wtype', '-s', '150', '-M', 'logo', '-M', 'shift',
                                   '-k', 'h', '-m', 'shift', '-m', 'logo', '-s', '80'])

    def test_stop_reaps_already_exited_and_kills_hung_process(self):
        exited = Mock()
        exited.poll.return_value = 0
        suite.App._stop(exited)
        exited.send_signal.assert_not_called()
        exited.wait.assert_called_once_with(timeout=10)
        hung = Mock()
        hung.poll.return_value = None
        hung.wait.side_effect = [subprocess.TimeoutExpired('wtype', 10), 0]
        suite.App._stop(hung)
        hung.send_signal.assert_called_once_with(signal.SIGTERM)
        hung.kill.assert_called_once()

    def test_startup_failure_cleans_started_children(self):
        with tempfile.TemporaryDirectory() as directory:
            aliases = []

            def fail(app):
                self.assertIn('layout_mode = "folder_tabs"', (app.root / 'desktop.toml').read_text())
                alias = Path(app.env['XDG_RUNTIME_DIR'])
                self.assertTrue(alias.is_dir())
                self.assertLess(len(str(alias / 'sway-ipc.1000.123456.sock')), 108)
                aliases.append(alias.parent)
                raise RuntimeError('failed')

            with patch.object(suite.App, '_start_sway', autospec=True, side_effect=fail):
                with self.assertRaisesRegex(RuntimeError, 'failed'):
                    suite.App(Path(directory) / ('nested-' * 15), Path('/unused'), True, 3, False, 'all',
                              'wayland', layout_mode='folder_tabs')
            self.assertFalse(aliases[0].exists())

    def measure(self, root, fail=False):
        app = suite.App.__new__(suite.App)
        app.root, app.env, app.display = root, {}, 'wayland'
        app.app = Mock(pid=123)
        app.app.poll.return_value = 1 if fail else None
        app.navigation = Mock(return_value=None)
        (root / 'runtime').mkdir()
        (root / 'runtime/jcode-desktop-profile-123-idle.jsonl').write_text(
            '\n'.join(json.dumps(f) for f in [
                {'unix_ms': 2000, 'interval_ms': 100},  # straddles start, exclude
                {'unix_ms': 2100, 'interval_ms': 100},
                {'unix_ms': 2300, 'interval_ms': 100},  # after end, exclude
            ]))
        keyboard = Mock()
        keyboard.poll.return_value = None
        clock = Clock()
        with patch.object(suite, 'time') as time, patch.object(suite, 'process_sample', side_effect=clock.sample), patch.object(suite.subprocess, 'Popen', return_value=keyboard) as launch, patch.object(suite, 'summarize', return_value={}) as summarize:
            time.monotonic.side_effect = lambda: clock.now
            time.time.side_effect = lambda: clock.now
            time.sleep.side_effect = clock.sleep
            if fail:
                with self.assertRaisesRegex(RuntimeError, 'App exited'):
                    app.measure('idle', [], .3)
                result = None
            else:
                result = app.measure('idle', [], .3)
                self.assertEqual(summarize.call_args.args[0], [{'unix_ms': 2100, 'interval_ms': 100}])
                self.assertAlmostEqual(result['elapsed_seconds'], .3)
                self.assertEqual(result['actions'], 0)
                self.assertEqual(result['action_count_kind'], 'scheduled')
            launch.assert_called_once()
            self.assertEqual(launch.call_args.args[0], ['wtype', '-s', '1000', '-s', '2300'])
            keyboard.send_signal.assert_called_once_with(signal.SIGTERM)
            keyboard.wait.assert_called_once()
            self.assertFalse((root / 'runtime/jcode-desktop-profile.json').exists())
        return result

    def test_idle_keeps_one_keyboard_through_measurement_and_filters_intervals(self):
        with tempfile.TemporaryDirectory() as directory:
            self.measure(Path(directory))

    def test_measurement_exception_removes_control_and_reaps_keyboard(self):
        with tempfile.TemporaryDirectory() as directory:
            self.measure(Path(directory), fail=True)

    def test_missing_cpu_profile_is_failure_and_both_children_are_reaped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'runtime').mkdir()
            app = suite.App.__new__(suite.App)
            app.root, app.env, app.display = root, {}, 'wayland'
            app.app = Mock(pid=123)
            app.app.poll.return_value = None
            app.navigation = Mock(return_value=None)
            sampler, keyboard = Mock(), Mock()
            sampler.poll.return_value = keyboard.poll.return_value = None
            clock = Clock()
            with patch.object(suite, 'time') as time, patch.object(suite, 'process_sample', side_effect=clock.sample), patch.object(suite.subprocess, 'Popen', side_effect=[sampler, keyboard]) as launch:
                time.monotonic.side_effect = lambda: clock.now
                time.time.side_effect = lambda: clock.now
                time.sleep.side_effect = clock.sleep
                with self.assertRaisesRegex(RuntimeError, 'did not save a profile'):
                    app.measure('idle', [], .3, root / 'missing.json.gz')
                self.assertTrue(launch.call_args_list[0].kwargs['stderr'].closed)
                sampler.send_signal.assert_called_once_with(signal.SIGINT)
                keyboard.send_signal.assert_called_once_with(signal.SIGTERM)
                self.assertFalse((root / 'runtime/jcode-desktop-profile.json').exists())

    def test_width_setup_waits_for_state_before_sending_another_cycle(self):
        app = suite.App.__new__(suite.App)
        widths, focus, pending, cycles = [.25, .25], [0], [], []

        def keys(*chords):
            for chord in chords:
                if chord == suite.WIDTH:
                    cycles.append(focus[0])
                    pending[:] = [focus[0], 3]
                elif chord == suite.FOCUS['right']:
                    focus[0] = min(1, focus[0] + 1)
                elif chord == suite.FOCUS['left']:
                    focus[0] = max(0, focus[0] - 1)

        def navigation():
            if pending:
                pending[1] -= 1
                if pending[1] == 0:
                    widths[pending[0]] = .5
                    pending.clear()
            return {'focused_slot': focus[0], 'rows': [{'panels': [
                {'slot': i, 'width': width, 'focused': i == focus[0]}
                for i, width in enumerate(widths)]}]}

        app.keys, app.navigation = keys, navigation
        clock = Clock()
        with patch.object(suite, 'time') as time:
            time.monotonic.side_effect = lambda: clock.now
            time.sleep.side_effect = clock.sleep
            app._prepare(False, 2, .5)
        self.assertEqual(cycles, [0, 1])
        self.assertEqual(widths, [.5, .5])


if __name__ == '__main__':
    unittest.main()
