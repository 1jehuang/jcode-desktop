"""Negative controls for the real-rendered workspace tab acceptance checks."""
import importlib.util
from pathlib import Path
import unittest

from PIL import Image, ImageDraw

spec = importlib.util.spec_from_file_location(
    "accept_live_tabs", Path(__file__).with_name("accept-live-tabs.py"))
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


class GroupedTabMeasurementTests(unittest.TestCase):
    def fixture(self):
        image = Image.new("RGB", (1440, 1000), (30, 30, 30))
        draw = ImageDraw.Draw(image)
        groups = [(500, 587), (604, 995), (1012, 1099)]
        targets = []
        for row, (left, right) in enumerate(groups):
            draw.rectangle((left, 20, right, 47), fill=(50, 50, 50))
            for position in range(2):
                x = left + (right - left) * (position + .5) / 2
                targets.append((row * 2 + position, x - accept.CANVAS_LEFT))
        x = round(targets[2][1] + accept.CANVAS_LEFT)
        draw.line((x - 10, 16, x + 10, 16), fill=(160, 150, 140))
        draw.rectangle((x - 10, 17, x + 10, 19), fill=(50, 50, 50))
        navigation = {"active_row": 1, "focused_slot": 2, "tab_targets": targets,
                      "rows": [{"row": row, "panels": [{"slot": row * 2}, {"slot": row * 2 + 1}]}
                               for row in range(3)]}
        return image, navigation

    def test_accepts_separated_compact_neighbors_and_thin_outline(self):
        image, nav = self.fixture()
        metrics = accept.measure_grouped_tabs(image, nav)
        self.assertEqual(metrics["gaps"], [16, 16])
        self.assertEqual(metrics["focused_outline_px"], 1)

    def test_rejects_heavy_top_stripe(self):
        image, nav = self.fixture()
        x = round(dict(nav["tab_targets"])[2] + accept.CANVAS_LEFT)
        ImageDraw.Draw(image).rectangle((x - 10, 16, x + 10, 19), fill=(160, 150, 140))
        with self.assertRaisesRegex(AssertionError, "heavy tab top stripe"):
            accept.measure_grouped_tabs(image, nav)

    def test_rejects_ungrouped_tabs(self):
        image, nav = self.fixture()
        ImageDraw.Draw(image).rectangle((500, 27, 1099, 27), fill=(50, 50, 50))
        with self.assertRaisesRegex(AssertionError, "three separated workspace groups"):
            accept.measure_grouped_tabs(image, nav)

    def test_rejects_workspaces_on_wrong_sides(self):
        image, nav = self.fixture()
        nav["rows"][0]["panels"], nav["rows"][2]["panels"] = (
            nav["rows"][2]["panels"], nav["rows"][0]["panels"])
        with self.assertRaisesRegex(AssertionError, "numerical order"):
            accept.measure_grouped_tabs(image, nav)

    def test_rejects_neighbor_dominating_active_workspace(self):
        image, nav = self.fixture()
        nav["active_row"] = 0
        with self.assertRaisesRegex(AssertionError, "current workspace must dominate"):
            accept.measure_grouped_tabs(image, nav)


if __name__ == "__main__":
    unittest.main()
