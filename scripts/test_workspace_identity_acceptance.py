"""Negative controls ensure the rendered-pixel verifier cannot pass a uniform UI."""
import unittest
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw
from workspace_identity_acceptance import blend, measure, verify


class WorkspaceIdentityMeasurementTests(unittest.TestCase):
    def test_existing_evidence_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = root / "review-selected-1.png"
            evidence.write_bytes(b"previous evidence")
            with self.assertRaises(FileExistsError):
                verify(root / "review.png", {}, root, theme="warm-neutral")
            self.assertEqual(evidence.read_bytes(), b"previous evidence")

    def fixture(self):
        background = (48, 43, 39)
        accents = [(138, 180, 248), (128, 203, 196), (232, 184, 109), (196, 161, 237)]
        image = Image.new("RGB", (1440, 1000), background)
        draw = ImageDraw.Draw(image)
        targets = {row: 210 + row * 184 for row in range(4)}
        for row, accent in enumerate(accents):
            x = targets[row] + 276
            rail = accent if row == 3 else blend(blend(background, accent, .05), accent, .65)
            draw.rectangle((x - 25, 17, x + 24, 24), fill=rail)
        draw.rectangle((1288, 83, 1305, 96), fill=accents[3])
        return image, targets, accents

    def test_recognizes_four_distinct_identities_and_one_selected_badge(self):
        image, targets, _ = self.fixture()
        result = measure(image, targets, "warm-neutral")
        self.assertEqual(result["identified_workspaces"], [0, 1, 2, 3])
        self.assertEqual(result["visually_selected_workspace"], 3)

    def test_rejects_former_uniform_workspace_styling(self):
        image, targets, _ = self.fixture()
        ImageDraw.Draw(image).rectangle((300, 16, 1200, 24), fill=(48, 43, 39))
        with self.assertRaisesRegex(AssertionError, "recognizable workspace identity"):
            measure(image, targets, "warm-neutral")

    def test_rejects_two_selected_workspaces(self):
        image, targets, accents = self.fixture()
        ImageDraw.Draw(image).rectangle((1288, 16, 1305, 29), fill=accents[0])
        with self.assertRaisesRegex(AssertionError, "visually ambiguous"):
            measure(image, targets, "warm-neutral")

    def test_rejects_no_selected_workspace(self):
        image, targets, _ = self.fixture()
        ImageDraw.Draw(image).rectangle((1288, 83, 1305, 96), fill=(48, 43, 39))
        with self.assertRaisesRegex(AssertionError, "visually ambiguous"):
            measure(image, targets, "warm-neutral")


if __name__ == "__main__":
    unittest.main()
