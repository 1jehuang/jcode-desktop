"""Keep the public Super-key behavior map and acceptance CLI honest."""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ShortcutBehaviorCoverage(unittest.TestCase):
    def test_every_registered_super_binding_has_a_behavior_row(self):
        source = (ROOT / 'crates/jcode-desktop-ui/src/lib.rs').read_text()
        registered = re.findall(r'KeyBinding::new\("(super-[^"]+)",\s*(\w+),', source)
        document = (ROOT / 'docs/super-key-behavior-matrix.md').read_text()
        rows = re.findall(r'^\| `(super-[^`]+)` \| `([^`]+)` \| ([^|]+) \| ([^|]+) \|$',
                          document, re.MULTILINE)
        self.assertEqual(sorted(registered), sorted((row[0], row[1]) for row in rows))
        self.assertTrue(rows)
        for chord, action, check, outcome in rows:
            self.assertTrue(action and check.strip() and outcome.strip(), chord)
            self.assertNotEqual(outcome.strip().lower(), 'passed', chord)

    def test_attachment_guard_requires_every_expected_live_session(self):
        from shortcut_behavior_acceptance import sessions_attached

        def state(ids, loaded):
            return {'rows': [{'panels': [
                {'session': sid, 'history_loaded': ready}
                for sid, ready in zip(ids, loaded)
            ]}]}

        expected = ['session_a', 'session_b']
        self.assertTrue(sessions_attached(state(expected, [True, True]), expected))
        self.assertFalse(sessions_attached(None, expected))
        self.assertFalse(sessions_attached(state(expected, [False, True]), expected))
        self.assertFalse(sessions_attached(state(expected, [True, False]), expected))
        self.assertFalse(sessions_attached(state(expected[:1], [True]), expected))
        self.assertFalse(sessions_attached(state(expected[::-1], [True, True]), expected))

    def test_bad_daemon_path_fails_before_creating_artifacts(self):
        scratch = Path(os.environ.get('JCODE_SCRATCH_DIR', ROOT / 'target'))
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='shortcut-cli-', dir=scratch) as temp:
            root = Path(temp)
            for bad in [root / 'missing', root]:
                output = root / 'evidence'
                result = subprocess.run(
                    [sys.executable, str(ROOT / 'scripts/accept-navigation.py'),
                     str(output), '--jcode-binary', str(bad)],
                    capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn('--jcode-binary must name an executable file', result.stderr)
                self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
