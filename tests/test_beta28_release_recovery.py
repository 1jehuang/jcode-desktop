#!/usr/bin/env python3
"""One-off beta28 workflow contracts using only the Python standard library.

Run: python3 -m unittest discover -s tests -p test_beta28_release_recovery.py -v
Committed build steps are archived from the immutable commit, never dirty workflows.
The baseline fixture also works in the public publisher's shallow checkout.
These text and shell contracts supplement, not replace, actionlint and dispatch.
"""
import os
import json
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / '.github/workflows/beta28-release-recovery.yml'
DESKTOP_SHA = 'ddad95424cd098e82cc76ca91c99742a6f654927'
RUNTIME_SHA = '56fb1f163aff19f5baeeda642c79f8499e597d0f'
TAG = 'desktop-v0.1.0-beta.28'
TEXT = WORKFLOW.read_text()


def committed(name):
    baseline = json.loads((ROOT / 'tests/fixtures/beta28-original-workflows.json').read_text())
    assert baseline['desktop_sha'] == DESKTOP_SHA
    return baseline['workflows'][name]


def job(name):
    return re.search(rf'^  {re.escape(name)}:\n(.*?)(?=^  [\w-]+:\n|\Z)',
                     TEXT, re.M | re.S)[1]


def steps(text):
    return re.findall(r'^      - .*?(?=^      - |^  [\w-]+:\n|\Z)',
                      text, re.M | re.S)


def step(text, name):
    return next(s for s in steps(text) if s.startswith(f'      - name: {name}\n'))


def script(text):
    body = text.split('        run: |\n', 1)[1]
    return '\n'.join(line[10:] for line in body.splitlines() if line.strip()) + '\n'


