import gzip
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('samply-top.py')


def fixture():
    return {
        'libs': [{'name': 'fixture', 'debugName': 'fixture', 'breakpadId': 'abc'}],
        'threads': [{
            'name': 'main', 'samples': {'stack': [1, 1, None], 'weight': [2, 3, 1]},
            'frameTable': {'func': [0, 1], 'address': [16, -1]},
            'funcTable': {'resource': [0, -1], 'name': [0, 1]},
            'resourceTable': {'lib': [0]}, 'stringArray': ['native', 'script-function'],
            'stackTable': {'frame': [0, 1], 'prefix': [None, 0]},
        }],
    }


class SamplyTopTests(unittest.TestCase):
    def run_profile(self, profile, *args, compressed=False, syms=None):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ('profile.json.gz' if compressed else 'profile.json')
            if compressed:
                with gzip.open(path, 'wt') as file:
                    json.dump(profile, file)
            else:
                path.write_text(json.dumps(profile))
            if syms is not None:
                (Path(directory) / 'profile.json.syms.json').write_text(json.dumps(syms))
            return subprocess.run([sys.executable, str(SCRIPT), str(path), *args],
                                  capture_output=True, text=True)

    def test_plain_json_missing_sidecar_weighted_stacks_and_script_names(self):
        result = self.run_profile(fixture())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('main: 5 samples', result.stdout)
        self.assertIn('script-function', result.stdout)
        self.assertIn('fixture+0x10', result.stdout)

    def test_gzip_sidecar_and_under_filter(self):
        syms = {'string_table': ['native-symbol'], 'data': [{
            'debug_name': 'fixture', 'debug_id': 'abc',
            'symbol_table': [{'rva': 16, 'size': 8, 'symbol': 0}],
        }]}
        result = self.run_profile(fixture(), '--under', 'native-symbol', compressed=True, syms=syms)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('native-symbol', result.stdout)
        self.assertIn('main: 5 samples', result.stdout)
        excluded = self.run_profile(fixture(), '--under', 'not-present')
        self.assertIn('main: 0 samples', excluded.stdout)

    def test_missing_thread_is_actionable_error(self):
        result = self.run_profile(fixture(), '--thread', 'missing')
        self.assertEqual(result.returncode, 2)
        self.assertIn('no matching thread', result.stderr)
        self.assertNotIn('Traceback', result.stderr)

    def test_child_thread_uses_child_library_table(self):
        child = fixture()
        child['libs'][0]['name'] = 'child-library'
        result = self.run_profile({'libs': [], 'threads': [], 'processes': [child]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('child-library+0x10', result.stdout)

    def test_nm_failure_is_not_silently_ignored(self):
        result = self.run_profile(fixture(), '--binary', 'fixture=/no/such/binary')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('CalledProcessError', result.stderr)


if __name__ == '__main__':
    unittest.main()
