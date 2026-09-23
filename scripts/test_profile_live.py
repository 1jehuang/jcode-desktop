import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("profile_live", Path(__file__).with_name("profile-live.py"))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class ProfileLiveTests(unittest.TestCase):
    def test_exited_process_reports_a_lookup_error_not_missing_rss(self):
        with mock.patch.object(Path, 'read_text', side_effect=['(app) ' + '0 ' * 30, 'read_bytes: 0', 'State: Z']):
            with self.assertRaises(ProcessLookupError):
                profile.process_sample(42)

    def test_target_exit_preserves_partial_capture_and_marks_failure(self):
        scratch = Path(os.environ.get('JCODE_SCRATCH_DIR', 'target'))
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            output = root / 'capture'
            def first_sample(pid):
                capture_id = json.loads((root / 'jcode-desktop-profile.json').read_text())['capture_id']
                frame = dict(unix_ms=1000, input_frame_count=0, draw_count=1,
                             ui_wake_lag_ms=0, draw_max_ms=2, input_max_ms=None,
                             animation_present_p95_ms=None)
                (root / f'jcode-desktop-profile-{pid}-{capture_id}.jsonl').write_text(json.dumps(frame) + '\n')
                return dict(unix_ms=1000)
            def exited(_):
                raise ProcessLookupError()
            calls = iter([first_sample, exited])
            with mock.patch.dict(os.environ, XDG_RUNTIME_DIR=str(root)), \
                 mock.patch('sys.argv', ['profile-live.py', '--pid', '42', '--seconds', '5', '--output', str(output)]), \
                 mock.patch.object(profile.os, 'readlink', return_value='/app/jcode-desktop'), \
                 mock.patch.object(profile.time, 'sleep'), \
                 mock.patch.object(profile, 'process_sample', side_effect=lambda pid: next(calls)(pid)), \
                 contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(SystemExit) as stopped:
                    profile.main()
            self.assertEqual(stopped.exception.code, 1)
            self.assertTrue((output / 'frames.jsonl').exists())
            self.assertIn('stopped_early_reason', json.loads((output / 'capture.json').read_text()))
            self.assertEqual(json.loads((output / 'summary.json').read_text())['draws'], 1)
            self.assertFalse((root / 'jcode-desktop-profile.json').exists())

    def test_cadence_weights_intervals_and_excludes_idle(self):
        frames = [dict(draw_count=2, draw_mean_ms=3, animation_present_count=2,
                       animation_present_mean_ms=10, window_active=True, thermal_state='Nominal'),
                  dict(draw_count=1, draw_mean_ms=6, animation_present_count=1,
                       animation_present_mean_ms=20, window_active=False, thermal_state='Serious'),
                  dict(draw_count=0, animation_present_count=0, animation_present_mean_ms=None)]
        result = profile.cadence_summary(frames)
        self.assertAlmostEqual(result['animation_fps'], 75)
        self.assertEqual(result['draw_mean_ms'], 4)
        self.assertEqual(result['inactive_windows'], 1)
        self.assertEqual(result['thermal_states'], ['Nominal', 'Serious'])

    def test_legacy_capture_does_not_invent_mean_fps(self):
        result = profile.cadence_summary([dict(draw_count=20, animation_present_count=10,
                                                animation_present_p95_ms=16)])
        self.assertIsNone(result['animation_fps'])
        self.assertIsNone(result['draw_mean_ms'])

    def test_reload_samplers_are_reported_separately(self):
        scratch = Path(os.environ.get('JCODE_SCRATCH_DIR', 'target'))
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            base = dict(unix_ms=1000, pid=42, window='main', input_frame_count=0,
                        draw_count=2, ui_wake_lag_ms=0, draw_max_ms=3,
                        input_max_ms=None, animation_present_count=2,
                        animation_present_p95_ms=20)
            frames = [dict(base, sampler_id='old', animation_present_mean_ms=10),
                      dict(base, sampler_id='new', animation_present_mean_ms=20)]
            (root / 'frames.jsonl').write_text('\n'.join(map(json.dumps, frames)))
            (root / 'process.jsonl').write_text(json.dumps(dict(unix_ms=1000)))
            text = io.StringIO()
            with contextlib.redirect_stdout(text):
                summary = profile.analyze(root)
            cadence = summary['cadence_by_sampler']
            self.assertEqual(cadence['42/main/old']['animation_fps'], 100)
            self.assertEqual(cadence['42/main/new']['animation_fps'], 50)
            self.assertIn('not deduplicated frames', text.getvalue())

    def test_process_counters_are_monotonic(self):
        before = profile.process_sample(os.getpid())
        after = profile.process_sample(os.getpid())
        self.assertGreaterEqual(after["main_cpu_ticks"], before["main_cpu_ticks"])
        self.assertGreater(after["rss_kb"], 0)
        self.assertEqual(set(after["pressure"]), {"cpu", "io", "memory"})

    def test_idle_is_not_reported_as_successful_input_validation(self):
        self.check_analysis(input_count=0, input_max=None, expected_slow=0, inconclusive=True)

    def test_slow_input_is_reported_and_correlated_with_system_sample(self):
        self.check_analysis(input_count=2, input_max=85.0, expected_slow=1, inconclusive=False)

    def check_analysis(self, input_count, input_max, expected_slow, inconclusive):
        scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", "target"))
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            frame = {"unix_ms": 1000, "input_frame_count": input_count, "draw_count": 2,
                     "ui_wake_lag_ms": 1.0, "draw_max_ms": 3.0, "input_max_ms": input_max,
                     "animation_present_p95_ms": None}
            (root / "frames.jsonl").write_text(json.dumps(frame) + "\n")
            (root / "process.jsonl").write_text(json.dumps({"unix_ms": 990}) + "\n")
            text = io.StringIO()
            with contextlib.redirect_stdout(text):
                result = profile.analyze(root)
            self.assertEqual(result["input_over_50ms_windows"], expected_slow)
            self.assertEqual(result["input_frames"], input_count)
            self.assertEqual("INCONCLUSIVE FOR INPUT LAG" in text.getvalue(), inconclusive)
            self.assertIn("INCONCLUSIVE FOR SUSTAINED FPS", text.getvalue())
            self.assertEqual(json.loads((root / "summary.json").read_text()), result)


if __name__ == "__main__":
    unittest.main()
