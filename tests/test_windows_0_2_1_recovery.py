"""Windows recovery contracts and actual Bash transport regression tests.

The shell harness works on both Linux and native Windows Git Bash. Network and
release writes are stubbed. It supplements, not replaces, fresh native builds.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

from test_stable_release_workflows import script, workflow_step

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = 'windows-0.2.1-recovery.yml'
TEXT = (ROOT / '.github/workflows' / WORKFLOW).read_text()
SHA = '4c5495f85a03671b20a00a108f10e6dbe041584d'
RUNTIME = '91c0bdda3884952bd2eba2e5cfc78fbed1281495'
TAG = 'desktop-v0.2.1'
UPLOAD = 'Upload only native-verified missing Windows assets'
BASH = os.environ.get('RELEASE_TEST_BASH') or shutil.which('bash')

# Functions run inside the real workflow shell. Paths passed across Python and
# Git Bash use forward slashes, including Windows temporary directory paths.
STUB = r'''
gh() {
  printf '%s\n' "$*" >> "$CALLS"
  case "$1 $2" in
    "api "*)
      [[ "$API_STATUS" == 0 ]] || return "$API_STATUS"
      if [[ "$*" == *'.object.type'* ]]; then echo "$TAG_TYPE";
      elif [[ "$*" == *'/git/tags/'* || "$TAG_TYPE" == commit ]]; then echo "$REMOTE_SHA";
      else echo annotated-tag-object; fi ;;
    "release view")
      [[ "$VIEW_STATUS" == 0 ]] || return "$VIEW_STATUS"
      if [[ "$*" == *'--json assets'* ]]; then
        [[ "$LIST_STATUS" == 0 ]] || return "$LIST_STATUS"
        find "$REMOTE" -maxdepth 1 -type f -printf '%f\n'
      else
        while [[ "$1" != --jq ]]; do shift; done
        metadata=$METADATA
        if [[ -f "$REMOTE/uploaded-marker" ]]; then metadata=$AFTER_METADATA; fi
        printf '%s\n' "$metadata" | jq -r "$2"
      fi ;;
    "release upload")
      [[ "$UPLOAD_STATUS" == 0 ]] || return "$UPLOAD_STATUS"
      shift 5
      for file in "$@"; do
        [[ "$file" != --clobber ]] || return 98
        [[ ! -e "$REMOTE/$(basename "$file")" ]] || return 97
        cp "$file" "$REMOTE/"
      done
      touch "$REMOTE/uploaded-marker" ;;
    "release download")
      [[ "$DOWNLOAD_STATUS" == 0 ]] || return "$DOWNLOAD_STATUS"
      shift 5
      [[ "$1" == --dir && "$3" == --pattern ]] || return 96
      cp "$REMOTE/$4" "$2/$4"
      if [[ "$CORRUPT_DOWNLOAD" == true ]]; then echo corrupt >> "$2/$4"; fi ;;
    *) return 95 ;;
  esac
}
'''


def windows_environment(values):
    """Model Windows' case-insensitive merge, preserving the first key spelling."""
    result = {}
    spellings = {}
    for key, value in values:
        canonical = spellings.setdefault(key.casefold(), key)
        result[canonical] = value
    return result


