import importlib.util
import random
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location('perf_suite', Path(__file__).with_name('perf-suite.py'))
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)


def frame(draws=0, draw_ms=2.0, presents=0, present_ms=16.0):
    return dict(interval_ms=100, draw_count=draws, draw_mean_ms=draw_ms if draws else None,
                draw_p95_ms=draw_ms if draws else None, draw_max_ms=draw_ms if draws else None,
                animation_present_count=presents,
                animation_present_mean_ms=present_ms if presents else None,
                input_p95_ms=None, input_max_ms=None, ui_wake_lag_ms=1)


class PerfSuiteTests(unittest.TestCase):
    def test_every_scenario_has_keys_except_idle(self):
        for name in suite.SCENARIOS:
            keys = suite.scenario_keys(name, random.Random(1))
            self.assertEqual(name == 'idle', not keys, name)

    def test_stress_is_reproducible_and_closes_what_it_opens(self):
        a = suite.scenario_keys('stress', random.Random(7))
        self.assertEqual(a, suite.scenario_keys('stress', random.Random(7)))
        opened = sum(chord == suite.NEW for chord, _ in a)
        closed = sum(chord == suite.CLOSE for chord, _ in a)
        self.assertEqual(opened, closed)

    def test_spawn_close_opens_and_closes_the_same_number(self):
        keys = [chord for chord, _ in suite.scenario_keys('spawn-close', random.Random(0))]
        self.assertEqual(keys.count(suite.NEW), keys.count(suite.CLOSE))

    def test_fps_comes_from_presentation_intervals_weighted_by_count(self):
        before = dict(unix_ms=0, main_cpu_ticks=0, main_thread_ticks=0, rss_kb=1024)
        after = dict(unix_ms=1000, main_cpu_ticks=0, main_thread_ticks=0, rss_kb=1024)
        summary = suite.summarize([frame(4, 2.0, 3, 10.0), frame(1, 6.0, 1, 50.0)], before, after)
        self.assertAlmostEqual(summary['animation_fps'], 1000 / 20.0)
        self.assertAlmostEqual(summary['draw_mean_ms'], (4 * 2.0 + 6.0) / 5)

    def test_idle_has_no_fps(self):
        before = dict(unix_ms=0, main_cpu_ticks=0, main_thread_ticks=0, rss_kb=1024)
        after = dict(unix_ms=1000, main_cpu_ticks=0, main_thread_ticks=0, rss_kb=1024)
        self.assertIsNone(suite.summarize([frame()], before, after)['animation_fps'])

    def test_compare_flags_only_real_regressions(self):
        base = {'x': dict(draw_mean_ms=4.0, main_thread_cpu_percent=50.0, animation_fps=60.0)}
        same = {'x': dict(draw_mean_ms=4.2, main_thread_cpu_percent=52.0, animation_fps=58.0)}
        self.assertEqual(suite.compare(same, base, 0.15), [])
        worse = {'x': dict(draw_mean_ms=6.0, main_thread_cpu_percent=50.0, animation_fps=40.0)}
        problems = suite.compare(worse, base, 0.15)
        self.assertEqual(len(problems), 2)
        tiny = {'x': dict(draw_mean_ms=0.5, main_thread_cpu_percent=1.0, animation_fps=60.0)}
        self.assertEqual(suite.compare(tiny, {'x': dict(draw_mean_ms=0.2, main_thread_cpu_percent=0.6)}, 0.15), [])


if __name__ == '__main__':
    unittest.main()
