import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("profile_live", Path(__file__).with_name("profile-live.py"))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class ProfileLiveTests(unittest.TestCase):
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
            self.assertEqual("INCONCLUSIVE" in text.getvalue(), inconclusive)
            self.assertEqual(json.loads((root / "summary.json").read_text()), result)


if __name__ == "__main__":
    unittest.main()