class Beta28RecoveryContract(unittest.TestCase):
    def test_dispatch_and_permissions(self):
        self.assertIn('on:\n  workflow_dispatch:\n\n', TEXT)
        self.assertNotIn('  push:', TEXT)
        self.assertNotIn('    inputs:', TEXT)
        self.assertIn("if: github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main'", job('prepare'))
        self.assertIn('permissions:\n  contents: write\n  actions: write', TEXT)
        self.assertIn('group: desktop-release-' + TAG, TEXT)
        self.assertIn('cancel-in-progress: false', TEXT)

    def test_immutable_sources_in_every_job(self):
        for key, value in [('RELEASE_TAG', TAG), ('DESKTOP_SHA', DESKTOP_SHA), ('RUNTIME_SHA', RUNTIME_SHA)]:
            self.assertEqual(TEXT.count(f'  {key}: {value}\n'), 1)
        for name in ['prepare', 'macos', 'cross-platform', 'publish']:
            with self.subTest(job=name):
                body = job(name)
                self.assertIn('ref: ${{ env.RELEASE_TAG }}', body)
                self.assertIn('fetch-depth: 0', body)
                self.assertIn('test "$(git -C jcode-desktop rev-parse HEAD)" = "$DESKTOP_SHA"', body)
                self.assertIn('test "$(git -C jcode-desktop rev-parse "$RELEASE_TAG^{}")" = "$DESKTOP_SHA"', body)
        for name in ['macos', 'cross-platform']:
            body = job(name)
            self.assertIn('repository: 1jehuang/jcode\n          ref: ${{ env.RUNTIME_SHA }}', body)
            self.assertIn('submodules: recursive', body)
            self.assertIn('test "$(git -C jcode rev-parse HEAD)" = "$RUNTIME_SHA"', body)
        self.assertEqual(TEXT.count('persist-credentials: false'), 6)
        self.assertNotIn('ref: main', TEXT)

    def test_dependency_graph_and_no_bypass(self):
        self.assertEqual(set(re.findall(r'^  ([\w-]+):\n', TEXT.split('jobs:\n')[1], re.M)),
                         {'prepare', 'macos', 'cross-platform', 'publish'})
        for name in ['macos', 'cross-platform']:
            self.assertIn('    needs: prepare\n', job(name))
        self.assertIn('    needs: [macos, cross-platform]\n', job('publish'))
        self.assertNotIn('continue-on-error', TEXT)
        self.assertNotIn('always()', job('publish'))
        self.assertIn('fail-fast: false', job('cross-platform'))
        for name in ['macos', 'cross-platform']:
            self.assertIn('timeout-minutes: 180', job(name))
        self.assertIn('runs-on: macos-14', job('macos'))
        for runner in ['ubuntu-24.04', 'windows-2025']:
            self.assertIn('os: ' + runner, job('cross-platform'))

    def test_no_artifact_actions_or_other_publication_paths(self):
        for forbidden in ['actions/upload-artifact', 'actions/download-artifact',
                          'desktop-updates', 'JCODE_PUBLIC_RELEASE_TOKEN', 'git push',
                          'git tag ', 'gh release delete', '--target', 'GITHUB_REF_NAME',
                          'inputs.version', 'github.ref_name']:
            self.assertNotIn(forbidden, TEXT)
        self.assertEqual(TEXT.count('gh release create '), 1)
        self.assertEqual(TEXT.count('gh release edit '), 1)
        self.assertEqual(TEXT.count('gh release upload '), 3)
        self.assertIn('--verify-tag --draft --prerelease', job('prepare'))

    def test_complete_cross_platform_steps_are_preserved(self):
        original = committed('cross-platform-release')
        names = ['Install Linux build dependencies', 'Build and package Linux',
                 'Verify Linux package', 'Smoke-test Linux package on X11',
                 'Smoke-test Linux package on native Wayland', 'Print Linux smoke diagnostics',
                 'Build and package Windows', 'Verify Windows package', 'Smoke-test Windows package']
        for name in names:
            with self.subTest(step=name):
                expected = step(original, name).replace('${{ inputs.version || github.ref_name }}', '${{ env.RELEASE_TAG }}')
                # The only inter-step comments removed described pre-smoke uploads.
                expected = expected.split('      # Keep the hour-long')[0]
                self.assertEqual(step(job('cross-platform'), name).rstrip(), expected.rstrip())

    def test_macos_packaging_and_signing_steps_are_preserved(self):
        original = committed('macos-beta')
        for name in ['Import Developer ID certificate', 'Configure notarization',
                     'Build universal app, DMG, and ZIP', 'Verify app bundle and DMG install path',
                     'Sign automatic update and generate appcast']:
            with self.subTest(step=name):
                expected = step(original, name)
                expected = expected.replace('${{ inputs.version || github.ref_name }}', '${{ env.RELEASE_TAG }}')
                expected = expected.replace("${{ secrets.APPLE_SIGNING_IDENTITY || '-' }}", '${{ secrets.APPLE_SIGNING_IDENTITY }}')
                expected = expected.replace("${{ env.HAS_SPARKLE_KEYS == 'true' && '1' || '0' }}", '"1"')
                expected = expected.replace("${{ secrets.APPLE_API_KEY_P8 != '' && '1' || '0' }}", '"1"')
                expected = expected.replace("startsWith(github.ref, 'refs/tags/desktop-v')", "startsWith(env.RELEASE_TAG, 'desktop-v')")
                expected = expected.replace('GITHUB_REF_NAME', 'RELEASE_TAG')
                self.assertEqual(step(job('macos'), name).rstrip(), expected.rstrip())
        self.assertIn('BUILD_NUMBER: "47"', job('macos'))
        self.assertNotIn('github.run_number', TEXT)
        self.assertNotIn('$GITHUB_REF', job('macos'))

    def test_credentials_required_even_on_main(self):
        credentials = step(job('macos'), 'Validate release credentials')
        names = ['CERTIFICATE_P12', 'CERTIFICATE_PASSWORD', 'SIGNING_IDENTITY',
                 'API_KEY_P8', 'API_KEY_ID', 'API_ISSUER_ID',
                 'SPARKLE_PUBLIC_KEY', 'SPARKLE_PRIVATE_KEY']
        for name in names:
            self.assertRegex(credentials, rf'{name}: \$\{{\{{ secrets\.[A-Z0-9_]+ \}}\}}')
        env = {**os.environ, **dict.fromkeys(names, 'present'), 'GITHUB_REF': 'refs/heads/main'}
        self.assertEqual(subprocess.run(['bash', '-e', '-c', script(credentials)], env=env, capture_output=True).returncode, 0)
        for missing in [None] + names:
            with self.subTest(missing=missing):
                values = dict.fromkeys(names, '') if missing is None else {missing: ''}
                result = subprocess.run(['bash', '-e', '-c', script(credentials)], env={**env, **values}, capture_output=True)
                self.assertNotEqual(result.returncode, 0)

    def test_each_transfer_is_after_all_platform_gates(self):
        mac = job('macos')
        cross = job('cross-platform')
        self.assertLess(mac.index('Sign automatic update and generate appcast'), mac.index('gh release upload'))
        for check in ['Smoke-test Linux package on X11', 'Smoke-test Linux package on native Wayland', 'Smoke-test Windows package']:
            self.assertLess(cross.index(check), cross.index('gh release upload'))
        for name in ['macos', 'linux', 'windows']:
            transfer = step(mac if name == 'macos' else cross, f'Transfer verified {name} assets to private draft')
            self.assertNotIn('always()', transfer)
            self.assertIn('shell: bash', transfer)
            self.assertIn('gh release upload "$RELEASE_TAG" --repo "$GITHUB_REPOSITORY" --clobber', transfer)
            if name != 'macos':
                self.assertIn(f"if: matrix.platform == '{name}'", transfer)

    def test_every_release_write_rechecks_private_draft(self):
        for body in [job('macos'), job('cross-platform'), job('publish')]:
            for block in steps(body):
                if 'gh release upload ' not in block and 'gh release edit ' not in block:
                    continue
                with self.subTest(step=block.splitlines()[0]):
                    command = 'gh release upload ' if 'gh release upload ' in block else 'gh release edit '
                    self.assertIn('set -euo pipefail', block)
                    self.assertIn('test "$(gh api "repos/$GITHUB_REPOSITORY" --jq .private)" = true', block)
                    self.assertIn('.tagName == env.RELEASE_TAG and .isDraft == true and .isPrerelease == true', block)
                    self.assertLess(block.index('gh release view '), block.index(command))
                    self.assertIn('| grep -Fx "$RELEASE_TAG"', block)

    def test_draft_guards_fail_closed_before_upload(self):
        block = step(job('macos'), 'Transfer verified macos assets to private draft')
        # Exercise actual shell gate with a local gh stub, never network or credentials.
        stub = '''gh() {
          if [[ "$1" == api ]]; then printf '%s\\n' "$PRIVATE";
          elif [[ "$1 $2" == 'release view' ]]; then
            [[ "$DRAFT" == true ]] && printf '%s\\n' "$RELEASE_TAG";
          else echo WRITE_REACHED; fi
        }
'''
        for private, draft, allowed in [('true', 'true', True), ('true', 'false', False), ('false', 'true', False), ('', '', False)]:
            result = subprocess.run(['bash', '-c', stub + script(block)], capture_output=True, text=True,
                                    env={**os.environ, 'PRIVATE': private, 'DRAFT': draft, 'RELEASE_TAG': TAG, 'GITHUB_REPOSITORY': 'example/private'})
            self.assertEqual(result.returncode == 0, allowed)
            self.assertEqual('WRITE_REACHED' in result.stdout, allowed)

    def test_explicit_asset_transport_and_publication_order(self):
        assets = ['Jcode-macOS-universal.dmg', 'Jcode-0.1.0-beta.28-macOS-universal.zip',
                  'SHA256SUMS', 'appcast.xml', 'Jcode-0.1.0-beta.28-linux-x86_64.tar.gz',
                  'Jcode-0.1.0-beta.28-linux-amd64.deb', 'SHA256SUMS-linux',
                  'Jcode-0.1.0-beta.28-windows-x86_64.zip', 'SHA256SUMS-windows']
        final = job('publish')
        self.assertEqual(re.findall(r'--pattern ([^\s]+)', final), assets)
        for asset in assets:
            self.assertRegex(TEXT, r'jcode-desktop/dist/(?:macos|linux|windows)/' + re.escape(asset) + r'(?:\s|$)')
        commands = ['python3 -m unittest discover -s tests -v', 'gh release download ',
                    'python3 jcode-desktop/scripts/prepare-public-release.py artifacts "$RELEASE_TAG" --manifest artifacts/latest.json',
                    'gh release edit "$RELEASE_TAG" --repo "$GITHUB_REPOSITORY" --draft=false --prerelease',
                    'gh workflow run publish-public-release.yml --repo "$GITHUB_REPOSITORY" --ref main -f tag="$RELEASE_TAG"']
        offsets = [final.index(command) for command in commands]
        self.assertEqual(offsets, sorted(offsets))
        self.assertIn('working-directory: jcode-desktop', final)


if __name__ == '__main__':
    unittest.main()
