"""Regression tests for the pixel-based secure-input acceptance checks."""
import unittest
from pathlib import Path
from PIL import Image, ImageDraw
from login_acceptance import masked_glyphs, verify, visible_words, phrase_bounds


class LoginAcceptanceTests(unittest.TestCase):
    def test_caret_does_not_expand_footer_button_bounds(self):
        tsv = "text\tleft\ttop\twidth\theight\n|\t900\t1200\t3\t36\nConnect\t60\t2700\t120\t30\naccount\t195\t2700\t120\t30\n"
        words = visible_words(tsv, (280, 60, 1428, 1000))
        self.assertEqual(phrase_bounds(words, "Connect account"), (300, 960, 385, 970))

    def test_mask_requires_separate_dot_sized_glyphs(self):
        image = Image.new("RGB", (160, 30), (30, 30, 30))
        draw = ImageDraw.Draw(image)
        for x in range(8, 88, 8):
            draw.ellipse((x, 12, x + 3, 15), fill="white")
        self.assertEqual(masked_glyphs(image, (0, 0, 160, 30), 10)["dot_count"], 10)

    def test_blank_input_is_not_masking(self):
        with self.assertRaises(AssertionError):
            masked_glyphs(Image.new("RGB", (160, 30), "black"), (0, 0, 160, 30), 10)

    def test_tall_plaintext_glyphs_are_rejected(self):
        image = Image.new("RGB", (160, 30), "black")
        draw = ImageDraw.Draw(image)
        for x in range(8, 88, 8):
            draw.rectangle((x, 6, x + 3, 20), fill="white")
        with self.assertRaises(AssertionError):
            masked_glyphs(image, (0, 0, 160, 30), 10)

    def test_refuses_live_environment_before_any_interaction(self):
        with self.assertRaisesRegex(AssertionError, "Offline fixture"):
            verify(Path("unused.png"), {}, Path("/unused"))


if __name__ == "__main__":
    unittest.main()
