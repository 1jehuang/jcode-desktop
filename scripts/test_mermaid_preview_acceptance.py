import contextlib
import io
from pathlib import Path
import unittest
from unittest.mock import patch

from PIL import Image, ImageDraw

import screenshot
import mermaid_preview_acceptance as acceptance
from mermaid_preview_acceptance import (
    close_button_point, diagram_pixels, enlarged, restored, zoom_button_points,
)


def fixture(bounds=(300, 200, 700, 300)):
    image = Image.new("RGB", (1000, 700), (37, 34, 31))
    draw = ImageDraw.Draw(image)
    draw.rectangle(bounds, fill=(51, 51, 51))
    x, y, right, bottom = bounds
    draw.rectangle((x + 10, y + 10, right - 10, bottom - 10), fill=(31, 32, 32))
    return image


class MermaidPixelsTests(unittest.TestCase):
    def test_warm_transcript_gutter_does_not_shift_native_click(self):
        image = Image.new("RGB", (1440, 1000), (48, 43, 39))
        draw = ImageDraw.Draw(image)
        draw.rectangle((276, 70, 1426, 896), fill=(37, 34, 31))
        draw.rectangle((302, 206, 889, 256), fill=(48, 43, 39))
        actual = diagram_pixels(image)
        self.assertEqual(actual.bounds, (302, 206, 890, 257))
        self.assertEqual(actual.point, (596, 231))

    def test_detects_actual_renderer_palette_and_center(self):
        actual = diagram_pixels(fixture())
        self.assertEqual(actual.bounds, (300, 200, 701, 301))
        self.assertEqual(actual.point, (500, 250))
        self.assertEqual(actual.count, 401 * 101)

    def test_sidebar_matching_colors_do_not_change_diagram(self):
        image = fixture()
        ImageDraw.Draw(image).rectangle((0, 0, 250, 699), fill=(51, 51, 51))
        self.assertTrue(restored(diagram_pixels(fixture()), diagram_pixels(image)))

    def test_missing_and_monochrome_diagrams_fail(self):
        for color in ((37, 34, 31), (51, 51, 51), (31, 32, 32)):
            with self.subTest(color=color), self.assertRaisesRegex(AssertionError, "did not paint"):
                diagram_pixels(Image.new("RGB", (1000, 700), color))

    def test_enlargement_requires_both_pixel_area_and_height_growth(self):
        initial = diagram_pixels(fixture())
        self.assertTrue(enlarged(initial, diagram_pixels(fixture((300, 200, 900, 500)))))
        self.assertFalse(enlarged(initial, initial))
        self.assertFalse(enlarged(initial, diagram_pixels(fixture((300, 200, 950, 300)))))

    def test_restoration_rejects_shifted_same_area_diagram(self):
        initial = diagram_pixels(fixture())
        self.assertTrue(restored(initial, initial))
        self.assertFalse(restored(initial, diagram_pixels(fixture((310, 210, 710, 310)))))

    def test_zoom_accepts_clipped_area_but_requires_taller_changed_nodes(self):
        fitted = diagram_pixels(fixture((300, 200, 950, 300)))
        clipped = diagram_pixels(fixture((500, 200, 950, 350)))
        self.assertFalse(enlarged(fitted, clipped))
        self.assertTrue(acceptance.zoom_changed(fitted, fitted, clipped))
        self.assertFalse(acceptance.zoom_changed(fitted, clipped, clipped))
        self.assertFalse(acceptance.zoom_changed(fitted, fitted, fitted))

    def test_close_button_comes_from_new_top_right_ink(self):
        before = fixture()
        # Unchanged text near the top must not move the discovered click point.
        ImageDraw.Draw(before).text((910, 10), "old tab", fill=(180, 180, 180))
        preview = before.copy()
        ImageDraw.Draw(preview).text((910, 65), "Close x", fill=(180, 180, 180))
        x, y = close_button_point(before, preview, diagram_pixels(preview))
        self.assertTrue(910 <= x <= 955)
        self.assertTrue(65 <= y <= 78)

    def test_missing_close_button_fails(self):
        image = fixture()
        with self.assertRaisesRegex(AssertionError, "Close button did not paint"):
            close_button_point(image, image, diagram_pixels(image))

    def test_close_locator_excludes_adjacent_escape_hint(self):
        before = fixture()
        preview = before.copy()
        draw = ImageDraw.Draw(preview)
        draw.rectangle((811, 25, 895, 34), fill=(180, 180, 180))
        draw.rectangle((922, 25, 969, 34), fill=(180, 180, 180))
        self.assertEqual(close_button_point(before, preview, diagram_pixels(preview)),
                         (945, 29))

    def test_zoom_buttons_are_located_by_header_text_groups(self):
        image = fixture()
        draw = ImageDraw.Draw(image)
        for x, text in ((300, "Mermaid diagram"), (630, "-"), (665, "Fit"),
                        (710, "+"), (748, "100%"), (810, "Esc to close"), (910, "Close x")):
            draw.text((x, 65), text, fill=(180, 180, 180))
        zoom, fit = zoom_button_points(image, (928, 72))
        self.assertTrue(710 <= zoom[0] <= 720)
        self.assertTrue(665 <= fit[0] <= 685)
        self.assertEqual(zoom[1], 72)

    def test_missing_zoom_controls_fail(self):
        with self.assertRaisesRegex(AssertionError, "zoom controls did not paint"):
            zoom_button_points(fixture(), (928, 72))

    def test_percentage_and_escape_hint_separate_at_actual_14px_gap(self):
        image = fixture()
        draw = ImageDraw.Draw(image)
        # Actual screenshot boundaries, offset left to fit the synthetic image.
        for left, right in ((683, 687), (719, 738), (768, 774),
                            (797, 824), (838, 922), (949, 979)):
            draw.rectangle((left, 72, right, 80), fill=(180, 180, 180))
        zoom, fit = zoom_button_points(image, (964, 77))
        self.assertEqual(zoom, (771, 77))
        self.assertEqual(fit, (728, 77))


class MermaidReadinessTests(unittest.TestCase):
    def test_blank_initial_frame_is_recaptured_before_clicking(self):
        env = {"DISPLAY": ":private"}
        root = Path("isolated-root")
        with patch.object(acceptance.Image, "open", side_effect=[
                Image.new("RGB", (1000, 700)), fixture()]), \
                patch.object(acceptance.time, "sleep"), \
                patch.object(acceptance.subprocess, "run", side_effect=[
                    None, RuntimeError("stop at first native click")]) as native:
            with self.assertRaisesRegex(RuntimeError, "stop at first native click"):
                acceptance.verify(Path("initial.png"), env, root)
        self.assertEqual(native.call_count, 2)
        self.assertEqual(native.call_args_list[0].args[0][0], "import")
        self.assertEqual(native.call_args_list[1].args[0][:4],
                         ["xdotool", "mousemove", "500", "250"])
        for call in native.call_args_list:
            self.assertIs(call.kwargs["env"], env)
            self.assertEqual(call.kwargs["cwd"], root)

    def test_missing_initial_diagram_times_out_without_clicks(self):
        with patch.object(acceptance.Image, "open", return_value=Image.new("RGB", (1000, 700))), \
                patch.object(acceptance.time, "monotonic", side_effect=[0, 16]), \
                patch.object(acceptance.subprocess, "run") as native:
            with self.assertRaisesRegex(AssertionError, "Initial Mermaid diagram did not paint"):
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
