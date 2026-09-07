import importlib.util
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location('profile_render', Path(__file__).with_name('profile-render.py'))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class RenderProfileTests(unittest.TestCase):
    def frame(self, draws=0):
        return dict(interval_ms=100, draw_count=draws, draw_max_ms=3 if draws else None,
                    draw_p95_ms=2 if draws else None, animation_present_count=draws,
                    animation_present_p95_ms=16 if draws else None,
                    input_frame_count=1 if draws else 0, input_max_ms=5 if draws else None,
                    ui_wake_lag_ms=1)

    def test_idle_does_not_invent_frame_latency(self):
        summary = profile.summarize([self.frame()], dict(unix_ms=100, main_cpu_ticks=1),
                                    dict(unix_ms=200, main_cpu_ticks=1))
        self.assertEqual(summary['draw_rate'], 0)
        self.assertIsNone(summary['draw_max_ms'])
        self.assertIsNone(summary['draw_window_p95_median_ms'])
        self.assertIsNone(summary['animation_present_max_window_p95_ms'])

    def test_rates_use_measured_time_not_configured_duration(self):
        summary = profile.summarize([self.frame(6), self.frame(6)],
                                    dict(unix_ms=100, main_cpu_ticks=1),
                                    dict(unix_ms=300, main_cpu_ticks=1))
        self.assertEqual(summary['draw_rate'], 60)
        self.assertEqual(summary['input_frames'], 2)
        self.assertEqual(summary['draw_window_p95_median_ms'], 2)
        self.assertEqual(summary['process_cpu_percent'], 0)

    def test_environment_does_not_inherit_user_display_or_credentials(self):
        env = profile.isolated_env(Path('/private/capture'))
        self.assertNotIn('DISPLAY', env)
        self.assertNotIn('WAYLAND_DISPLAY', env)
        self.assertNotIn('JCODE_API_SOCKET', env)
        self.assertEqual(env['XDG_RUNTIME_DIR'], '/private/capture/runtime')
        self.assertEqual(env['JCODE_HOME'], '/private/capture/jcode')


if __name__ == '__main__':
    unittest.main()
