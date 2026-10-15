import contextlib
import io
from pathlib import Path
import unittest
from unittest.mock import patch

from PIL import Image, ImageDraw

import screenshot
import mermaid_preview_acceptance as acceptance
from mermaid_preview_acceptance import diagram_pixels, restored


def fixture(bounds=(300, 200, 700, 300)):
    image = Image.new("RGB", (1000, 700), (37, 34, 31))
    ImageDraw.Draw(image).rectangle(bounds, fill=(48, 43, 39))
    return image


class MermaidPixelsTests(unittest.TestCase):
    def test_top_diagram_is_not_confused_with_stop_notice(self):
        image = Image.new("RGB", (1440, 1000), (37, 34, 31))
        draw = ImageDraw.Draw(image)
        for left, right in ((248, 342), (378, 480), (591, 685), (744, 838)):
            draw.rectangle((left, 139, right, 189), fill=(48, 43, 39))
        draw.rounded_rectangle((236, 322, 1427, 370), radius=16, fill=(48, 43, 39))
        actual = diagram_pixels(image)
        self.assertEqual(actual.bounds, (248, 139, 839, 190))
        self.assertEqual(actual.point, (543, 164))

    def test_warm_transcript_gutter_does_not_shift_native_click(self):
        image = Image.new("RGB", (1440, 1000), (48, 43, 39))
        draw = ImageDraw.Draw(image)
        draw.rectangle((276, 70, 1426, 896), fill=(37, 34, 31))
        draw.rectangle((302, 206, 889, 256), fill=(48, 43, 39))
        actual = diagram_pixels(image)
        self.assertEqual(actual.bounds, (302, 206, 890, 257))
        self.assertEqual(actual.point, (596, 231))

    def test_detects_native_node_palette_and_center(self):
        actual = diagram_pixels(fixture())
        self.assertEqual(actual.bounds, (300, 200, 701, 301))
        self.assertEqual(actual.point, (500, 250))
        self.assertEqual(actual.count, 401 * 101)

    def test_sidebar_matching_colors_do_not_change_diagram(self):
        image = fixture()
        ImageDraw.Draw(image).rectangle((0, 0, 250, 699), fill=(48, 43, 39))
        self.assertTrue(restored(diagram_pixels(fixture()), diagram_pixels(image)))

    def test_missing_diagram_fails(self):
        with self.assertRaisesRegex(AssertionError, "did not paint"):
            diagram_pixels(Image.new("RGB", (1000, 700), (37, 34, 31)))

    def test_restoration_rejects_shifted_same_area_diagram(self):
        initial = diagram_pixels(fixture())
        self.assertTrue(restored(initial, initial))
        self.assertFalse(restored(initial, diagram_pixels(fixture((310, 210, 710, 310)))))

    def test_header_and_composer_matching_colors_are_excluded(self):
        image = fixture()
        draw = ImageDraw.Draw(image)
        draw.rectangle((280, 0, 975, 105), fill=(48, 43, 39))
        draw.rectangle((280, 590, 975, 699), fill=(48, 43, 39))
        self.assertTrue(restored(diagram_pixels(fixture()), diagram_pixels(image)))


class MermaidReadinessTests(unittest.TestCase):
    def test_blank_initial_frame_is_recaptured_before_zooming(self):
        env = {"DISPLAY": ":private"}
        root = Path("isolated-root")
        with patch.object(acceptance.Image, "open", side_effect=[
                Image.new("RGB", (1000, 700)), fixture()]), \
                patch.object(Path, "exists", return_value=False), \
                patch.object(acceptance.time, "sleep"), \
                patch.object(acceptance.subprocess, "run", side_effect=[
                    None, None, RuntimeError("stop at first native zoom")]) as native:
            with self.assertRaisesRegex(RuntimeError, "stop at first native zoom"):
                acceptance.verify(Path("initial.png"), env, root)
        self.assertEqual(native.call_count, 3)
        self.assertEqual(native.call_args_list[0].args[0][0], "import")
        self.assertEqual(native.call_args_list[1].args[0][0], "import")
        self.assertEqual(native.call_args_list[2].args[0][:4],
                         ["xdotool", "mousemove", "500", "250"])
        self.assertIn("ctrl", native.call_args_list[2].args[0])
        for call in native.call_args_list:
            self.assertIs(call.kwargs["env"], env)
            self.assertEqual(call.kwargs["cwd"], root)

    def test_missing_initial_diagram_times_out_without_clicks(self):
        with patch.object(Path, "exists", return_value=False), \
                patch.object(acceptance.time, "monotonic", side_effect=[0, 16]), \
                patch.object(acceptance.subprocess, "run") as native:
            with self.assertRaisesRegex(AssertionError, "Mermaid inline -settled failed"):
                acceptance.verify(Path("initial.png"), {}, Path("isolated-root"))
        native.assert_not_called()


class MermaidArgumentsTests(unittest.TestCase):
    def assert_rejected(self, arguments, message):
        stderr = io.StringIO()
        with patch("sys.argv", ["screenshot.py", "unused.png", "--no-build", *arguments]), \
                patch.object(screenshot.subprocess, "Popen") as launch, \
                patch.object(screenshot.subprocess, "run") as run, \
                contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as error:
            screenshot.main()
        self.assertEqual(error.exception.code, 2)
        self.assertIn(message, stderr.getvalue())
        launch.assert_not_called()
        run.assert_not_called()

    def test_requires_mermaid_fixture(self):
        for transcript in ("all", "empty", "html", "image", "streaming"):
            with self.subTest(transcript=transcript):
                self.assert_rejected(["--mermaid-interact", "--transcript", transcript],
                                     "requires the mermaid transcript")

    def test_requires_one_panel_and_no_conflicting_modes(self):
        conflicts = (["--panels", "2"], ["--fresh-interact"], ["--html-interact"],
                     ["--image-interact"], ["--history-interact"],
                     ["--learn-stage", "1"], ["--focus-panel", "0"])
        for conflict in conflicts:
            with self.subTest(conflict=conflict):
                self.assert_rejected(["--mermaid-interact", "--transcript", "mermaid", *conflict],
                                     "one panel, and no other interaction mode")

    def test_requires_native_click_tool_before_launch(self):
        with patch.object(screenshot.shutil, "which", return_value=None):
            self.assert_rejected(["--mermaid-interact", "--transcript", "mermaid"],
                                 "mermaid-interact requires xdotool")


if __name__ == "__main__":
    unittest.main()
