#!/usr/bin/env python3
"""Headless unit checks for acceptance evidence, not native rendering proof."""
import importlib.util
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location('voice_accept', Path(__file__).with_name('accept-global-voice-overlay.py'))
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)

PROTOCOL = '''[1] -> zwlr_layer_shell_v1#8.get_layer_surface(new id zwlr_layer_surface_v1#42, wl_surface#40, nil, 3, "jcode-voice-overlay")
[2] -> zwlr_layer_surface_v1#42.set_keyboard_interactivity(0)
[3] -> zwlr_layer_surface_v1#42.set_exclusive_zone(0)
'''


class AcceptanceEvidenceTests(unittest.TestCase):
    def test_native_protocol(self):
        self.assertEqual(harness.assert_native_protocol(PROTOCOL), ['42'])
        self.assertEqual(harness.assert_native_protocol(PROTOCOL.replace('#', '@')), ['42'])

    def test_reject_normal_window_or_wrong_layer(self):
        for log in ('xdg_toplevel#42.set_title("voice")', PROTOCOL.replace(', 3,', ', 2,')):
            with self.assertRaises(AssertionError):
                harness.assert_native_protocol(log)

    def test_reject_focus_and_reservations(self):
        for log in (PROTOCOL.replace('interactivity(0)', 'interactivity(1)'),
                    PROTOCOL.replace('zone(0)', 'zone(64)'),
                    PROTOCOL.replace('zone(0)', 'zone(-1)'),
                    PROTOCOL + 'zwlr_layer_surface_v1#42.set_exclusive_zone(50)'):
            with self.assertRaises(AssertionError):
                harness.assert_native_protocol(log)

    def test_unrelated_surface_cannot_satisfy_policy(self):
        with self.assertRaises(AssertionError):
            harness.assert_native_protocol(PROTOCOL.replace('#42.set_', '#99.set_'))

    def test_focus_and_geometry_evidence(self):
        target = {'app_id': 'voice-overlay-accept-target', 'id': 8,
                  'focused': True, 'rect': {'width': 1280, 'height': 800},
                  'fullscreen_mode': 1}
        tree = {'nodes': [{'floating_nodes': [target]}]}
        self.assertEqual(harness.target_state(tree)['rect'], target['rect'])
        target['focused'] = False
        with self.assertRaises(AssertionError):
            harness.target_state(tree)
        with self.assertRaises(AssertionError):
            harness.target_state({'nodes': []})

    def test_rgb_diff_detects_overlay_and_restoration(self):
        from PIL import Image
        baseline = Image.new('RGB', (1280, 800), '#18202a')
        changed = baseline.copy()
        changed.paste('#ffffff', (540, 730, 740, 770))
        self.assertEqual(harness.changed_bounds(baseline, changed), (540, 730, 740, 770))
        self.assertIsNone(harness.changed_bounds(baseline, baseline.copy()))

    def test_launch_safety_invariants(self):
        source = Path(harness.__file__).read_text()
        self.assertIn("WLR_BACKENDS='headless'", source)
        self.assertIn("WLR_LIBINPUT_NO_DEVICES='1'", source)
        self.assertIn("'--no-hot-reload'", source)
        self.assertIn("'xwayland disable", source)
        for forbidden in ('/dev/input', '/dev/snd', "['cargo'", "['niri'", "['wtype'"):
            self.assertNotIn(forbidden, source)


if __name__ == '__main__':
    unittest.main()
