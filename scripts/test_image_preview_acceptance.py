"""Regression checks for native inline image acceptance geometry and startup."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from PIL import Image

import image_preview_acceptance as acceptance


class BaselineReady(Exception):
    pass


class ImagePreviewAcceptanceTests(unittest.TestCase):
    def test_chart_detection_is_independent_of_image_position(self):
        image = Image.new("RGB", (320, 8))
        image.paste((92, 124, 173), (50, 0, 150, 8))
        self.assertEqual(acceptance.chart_pixels(image), (800, (99, 3)))
        self.assertEqual(acceptance.chart_pixels(image, top_left=True), (800, (50, 0)))

    def canvas(self, bounds=(300, 100, 967, 500)):
        image = Image.new("RGB", (1400, 900), (30, 30, 30))
        image.paste((244, 244, 240), bounds)
        return image

    def test_canvas_bounds_exclude_metadata_footer(self):
        image = self.canvas()
        image.paste((140, 140, 140), (300, 504, 967, 528))
        self.assertEqual(acceptance.image_bounds(image), (300, 100, 967, 500))
        self.assertEqual(acceptance.image_bounds(image)[3] - acceptance.image_bounds(image)[1], 400)

    def test_missing_canvas_fails(self):
        with self.assertRaisesRegex(AssertionError, "canvas did not paint"):
            acceptance.image_bounds(Image.new("RGB", (20, 20)))

    def test_inline_expansion_accepts_less_than_double_area(self):
        acceptance.assert_inline_expanded(self.canvas(), self.canvas((300, 100, 1100, 580)))

    def test_inline_expansion_supports_narrow_sidebar(self):
        acceptance.assert_inline_expanded(
            self.canvas((248, 127, 915, 527)), self.canvas((248, 120, 1415, 820)))

    def test_inline_expansion_rejects_unchanged_size(self):
        with self.assertRaises(AssertionError):
            acceptance.assert_inline_expanded(self.canvas(), self.canvas())

    def test_inline_expansion_rejects_fullscreen_image(self):
        with self.assertRaisesRegex(AssertionError, "left the transcript"):
            acceptance.assert_inline_expanded(self.canvas(), self.canvas((0, 50, 1300, 830)))

    def test_inline_expansion_preserves_sidebar(self):
        expanded = self.canvas((300, 100, 1100, 580))
        expanded.paste((0, 0, 0), (0, 0, 250, 900))
        with self.assertRaisesRegex(AssertionError, "opened an overlay"):
            acceptance.assert_inline_expanded(self.canvas(), expanded)

    def test_black_first_frame_waits_for_chart_before_clicking(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "baseline.png"
            Image.new("RGB", (320, 8)).save(output)
            commands = []

            def run(command, **kwargs):
                commands.append(command)
                if command[0] == "import":
                    image = Image.new("RGB", (320, 8))
                    image.paste((92, 124, 173), (300, 0, 320, 8))
                    image.save(output)
                else:
                    raise BaselineReady

            with patch.object(acceptance.subprocess, "run", side_effect=run), \
                    patch.object(acceptance.time, "sleep"):
                with self.assertRaises(BaselineReady):
                    acceptance.verify(output, {}, root)
            self.assertEqual(commands[0][0], "import")
            self.assertEqual(commands[1][:4], ["xdotool", "mousemove", "309", "3"])
            self.assertEqual(acceptance.chart_pixels(Image.open(output))[0], 160)

    def test_missing_chart_still_fails_after_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "baseline.png"
            Image.new("RGB", (320, 8)).save(output)
            with patch.object(acceptance.time, "monotonic", side_effect=[0, 31]), \
                    patch.object(acceptance.subprocess, "run") as run:
                with self.assertRaisesRegex(AssertionError, "blue bar did not paint"):
                    acceptance.verify(output, {}, Path(directory))
                run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
