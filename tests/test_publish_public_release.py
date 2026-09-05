import importlib.util
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
        self.assertEqual(len([p for p in version_upload if str(self.directory) in p]), 10)
        self.assertFalse(any("private-source" in str(c) for c in calls))
        self.assertTrue(all(PUBLISH.PUBLIC_REPOSITORY in c for c in calls))
        create = next(c for c in calls if c[:3] == ("release", "create", TAG))
        self.assertIn("main", create)
        self.assertNotIn("1jehuang/jcode-desktop", create)
        self.assertEqual(verify.call_count, 9)

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


if __name__ == "__main__":
    unittest.main()
