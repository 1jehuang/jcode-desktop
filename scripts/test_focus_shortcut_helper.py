"""Test global shortcut routing without using a compositor or sending input."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


HELPER = Path(__file__).with_name('firefox-tab-shortcut.sh')


class FocusShortcutHelperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        # PATH stubs guarantee this test never invokes the real compositor
        # or virtual keyboard. jq alone processes the synthetic JSON.
        for name, script in {
            'niri': '#!/bin/sh\nprintf "%s\\n" "$FOCUSED_WINDOW"\n',
            'wtype': '#!/bin/sh\nprintf "%s\\n" "$@" > "$KEY_LOG"\n',
        }.items():
            path = self.root / name
            path.write_text(script)
            path.chmod(0o755)
        self.log = self.root / 'keys'

    def route(self, window, action):
        self.log.unlink(missing_ok=True)
        result = subprocess.run(['bash', str(HELPER), action], env={
            'PATH': str(self.root) + ':/usr/bin:/bin',
            'FOCUSED_WINDOW': json.dumps(window),
            'KEY_LOG': str(self.log),
        }, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_desktop_receives_both_navigation_aliases(self):
        for action, key in [('previous', 'Page_Up'), ('next', 'Page_Down')]:
            with self.subTest(action=action):
                self.assertEqual(self.route({'app_id': 'jcode-desktop'}, action),
                                 ['-M', 'ctrl', '-k', key, '-m', 'ctrl'])

    def test_all_existing_firefox_actions_are_preserved(self):
        for app in ['firefox', 'org.mozilla.firefox']:
            for action, key in [('previous', 'Page_Up'), ('next', 'Page_Down'),
                                ('new', 't'), ('close', 'w')]:
                with self.subTest(app=app, action=action):
                    self.assertEqual(self.route({'app_id': app}, action),
                                     ['-M', 'ctrl', '-k', key, '-m', 'ctrl'])

    def test_other_apps_and_missing_focus_never_receive_input(self):
        for window in [None, {}, {'app_id': None}, {'app_id': 'kitty'},
                       {'app_id': 'gpui'}, {'app_id': 'firefox-lookalike'}]:
            for action in ['previous', 'next', 'new', 'close']:
                with self.subTest(window=window, action=action):
                    self.assertEqual(self.route(window, action), [])

    def test_desktop_new_and_close_remain_unchanged(self):
        for action in ['new', 'close']:
            self.assertEqual(self.route({'app_id': 'jcode-desktop'}, action), [])

    def test_invalid_action_fails_before_query_or_input(self):
        result = subprocess.run(['bash', str(HELPER), 'invalid'], env={
            'PATH': str(self.root) + ':/usr/bin:/bin',
            'FOCUSED_WINDOW': 'null',
            'KEY_LOG': str(self.log),
        }, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 2)
        self.assertIn('usage:', result.stderr)
        self.assertFalse(self.log.exists())


if __name__ == '__main__':
    unittest.main()
