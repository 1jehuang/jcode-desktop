"""Regression tests for the native navigation acceptance checker."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('accept_navigation', Path(__file__).with_name('accept-navigation.py'))
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


class NavigationCheckerTests(unittest.TestCase):
    def setUp(self):
        self.state = {
            'version': 1, 'active_row': 0, 'focused_slot': 1, 'keyboard_panel': 1,
            'rows': [
                {'remembered': 1, 'panels': [
                    {'slot': 0, 'session': 'a', 'focused': False},
                    {'slot': 1, 'session': 'b', 'focused': True},
                    {'slot': 2, 'session': 'c', 'focused': False},
                ]},
                {'remembered': None, 'panels': []},
            ],
        }

    def test_accepts_matching_state(self):
        accept.assert_state(self.state, 0, 1, ['a', 'b', 'c'])

    def test_rejects_skipped_panel_and_wrong_row(self):
        for row, position in [(0, 0), (0, 2), (1, None)]:
            with self.subTest(row=row, position=position), self.assertRaises(AssertionError):
                accept.assert_state(self.state, row, position)

    def test_rejects_divergent_keyboard_map_memory_or_order(self):
        mutations = [
            lambda s: s.update(keyboard_panel=0),
            lambda s: s['rows'][0]['panels'][0].update(focused=True),
            lambda s: s['rows'][0].update(remembered=0),
            lambda s: s['rows'][0]['panels'][0].update(session='c'),
        ]
        for mutate in mutations:
            state = copy.deepcopy(self.state)
            mutate(state)
            with self.assertRaises(AssertionError):
                accept.assert_state(state, 0, 1, ['a', 'b', 'c'])

    def test_empty_row_rejects_stale_map_focus(self):
        self.state.update(active_row=1, focused_slot=None, keyboard_panel=None)
        with self.assertRaises(AssertionError):
            accept.assert_state(self.state, 1, None, [])
        self.state['rows'][0]['panels'][1]['focused'] = False
        accept.assert_state(self.state, 1, None, [])

    def test_reader_tolerates_missing_and_partial_writes(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'state'
            for text in [None, '', 'strip=0 focus=1', 'navigation={']:
                if text is not None:
                    path.write_text(text)
                self.assertIsNone(accept.navigation_state(path))
            path.write_text('strip=0 focus=1\nnavigation={"version":1}\n')
            self.assertEqual(accept.navigation_state(path), {'version': 1})


if __name__ == '__main__':
    unittest.main()
