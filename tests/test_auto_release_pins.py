import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("auto_release", ROOT / "scripts/auto-release.py")
AUTO = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUTO)


class RuntimePinTests(unittest.TestCase):
    def test_every_native_workflow_uses_same_immutable_pin(self):
        with patch.object(AUTO, "git", side_effect=lambda _, path: (ROOT / path.split(":", 1)[1]).read_text()) as git:
            AUTO.validate_runtime_pins("HEAD")
        self.assertEqual(git.call_count, 3)
        self.assertTrue(any("freebsd-release.yml" in call.args[1] for call in git.call_args_list))

    def test_freebsd_drift_is_rejected(self):
        def workflow(_, path):
            pin = "b" * 40 if path.endswith("freebsd-release.yml") else "a" * 40
            return f"repository: 1jehuang/jcode\n          ref: {pin}"
        with patch.object(AUTO, "git", side_effect=workflow):
            with self.assertRaisesRegex(ValueError, "same Jcode runtime"):
                AUTO.validate_runtime_pins("HEAD")


if __name__ == "__main__":
    unittest.main()
