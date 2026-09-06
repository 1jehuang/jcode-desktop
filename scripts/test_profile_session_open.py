import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('profile_session_open', Path(__file__).with_name('profile-session-open.py'))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class SessionOpenProfileTests(unittest.TestCase):
    def test_seed_is_persisted_history_not_render_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixtures = profile.seed_sessions(root, 3, 20)
            self.assertEqual(len({f['session_id'] for f in fixtures}), 3)
            for fixture in fixtures:
                data = json.loads((root / 'jcode/sessions' / (fixture['session_id'] + '.json')).read_text())
                self.assertEqual(data['id'], fixture['session_id'])
                self.assertEqual(data['title'], fixture['title'])
                self.assertEqual(data['working_dir'], str(root))
                self.assertEqual(data['status'], 'Closed')
                self.assertEqual(len(data['messages']), 20)
                self.assertEqual(data['messages'][-1]['content'][0]['text'], profile.MARKER)
                self.assertEqual([m['role'] for m in data['messages']], ['user', 'assistant'] * 10)

    def test_ocr_lines_group_words_without_merging_blocks(self):
        header = 'level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n'
        tsv = header + '5\t1\t1\t1\t1\t1\t37\t144\t40\t12\t95\tHISTORY\n'
        tsv += '5\t1\t1\t1\t1\t2\t90\t145\t30\t11\t95\tALPHA\n'
        tsv += '5\t1\t2\t1\t1\t1\t400\t144\t40\t12\t95\tother\n'
        self.assertEqual(profile.ocr_lines(tsv), [
            dict(text='HISTORY ALPHA', x=37, y=144, height=12),
            dict(text='other', x=400, y=144, height=12)])

    def test_loaded_flag_alone_cannot_pass_empty_history(self):
        self.assertFalse(profile.history_ready({}, 20))
        self.assertFalse(profile.history_ready({'history_loaded': True, 'history_items': 0}, 20))
        self.assertFalse(profile.history_ready({'history_loaded': False, 'history_items': 20}, 20))
        self.assertTrue(profile.history_ready({'history_loaded': True, 'history_items': 20}, 20))

    def test_isolation_does_not_inherit_live_display_or_credentials(self):
        env = profile.isolated_env(Path('/isolated'))
        self.assertNotIn('DISPLAY', env)
        self.assertNotIn('WAYLAND_DISPLAY', env)
        self.assertNotIn('ANTHROPIC_API_KEY', env)
        self.assertEqual(env['JCODE_HOME'], '/isolated/jcode')
        self.assertEqual(env['HOME'], '/isolated/home')

    def test_binary_fingerprint_records_contents(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'binary'
            path.write_bytes(b'abc')
            self.assertEqual(profile.fingerprint(path)['sha256'],
                             'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad')
            self.assertEqual(profile.fingerprint(path)['bytes'], 3)


if __name__ == '__main__':
    unittest.main()