class RecoveryContracts(unittest.TestCase):
    def test_dispatch_only_no_publish_or_artifact_dependency(self):
        self.assertIn('on:\n  workflow_dispatch:\n\n', TEXT)
        for forbidden in ('inputs:', 'push:', 'gh release edit', 'gh release create',
                          'gh workflow run', 'actions/upload-artifact', 'actions/download-artifact',
                          '--clobber', 'git tag ', 'git push', 'continue-on-error', 'PLATFORM:'):
            self.assertNotIn(forbidden, TEXT)
        self.assertEqual(re.findall(r'^  ([\w-]+):', TEXT.split('jobs:\n')[1], re.M), ['recover-windows'])
        self.assertIn('contents: write', TEXT)
        self.assertIn('actions: read', TEXT)
        self.assertIn('cancel-in-progress: false', TEXT)
        self.assertIn('fail-fast: false', TEXT)
        self.assertIn('cache-on-failure: true', TEXT)
        self.assertIn('SKIP_BUILD: "0"', TEXT)

    def test_sources_and_native_matrix(self):
        self.assertIn('DESKTOP_SHA: ' + SHA, TEXT)
        self.assertIn('ref: desktop-v0.2.1\n          fetch-depth: 0', TEXT)
        self.assertIn('ref: ' + RUNTIME, TEXT)
        self.assertIn('submodules: recursive', TEXT)
        for assertion in ('test "$(git -C jcode-desktop rev-parse HEAD)" = "$DESKTOP_SHA"',
                          'test "$(git -C jcode-desktop rev-parse "$RELEASE_TAG^{}")" = "$DESKTOP_SHA"',
                          'test "$(git -C jcode rev-parse HEAD)" = "$RUNTIME_SHA"',
                          'git -C jcode-desktop diff HEAD --exit-code',
                          'git -C jcode diff HEAD --exit-code'):
            self.assertIn(assertion, TEXT)
        for runner, arch, target in [('windows-2025', 'x86_64', 'x86_64-pc-windows-msvc'),
                                     ('windows-11-arm', 'aarch64', 'aarch64-pc-windows-msvc')]:
            self.assertIn('os: ' + runner, TEXT)
            self.assertIn('arch: ' + arch, TEXT)
            self.assertIn('target: ' + target, TEXT)
        self.assertIn('RuntimeInformation]::OSArchitecture', TEXT)
        self.assertLess(TEXT.index('Configure MSVC build environment'), TEXT.index('Test Windows transport'))
        self.assertLess(TEXT.index('Smoke-test Windows package'), TEXT.index(UPLOAD))
        self.assertLess(TEXT.index('Verify sources remain unchanged'), TEXT.index(UPLOAD))

    def test_original_build_verify_cli_and_eight_second_gui_preserved(self):
        # The only permitted build differences are a fixed version, removal of
        # the redundant platform condition, and explicitly disallowing skip-build.
        for name in ('Build and package Windows', 'Verify Windows package', 'Smoke-test Windows package'):
            original = workflow_step('cross-platform-release.yml', name)
            expected = original.replace("        if: matrix.platform == 'windows'\n", '')
            expected = expected.replace("${{ startsWith(github.ref, 'refs/tags/desktop-v') && github.ref_name || inputs.version }}", '${{ env.RELEASE_TAG }}')
            expected = expected.replace('          VERSION:', '          SKIP_BUILD: "0"\n          VERSION:')
            self.assertEqual(workflow_step(WORKFLOW, name), expected)
        self.assertIn('Start-Sleep -Seconds 8', TEXT)
        self.assertIn('run: ./jcode-desktop/scripts/package-windows.ps1', TEXT)
        self.assertIn('test_release_targets.py', TEXT)

    def test_future_transport_uses_collision_safe_name(self):
        block = workflow_step('cross-platform-release.yml', 'Upload verified platform packages to private draft')
        self.assertIn('RELEASE_PLATFORM: ${{ matrix.platform }}', block)
        self.assertNotRegex(block, r'(?<!RELEASE_)\bPLATFORM:|\$PLATFORM\b')
        self.assertIn('case "$RELEASE_PLATFORM" in', block)
        self.assertIn('dist/$RELEASE_PLATFORM/$CHECKSUMS', block)


