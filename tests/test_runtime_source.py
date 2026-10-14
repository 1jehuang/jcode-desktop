"""Source preflight regressions. Native builds remain the full API compatibility gate."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('runtime_source', ROOT / 'scripts/verify-runtime-source.py')
RUNTIME = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNTIME)


class RuntimeSourceTests(unittest.TestCase):
    def sources(self):
        sources = {'Cargo.toml': '', 'crates/jcode-build-meta/Cargo.toml': ''}
        for path, package, features in RUNTIME.runtime_dependencies(ROOT):
            sources[path] = f'[package]\nname = "{package}"\n[features]\n' + ''.join(
                f'"{feature}" = []\n' for feature in features)
        sources.update({
            'crates/jcode-harness-api/src/events.rs': 'pub user_connected: bool',
            'crates/jcode-sdk/src/client.rs': 'pub supports_user_response: bool\npub fn runtime_command(',
            'crates/jcode-sdk/src/ssh.rs': 'pub supports_user_response: bool',
            'crates/jcode-sdk/src/collections.rs': 'pub fn create_with_id(',
            'crates/jcode-harness-api/src/runtime_commands.rs': 'pub enum RuntimeCommand {',
            'crates/jcode-voice/src/lib.rs': 'pub mod dictation;\npub mod global;\npub use streaming_capture::',
        })
        return sources

    def test_current_desktop_dependencies_include_voice_and_target_specific_base(self):
        deps = RUNTIME.runtime_dependencies(ROOT)
        self.assertIn(('crates/jcode-voice/Cargo.toml', 'jcode-voice', ['voice-capture']), deps)
        self.assertGreater(sum(package == 'jcode-base' for _, package, _ in deps), 1)
        self.assertIn(('crates/jcode-core/Cargo.toml', 'jcode-core', []), deps)

    def test_compatible_source_passes(self):
        manifests = RUNTIME.verify(ROOT, self.sources().__getitem__)
        self.assertIn('crates/jcode-voice/Cargo.toml', manifests)

    def test_missing_voice_manifest_fails_before_api_checks(self):
        sources = self.sources()
        del sources['crates/jcode-voice/Cargo.toml']
        def read(path):
            if path not in sources:
                raise ValueError(f'Missing required runtime source: {path}')
            return sources[path]
        with self.assertRaisesRegex(ValueError, 'jcode-voice/Cargo.toml'):
            RUNTIME.verify(ROOT, read)

    def test_every_known_api_boundary_is_required(self):
        for path in RUNTIME.API_REQUIREMENTS:
            with self.subTest(path=path):
                sources = self.sources()
                sources[path] = '// API absent in older runtime'
                with self.assertRaisesRegex(ValueError, 'Missing required runtime API'):
                    RUNTIME.verify(ROOT, sources.__getitem__)

    def test_missing_capture_feature_fails(self):
        sources = self.sources()
        sources['crates/jcode-voice/Cargo.toml'] = '[package]\nname = "jcode-voice"\n'
        with self.assertRaisesRegex(ValueError, 'jcode-voice/voice-capture'):
            RUNTIME.verify(ROOT, sources.__getitem__)

    def test_wrong_dependency_package_fails(self):
        sources = self.sources()
        sources['crates/jcode-voice/Cargo.toml'] = '[package]\nname = "wrong-crate"\n'
        with self.assertRaisesRegex(ValueError, 'Runtime package mismatch'):
            RUNTIME.verify(ROOT, sources.__getitem__)

    def test_git_revision_read_does_not_read_dirty_checkout_files(self):
        with patch.object(RUNTIME.subprocess, 'check_output', return_value=b'committed source') as show:
            self.assertEqual(RUNTIME.runtime_reader(ROOT, 'a' * 40)('crates/example/src/lib.rs'), 'committed source')
        self.assertIn('show', show.call_args.args[0])
        self.assertEqual(show.call_args.args[0][-1], 'a' * 40 + ':crates/example/src/lib.rs')

    def test_mutable_revision_rejected(self):
        with self.assertRaisesRegex(ValueError, 'immutable'):
            RUNTIME.runtime_reader(ROOT, 'master')

    def test_missing_source_has_actionable_error(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, 'crates/jcode-voice/Cargo.toml'):
                RUNTIME.runtime_reader(directory)('crates/jcode-voice/Cargo.toml')

    def test_checkout_mismatch_fails_before_source_read(self):
        with patch.object(RUNTIME.subprocess, 'check_output', return_value='b' * 40), \
             patch.object(RUNTIME, 'verify') as verify:
            with self.assertRaises(SystemExit) as exit:
                RUNTIME.main(['--runtime', str(ROOT), '--expected-commit', 'a' * 40])
        self.assertEqual(exit.exception.code, 1)
        verify.assert_not_called()

    def test_dirty_exact_checkout_rejected(self):
        with patch.object(RUNTIME.subprocess, 'check_output', return_value='a' * 40), \
             patch.object(RUNTIME.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, ['git', 'diff'])), \
             patch.object(RUNTIME, 'verify') as verify:
            with self.assertRaises(SystemExit) as exit:
                RUNTIME.main(['--runtime', str(ROOT), '--expected-commit', 'a' * 40])
        self.assertEqual(exit.exception.code, 1)
        verify.assert_not_called()


if __name__ == '__main__':
    unittest.main()
