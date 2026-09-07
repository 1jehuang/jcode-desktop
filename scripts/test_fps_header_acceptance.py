import unittest
from PIL import Image, ImageDraw
from fps_header_acceptance import header_pixels


class HeaderPixelsTests(unittest.TestCase):
    def image(self):
        return Image.new("RGB", (400, 100), (48, 43, 39))

    def test_centered_readout(self):
        image = self.image()
        ImageDraw.Draw(image).rectangle((185, 5, 215, 14), fill=(170, 160, 150))
        self.assertGreater(header_pixels(image, 20)["text_pixels"], 0)

    def test_missing_readout_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, "missing"):
            header_pixels(self.image(), 20)

    def test_floating_badge_off_center_is_rejected(self):
        image = self.image()
        ImageDraw.Draw(image).rectangle((20, 5, 60, 14), fill=(170, 160, 150))
        with self.assertRaisesRegex(AssertionError, "not centered"):
            header_pixels(image, 20)

    def test_clipped_readout_is_rejected(self):
        image = self.image()
        ImageDraw.Draw(image).rectangle((185, 0, 215, 8), fill=(170, 160, 150))
        with self.assertRaisesRegex(AssertionError, "clipped"):
            header_pixels(image, 20)


if __name__ == "__main__":
    unittest.main()