@unittest.skipUnless(BASH and shutil.which('jq'), 'bash and jq required')
class TransportTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.remote = self.root / 'remote'
        self.remote.mkdir()
        self.dist = self.root / 'jcode-desktop/dist/windows'
        self.dist.mkdir(parents=True)
        self.calls = self.root / 'calls'
        self.metadata = json.dumps(dict(tagName=TAG, isDraft=True, isPrerelease=False))
        self.env = {k: v for k, v in os.environ.items() if k.casefold() not in ('platform', 'release_platform')}
        self.env.update(CALLS=self.calls.as_posix(), REMOTE=self.remote.as_posix(), RELEASE_TAG=TAG,
                        DESKTOP_SHA=SHA, REMOTE_SHA=SHA, TAG_TYPE='tag', GITHUB_REPOSITORY='example/private',
                        METADATA=self.metadata, AFTER_METADATA=self.metadata, API_STATUS='0', VIEW_STATUS='0',
                        LIST_STATUS='0', UPLOAD_STATUS='0', DOWNLOAD_STATUS='0', CORRUPT_DOWNLOAD='false')
        self.body = script(workflow_step(WORKFLOW, UPLOAD))
        self.assets('x86_64')

    def assets(self, arch):
        self.zip = self.dist / f'Jcode-0.2.1-windows-{arch}.zip'
        self.zip.write_bytes(b'native-tested-package-' + arch.encode())
        self.checksum = self.dist / ('SHA256SUMS-windows' + ('-aarch64' if arch == 'aarch64' else ''))
        self.checksum.write_text(hashlib.sha256(self.zip.read_bytes()).hexdigest() + '  ' + self.zip.name + '\n')
        self.env.update(RELEASE_ARCH=arch, RELEASE_CHECKSUMS=self.checksum.name)

    def run_shell(self, body=None, stub=STUB, **env):
        self.calls.write_text('')
        result = subprocess.run([BASH, '-c', stub + '\n' + (self.body if body is None else body)],
                                cwd=self.root, env={**self.env, **env}, capture_output=True, text=True, timeout=30)
        return result, self.calls.read_text()

    def test_fresh_upload_and_round_trip_for_both_native_architectures(self):
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                self.assets(arch)
                result, calls = self.run_shell(Platform='ARM64')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((self.remote / self.zip.name).read_bytes(), self.zip.read_bytes())
                self.assertEqual(calls.count('release upload '), 1)
                self.assertEqual(calls.count('release download '), 2)
                self.assertEqual(calls.count('--json tagName,isDraft,isPrerelease'), 3)
                self.assertTrue(calls.splitlines()[-1].startswith('release view '))

    def test_partial_or_complete_existing_assets_never_clobbered(self):
        for existing in ((self.zip,), (self.checksum,), (self.zip, self.checksum)):
            with self.subTest(existing=[p.name for p in existing]):
                for p in self.remote.iterdir():
                    p.unlink()
                for p in existing:
                    shutil.copyfile(p, self.remote / p.name)
                result, calls = self.run_shell()
                self.assertEqual(result.returncode, 0, result.stderr)
                uploads = [line for line in calls.splitlines() if line.startswith('release upload ')]
                self.assertEqual(len(uploads), 0 if len(existing) == 2 else 1)
                for p in existing:
                    self.assertFalse(any(p.name in line for line in uploads))

    def test_conflicting_existing_asset_blocks_all_writes(self):
        for existing in (self.zip, self.checksum):
            for p in self.remote.iterdir():
                p.unlink()
            (self.remote / existing.name).write_bytes(b'conflicting prior build')
            result, calls = self.run_shell()
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn('release upload ', calls)

    def test_metadata_and_tag_checks_fail_closed_before_write(self):
        cases = [dict(tagName='desktop-v0.2.2', isDraft=True, isPrerelease=False),
                 dict(tagName=TAG, isDraft=False, isPrerelease=False),
                 dict(tagName=TAG, isDraft=True, isPrerelease=True)]
        for metadata in cases:
            result, calls = self.run_shell(METADATA=json.dumps(metadata))
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn('release upload ', calls)
        for env in ({'REMOTE_SHA': '0'*40}, {'TAG_TYPE': 'tree'}, {'API_STATUS': '1'},
                    {'VIEW_STATUS': '1'}, {'LIST_STATUS': '1'}, {'RELEASE_ARCH': 'invalid'}):
            result, calls = self.run_shell(**env)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn('release upload ', calls)

    def test_missing_or_corrupt_local_package_blocks_write(self):
        self.zip.write_bytes(b'corrupt')
        result, calls = self.run_shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('release upload ', calls)
        self.zip.unlink()
        result, calls = self.run_shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('release upload ', calls)

    def test_upload_download_and_hash_failures_propagate(self):
        for env in ({'UPLOAD_STATUS': '1'}, {'DOWNLOAD_STATUS': '1'}, {'CORRUPT_DOWNLOAD': 'true'}):
            for p in self.remote.iterdir():
                p.unlink()
            result, _ = self.run_shell(**env)
            self.assertNotEqual(result.returncode, 0)

    def test_publication_during_upload_fails_post_transfer_gate(self):
        result, calls = self.run_shell(AFTER_METADATA=json.dumps(dict(tagName=TAG, isDraft=False, isPrerelease=False)))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('release upload ', calls)
        self.assertEqual(calls.count('release download '), 2)

    def test_lightweight_tag_is_also_verified(self):
        result, _ = self.run_shell(TAG_TYPE='commit')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_windows_case_collision_reproduces_old_failure_and_fixed_transport_passes(self):
        block = workflow_step('cross-platform-release.yml', 'Upload verified platform packages to private draft')
        body = script(block)
        # First spelling is inherited from MSVC. Windows replaces its value but
        # Bash still exposes only Platform, not the requested generic PLATFORM.
        collision = windows_environment([('Platform', 'ARM64'), ('PLATFORM', 'windows')])
        self.assertEqual(collision, {'Platform': 'windows'})
        stub = r'''
gh() {
  printf '%s\n' "$*" >> "$CALLS"
  if [[ "$1 $2" == 'release view' ]]; then echo true; fi
}
'''
        old = body.replace('RELEASE_PLATFORM', 'PLATFORM')
        result, calls = self.run_shell(old, stub=stub, GITHUB_REF_NAME=TAG,
                                      CHECKSUMS=self.checksum.name, **collision)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('PLATFORM: unbound variable', result.stderr)
        self.assertNotIn('release upload ', calls)
        fixed = windows_environment([('Platform', 'ARM64'), ('RELEASE_PLATFORM', 'windows')])
        result, calls = self.run_shell(body, stub=stub, GITHUB_REF_NAME=TAG,
                                      CHECKSUMS=self.checksum.name, **fixed)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('release upload ', calls)
        self.assertIn(self.zip.name, calls)
        self.assertIn(self.checksum.name, calls)


if __name__ == '__main__':
    unittest.main()
