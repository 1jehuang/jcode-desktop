import importlib.util
from pathlib import Path
import re
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("auto_release", ROOT / "scripts/auto-release.py")
AUTO = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUTO)


class RuntimePinTests(unittest.TestCase):
    def runtime_pin(self, workflow):
        text = (ROOT / '.github/workflows' / workflow).read_text()
        match = re.search(r'repository: 1jehuang/jcode\s+ref: ([0-9a-f]{40})\b', text)
        self.assertIsNotNone(match, workflow)
        return match[1]

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

    def test_native_preflight_checks_the_exact_checkout_before_building(self):
        for workflow in ('cross-platform-release.yml', 'macos-beta.yml', 'freebsd-release.yml'):
            with self.subTest(workflow=workflow):
                text = (ROOT / '.github/workflows' / workflow).read_text()
                pin = self.runtime_pin(workflow)
                command = f'verify-runtime-source.py --runtime jcode --expected-commit {pin}'
                self.assertIn(command, text)
                self.assertLess(text.index(command), text.index('dtolnay/rust-toolchain')
                                if 'dtolnay/rust-toolchain' in text else text.index('vmactions/freebsd-vm'))

    def test_update_matrix_and_testing_defaults_match_native_runtime(self):
        pin = self.runtime_pin('cross-platform-release.yml')
        text = (ROOT / '.github/workflows/update-matrix.yml').read_text()
        pins = re.findall(r'repository: 1jehuang/jcode\s+ref: ([0-9a-f]{40})\b', text)
        self.assertEqual(pins, [pin, pin])
        macos = (ROOT / '.github/workflows/macos-tests.yml').read_text()
        self.assertIn(f'default: {pin}', macos)
        self.assertIn("inputs.jcode_ref || '" + pin + "'", macos)
        hosted = (ROOT / '.github/workflows/linux-hosted-validation.yml').read_text()
        self.assertIn(f'default: {pin}', hosted)
        self.assertIn('verify-runtime-source.py --runtime jcode --expected-commit "$JCODE_REVISION"', hosted)

    def test_runtime_preflight_is_a_fingerprinted_release_input(self):
        self.assertTrue(AUTO.release_input('scripts/verify-runtime-source.py'))
        self.assertTrue(AUTO.release_input('scripts/windows-runtime-acceptance.py'))
        self.assertTrue(AUTO.release_input('scripts/windows_screenshots.py'))


if __name__ == "__main__":
    unittest.main()
