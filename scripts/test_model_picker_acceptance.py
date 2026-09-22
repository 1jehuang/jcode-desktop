import contextlib
import io
from pathlib import Path
import unittest
from unittest.mock import patch

from PIL import Image, ImageDraw

import model_picker_acceptance as acceptance
import screenshot


def fixture(bounds=(540, 430, 1160, 710)):
    image = Image.new("RGB", (1440, 1000), (12, 12, 12))
    draw = ImageDraw.Draw(image)
    draw.rectangle(bounds, outline=(135, 121, 107))
    draw.rectangle((bounds[0], bounds[3] + 5, bounds[2], bounds[3] + 115), outline=(135, 121, 107))
    return image


def word(text, x=20, y=30, width=50, height=12):
    return dict(text=text, x=x, y=y, width=width, height=height)


class ModelPickerPixelTests(unittest.TestCase):
    def test_dialog_bounds_come_from_visible_border(self):
        self.assertEqual(acceptance.dialog_bounds(fixture()), (540, 430, 1161, 711))

    def test_viewport_edge_popup_border_is_not_excluded(self):
        image = Image.new("RGB", (1440, 1000), (12, 12, 12))
        draw = ImageDraw.Draw(image)
        draw.rectangle((450, 430, 1210, 540), fill=(33, 30, 27), outline=(135, 121, 107))
        draw.rectangle((450, 545, 1210, 991), fill=(48, 43, 39), outline=(135, 121, 107))
        self.assertEqual(acceptance.dialog_bounds(image), (450, 545, 1211, 992))

    def test_flipped_popup_is_not_mistaken_for_editor(self):
        image = Image.new("RGB", (1440, 1000), (12, 12, 12))
        draw = ImageDraw.Draw(image)
        draw.rectangle((450, 430, 1210, 540), fill=(33, 30, 27), outline=(135, 121, 107))
        draw.rectangle((450, 545, 1210, 900), fill=(48, 43, 39), outline=(135, 121, 107))
        self.assertEqual(acceptance.picker_regions(image),
                         ((450, 545, 1211, 901), (450, 430, 1211, 541)))

    def test_wide_composer_does_not_crop_left_hand_menu_text(self):
        self.assertEqual(acceptance.dialog_bounds(fixture((245, 430, 1418, 710))),
                         (245, 430, 1419, 711))

    def test_popup_shadow_can_darken_composer_top_border(self):
        image = fixture()
        ImageDraw.Draw(image).line((540, 715, 1160, 715), fill=(124, 111, 98))
        self.assertEqual(acceptance.dialog_bounds(image), (540, 430, 1161, 711))

    def test_sidebar_and_tab_borders_do_not_move_dialog(self):
        image = fixture()
        draw = ImageDraw.Draw(image)
        draw.rectangle((0, 0, 250, 999), outline=(135, 121, 107))
        draw.rectangle((280, 1, 1428, 60), outline=(135, 121, 107))
        self.assertEqual(acceptance.dialog_bounds(image), (540, 430, 1161, 711))

    def test_absent_or_short_composer_border_is_not_a_dialog(self):
        modal = Image.new("RGB", (1440, 1000))
        ImageDraw.Draw(modal).rectangle((540, 260, 1160, 720), outline=(135, 121, 107))
        for image in (Image.new("RGB", (1440, 1000)), modal):
            with self.subTest(image=image), self.assertRaises(AssertionError):
                acceptance.dialog_bounds(image)

    def test_extra_border_does_not_silently_choose_background(self):
        image = fixture()
        ImageDraw.Draw(image).line((500, 800, 1100, 800), fill=(135, 121, 107))
        with self.assertRaisesRegex(AssertionError, "suggestion and composer borders"):
            acceptance.dialog_bounds(image)

    def test_keyboard_evidence_requires_route_highlight_not_only_visible_text(self):
        image = fixture()
        bounds = acceptance.dialog_bounds(image)
        words = [word("openai:atlas-10", x=600, y=500)]
        with self.assertRaisesRegex(AssertionError, "visibly highlighted"):
            acceptance.selected_row(image, bounds, words, "openai:atlas-10")
        # The old selection color was identical to the menu surface. It must
        # not count as visible selection evidence.
        ImageDraw.Draw(image).rectangle((541, 490, 1159, 525), fill=(48, 43, 39))
        with self.assertRaisesRegex(AssertionError, "visibly highlighted"):
            acceptance.selected_row(image, bounds, words, "openai:atlas-10")
        ImageDraw.Draw(image).rectangle((541, 490, 1159, 525), fill=(228, 221, 211))
        acceptance.selected_row(image, bounds, words, "openai:atlas-10")

    def test_other_highlighted_row_does_not_satisfy_keyboard_selection(self):
        image = fixture()
        ImageDraw.Draw(image).rectangle((541, 450, 1159, 480), fill=(228, 221, 211))
        with self.assertRaisesRegex(AssertionError, "visibly highlighted"):
            acceptance.selected_row(image, acceptance.dialog_bounds(image),
                                    [word("openai:atlas-10", x=600, y=500)], "openai:atlas-10")


