"""Regression checks for native image-preview acceptance startup."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from PIL import Image

import image_preview_acceptance as acceptance


class BaselineReady(Exception):
    pass


class ImagePreviewAcceptanceTests(unittest.TestCase):
    def test_fullscreen_chart_can_extend_left_of_chat_sidebar(self):
        image = Image.new("RGB", (320, 8))
        image.paste((92, 124, 173), (50, 0, 150, 8))
        self.assertEqual(acceptance.chart_pixels(image), (800, (99, 3)))
        self.assertEqual(acceptance.chart_pixels(image, top_left=True), (800, (50, 0)))

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
