"""Guard the canonical Jcode donut without requiring a sibling checkout."""
from pathlib import Path
import shutil
import struct
import subprocess
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SVG = ROOT / "assets/icons/jcode.svg"
PNG = ROOT / "assets/app-icon/icon-1024.png"
NS = {"svg": "http://www.w3.org/2000/svg"}


class BrandAssetsTests(unittest.TestCase):
    def test_svg_is_canonical_circle_geometry_not_old_trace(self):
        root = ET.parse(SVG).getroot()
        self.assertEqual(root.attrib["viewBox"], "0 0 512.00 512.00")
        self.assertEqual(len(root.findall(".//svg:circle", NS)), 3344)
        self.assertEqual(root.findall(".//svg:path", NS), [])
        self.assertEqual(root.findall(".//svg:rect", NS), [])

    def test_packaging_png_dimensions(self):
        data = PNG.read_bytes()
        self.assertEqual(data[:8], b"\x89PNG\r\n\x1a\n")
        self.assertEqual(struct.unpack(">II", data[16:24]), (1024, 1024))

    @unittest.skipUnless(shutil.which("rsvg-convert"), "rsvg-convert is not installed")
    def test_packaging_png_is_rendered_from_current_svg(self):
        generated = subprocess.check_output([
            "rsvg-convert", "-w", "1024", "-h", "1024", "-b", "none", str(SVG)
        ])
        self.assertEqual(generated, PNG.read_bytes())


if __name__ == "__main__":
    unittest.main()
