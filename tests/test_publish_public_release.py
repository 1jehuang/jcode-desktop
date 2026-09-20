import importlib.util
import hashlib
import io
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import test_prepare_public_release as fixtures

TAG = fixtures.TAG

SPEC = importlib.util.spec_from_file_location(
    "publish_public_release", Path(__file__).resolve().parents[1] / "scripts/publish-public-release.py"
)
PUBLISH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PUBLISH)


class PublisherTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.PublicReleaseTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.directory = self.fixture.directory

    @patch.object(PUBLISH, "verify_download")
    @patch.object(PUBLISH, "gh")
    @patch.object(PUBLISH, "release", return_value=None)
    def test_publishes_allowlisted_binaries_and_no_private_source(self, release, gh, verify):
        (self.directory / "private-source.tar.gz").write_text("secret source")
        PUBLISH.publish(self.directory, TAG)
        calls = [call.args for call in gh.call_args_list]
        version_upload = next(c for c in calls if c[:3] == ("release", "upload", TAG))
        self.assertEqual(len([p for p in version_upload if str(self.directory) in p]), 17)
        self.assertFalse(any("private-source" in str(c) for c in calls))
        self.assertTrue(all(PUBLISH.PUBLIC_REPOSITORY in c for c in calls))
        create = next(c for c in calls if c[:3] == ("release", "create", TAG))
        self.assertIn("main", create)
        self.assertNotIn("1jehuang/jcode-desktop", create)
        self.assertEqual(verify.call_count, 16)

    @patch.object(PUBLISH, "verify_download", side_effect=ValueError("bad public bytes"))
    @patch.object(PUBLISH, "gh")
    @patch.object(PUBLISH, "release", return_value=None)
    def test_failed_anonymous_download_does_not_promote_channel(self, release, gh, verify):
        with self.assertRaisesRegex(ValueError, "bad public bytes"):
            PUBLISH.publish(self.directory, TAG)
        self.assertFalse(any(PUBLISH.CHANNEL in c.args for c in gh.call_args_list))

    def test_beta_order_is_numeric_and_stable_is_newer(self):
        self.assertLess(PUBLISH.version_key("desktop-v0.1.0-beta.9"), PUBLISH.version_key("desktop-v0.1.0-beta.24"))
        self.assertLess(PUBLISH.version_key(TAG), PUBLISH.version_key("desktop-v0.1.0"))
        self.assertLess(PUBLISH.version_key("desktop-v0.1.0"), PUBLISH.version_key("desktop-v0.2.0-beta.1"))

    def test_download_verifier_has_an_identity_but_no_credentials(self):
        payload = b"public package"
        with patch.object(PUBLISH.urllib.request, "urlopen", return_value=io.BytesIO(payload)) as open_url:
            PUBLISH.verify_download("https://jcode.sh/desktop/package.zip", hashlib.sha256(payload).hexdigest(), len(payload))
        request = open_url.call_args.args[0]
        self.assertEqual(request.get_header("User-agent"), "JcodeReleaseVerifier/1.0")
        self.assertFalse(request.has_header("Authorization"))
        self.assertFalse(request.has_header("Cookie"))

    @patch.object(PUBLISH, "gh", return_value=subprocess.CompletedProcess([], 1, "", "network failure"))
    def test_api_failure_is_not_treated_as_missing_release(self, gh):
        with self.assertRaisesRegex(RuntimeError, "network failure"):
            PUBLISH.release(TAG)

    @patch.object(PUBLISH, "verify_download")
    @patch.object(PUBLISH, "release", return_value={"isDraft": False, "assets": []})
    def test_published_version_cannot_be_overwritten(self, release, verify):
        def download(*args, **kwargs):
            directory = Path(args[args.index("--dir") + 1])
            (directory / "latest.json").write_text(json.dumps({"tag_name": "different"}))
        with patch.object(PUBLISH, "gh", side_effect=download) as gh:
            with self.assertRaisesRegex(ValueError, "already published"):
                PUBLISH.publish(self.directory, TAG)
            self.assertEqual(gh.call_count, 1)
        verify.assert_not_called()


