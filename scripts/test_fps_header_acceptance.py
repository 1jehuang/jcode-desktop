import unittest
from PIL import Image, ImageDraw
from fps_header_acceptance import header_pixels


class HeaderPixelsTests(unittest.TestCase):
    def image(self, fps=True):
        image = Image.new("RGB", (1200, 100), (48, 43, 39))
        draw = ImageDraw.Draw(image)
        if fps:
            draw.rectangle((409, 14, 450, 24), fill=(170, 160, 150))
        draw.rectangle((1133, 14, 1143, 24), fill=(170, 160, 150))
        return image

    def test_compact_readout_after_version(self):
        self.assertGreater(header_pixels(self.image(), 0, False)["text_pixels"], 0)

    def test_missing_readout_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, "missing"):
            header_pixels(self.image(fps=False), 0, False)

    def test_separate_header_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, "separate header"):
            header_pixels(self.image(), 20, False)

    def test_clipped_readout_is_rejected(self):
        image = self.image()
        ImageDraw.Draw(image).rectangle((409, 6, 450, 14), fill=(170, 160, 150))
        with self.assertRaisesRegex(AssertionError, "clipped"):
            header_pixels(image, 0, False)

    def test_missing_plus_is_rejected(self):
        image = self.image()
        ImageDraw.Draw(image).rectangle((1120, 6, 1148, 34), fill=(48, 43, 39))
        with self.assertRaisesRegex(AssertionError, "plus is missing"):
            header_pixels(image, 0, False)


if __name__ == "__main__":
    unittest.main()