class ModelPickerOCRTests(unittest.TestCase):
    def test_inverse_selection_is_normalized_without_changing_capture(self):
        image = Image.new("RGB", (100, 60), (48, 43, 39))
        ImageDraw.Draw(image).rectangle((0, 0, 99, 29), fill=(228, 221, 211))
        normalized = acceptance.normalize_menu_ocr(image)
        self.assertEqual(normalized.getpixel((5, 5)), (27, 34, 44))
        self.assertEqual(normalized.getpixel((5, 40)), (48, 43, 39))
        self.assertEqual(image.getpixel((5, 5)), (228, 221, 211))

    def test_tsv_positions_are_transformed_back_to_screen_pixels(self):
        tsv = "left\ttop\twidth\theight\ttext\n0\t0\t0\t0\t\n30\t60\t120\t36\tSearch\n"
        self.assertEqual(acceptance.parse_words(tsv, (500, 250, 1000, 700)),
                         [word("Search", x=510, y=270, width=40)])

    def test_phrase_matching_accepts_punctuation_and_space_variation(self):
        words = [word("openai:", x=500), word("atlas-10", x=560)]
        self.assertEqual(acceptance.phrase_bounds(words, "openai:atlas-10"),
                         (500, 30, 610, 42))

    def test_exact_route_does_not_accept_wrong_index_or_only_query(self):
        for words in ([word("openai:atlas-01")], [word("atlas-10")]):
            with self.subTest(words=words), self.assertRaises(AssertionError):
                acceptance.phrase_bounds(words, "openai:atlas-10")

    def test_noncontiguous_words_do_not_satisfy_visible_phrase(self):
        with self.assertRaises(AssertionError):
            acceptance.phrase_bounds([word("Search"), word("something"), word("models")],
                                     "Search models")

    def test_empty_ocr_fails_closed(self):
        with self.assertRaisesRegex(AssertionError, "missing"):
            acceptance.phrase_bounds([], "Choose a model")


class ModelPickerIsolationTests(unittest.TestCase):
    def test_helper_refuses_live_or_unconfigured_display_before_native_input(self):
        root = Path("/isolated")
        good = screenshot.isolated_env(root)
        good.update(DISPLAY=":123", JCODE_DESKTOP_SCREENSHOT_MODELS="1")
        cases = ({}, {**good, "JCODE_DESKTOP_SCREENSHOT": "0"},
                 {**good, "JCODE_DESKTOP_SCREENSHOT_MODELS": "0"},
                 {**good, "XDG_RUNTIME_DIR": "/run/user/1000"},
                 {**good, "WAYLAND_DISPLAY": "wayland-live"}, {**good, "DISPLAY": ""})
        for env in cases:
            with self.subTest(env=env), patch.object(acceptance.subprocess, "run") as run, \
                    self.assertRaises(AssertionError):
                acceptance.verify(Path("unused.png"), env, root)
            run.assert_not_called()


class ModelPickerArgumentTests(unittest.TestCase):
    def assert_rejected(self, extra, message="model-interact requires"):
        stderr = io.StringIO()
        with patch("sys.argv", ["screenshot.py", "unused.png", "--model-interact", *extra]), \
                patch.object(screenshot.subprocess, "Popen") as launch, \
                patch.object(screenshot.subprocess, "run") as run, \
                contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as error:
            screenshot.main()
        self.assertEqual(error.exception.code, 2)
        self.assertIn(message, stderr.getvalue())
        launch.assert_not_called()
        run.assert_not_called()

    def test_rejects_incompatible_geometry_or_fixture_before_build(self):
        for extra in (["--panels", "2"], ["--size", "800x600"],
                      ["--theme", "neutral-light"], ["--layout-mode", "normal"],
                      ["--transcript", "image"], ["--learn-stage", "1"],
                      ["--focus-panel", "0"]):
            with self.subTest(extra=extra):
                self.assert_rejected(extra)

    def test_all_other_interaction_modes_are_exclusive(self):
        for mode in ("fresh", "html", "image", "image-cache", "mermaid", "history", "workspace"):
            with self.subTest(mode=mode):
                self.assert_rejected([f"--{mode}-interact"])

    def test_requires_both_ocr_and_native_input_tools(self):
        for missing in ("xdotool", "tesseract"):
            with self.subTest(missing=missing), patch.object(
                    screenshot.shutil, "which", side_effect=lambda tool: None if tool == missing else "/usr/bin/" + tool):
                self.assert_rejected([], "model-interact requires xdotool and tesseract")


if __name__ == "__main__":
    unittest.main()
