"""Active macOS release gates must not use the retired macOS 14 image."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
ACTIVE_WORKFLOWS = (
    "macos-beta.yml",
    "macos-tests.yml",
    "macos-update-matrix.yml",
    "macos-public-release-verify.yml",
    "macos-notary-log.yml",
    "macos-installed-smoke.yml",
)


class MacosRunnerTests(unittest.TestCase):
    def test_active_release_and_acceptance_jobs_use_supported_pinned_runner(self):
        for workflow in ACTIVE_WORKFLOWS:
            with self.subTest(workflow=workflow):
                source = (ROOT / ".github/workflows" / workflow).read_text()
                self.assertIn("runs-on: macos-15", source)
                self.assertNotIn("runs-on: macos-14", source)


if __name__ == "__main__":
    unittest.main()