class StablePublisherTests(unittest.TestCase):
    def fixture(self, tag):
        fixture = fixtures.PublicReleaseTests()
        fixture.tag = tag
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        return fixture.directory

    def test_new_versions_and_draft_retries_use_tag_metadata(self):
        for tag, prerelease in [("desktop-v0.2.0", False), ("desktop-v0.2.1-beta.1", True)]:
            for draft in (False, True):
                with self.subTest(tag=tag, draft=draft):
                    directory = self.fixture(tag)
                    current = {"isDraft": True, "isPrerelease": not prerelease, "assets": []} if draft else None
                    with patch.object(PUBLISH, "release", side_effect=[current, None]), \
                         patch.object(PUBLISH, "gh") as gh, patch.object(PUBLISH, "verify_download") as verify:
                        PUBLISH.publish(directory, tag)
                    calls = [c.args for c in gh.call_args_list]
                    flag = f"--prerelease={str(prerelease).lower()}"
                    version_creates = [c for c in calls if c[:3] == ("release", "create", tag)]
                    self.assertEqual(len(version_creates), 0 if draft else 1)
                    if not draft:
                        self.assertIn(flag, version_creates[0])
                        self.assertIn("--draft", version_creates[0])
                    edit = next(c for c in calls if c[:3] == ("release", "edit", tag))
                    self.assertIn(flag, edit)
                    self.assertIn("--draft=false", edit)
                    channel = next(c for c in calls if c[:3] == ("release", "create", PUBLISH.CHANNEL))
                    self.assertIn("--prerelease", channel, "Metadata-only channel is never a stable binary release")
                    manifest = json.loads((directory / "latest.json").read_text())
                    self.assertIs(manifest["prerelease"], prerelease)
                    self.assertEqual(verify.call_count, 16)

    def test_published_retry_only_repairs_metadata_after_byte_verification(self):
        tag = "desktop-v0.2.0"
        for wrong_metadata, corrupt_bytes in [(False, False), (True, False), (True, True)]:
            with self.subTest(wrong_metadata=wrong_metadata, corrupt_bytes=corrupt_bytes):
                directory = self.fixture(tag)
                manifest = PUBLISH.PREPARE.validate(directory, tag)
                events = []

                def gh_call(*args, **kwargs):
                    events.append(args)
                    if args[:2] == ("release", "download"):
                        (Path(args[args.index("--dir") + 1]) / "latest.json").write_text(json.dumps(manifest))

                def verify(*args):
                    events.append(("verified",))
                    if corrupt_bytes:
                        raise ValueError("bad public bytes")

                current = {"isDraft": False, "isPrerelease": wrong_metadata, "assets": []}
                with patch.object(PUBLISH, "release", side_effect=[current, None]), \
                     patch.object(PUBLISH, "gh", side_effect=gh_call), \
                     patch.object(PUBLISH, "verify_download", side_effect=verify):
                    if corrupt_bytes:
                        with self.assertRaisesRegex(ValueError, "bad public bytes"):
                            PUBLISH.publish(directory, tag)
                    else:
                        PUBLISH.publish(directory, tag)
                self.assertFalse(any(c[:3] == ("release", "upload", tag) for c in events))
                edits = [c for c in events if c[:3] == ("release", "edit", tag)]
                self.assertEqual(len(edits), int(wrong_metadata and not corrupt_bytes))
                if edits:
                    self.assertIn("--prerelease=false", edits[0])
                    self.assertEqual(events[1:17], [("verified",)] * 16)
                    self.assertEqual(events[17], edits[0])

    def test_stable_channel_resists_newer_beta_but_allows_stable_upgrade(self):
        cases = [
            ("desktop-v0.2.0", "desktop-v0.3.0-beta.1", False),
            ("desktop-v0.2.0", "desktop-v0.2.1", True),
            ("desktop-v0.2.0", "desktop-v0.1.0", False),
            ("desktop-v0.2.0-beta.29", "desktop-v0.2.0", True),
            ("desktop-v0.2.0-beta.9", "desktop-v0.2.0-beta.10", True),
            ("desktop-v0.2.0-beta.10", "desktop-v0.2.0-beta.9", False),
        ]
        for previous, tag, promote in cases:
            with self.subTest(previous=previous, tag=tag):
                directory = self.fixture(tag)

                def gh_call(*args, **kwargs):
                    if args[:3] == ("release", "download", PUBLISH.CHANNEL):
                        (Path(args[args.index("--dir") + 1]) / "latest.json").write_text(
                            json.dumps({"tag_name": previous}))
                    return subprocess.CompletedProcess([], 0, "", "")

                channel = {"isDraft": False, "isPrerelease": True, "assets": [{"name": "latest.json"}]}
                with patch.object(PUBLISH, "release", side_effect=[None, channel]), \
                     patch.object(PUBLISH, "gh", side_effect=gh_call) as gh, \
                     patch.object(PUBLISH, "verify_download") as verify:
                    PUBLISH.publish(directory, tag)
                calls = [c.args for c in gh.call_args_list]
                self.assertTrue(any(c[:3] == ("release", "upload", tag) for c in calls))
                self.assertEqual(any(c[:3] == ("release", "upload", PUBLISH.CHANNEL) for c in calls), promote)
                self.assertEqual(verify.call_count, 16)

    def test_incomplete_stable_release_never_calls_github(self):
        tag = "desktop-v0.2.0"
        directory = self.fixture(tag)
        (directory / "SHA256SUMS-freebsd-x86_64").unlink()
        with patch.object(PUBLISH, "gh") as gh:
            with self.assertRaises(FileNotFoundError):
                PUBLISH.publish(directory, tag)
        gh.assert_not_called()


if __name__ == "__main__":
    unittest.main()
