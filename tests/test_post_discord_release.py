import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("discord_release", ROOT / "scripts/post_discord_release.py")
D = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(D)
TAG = "desktop-v0.2.0"
WEBHOOK = "https://discord.com/api/webhooks/123/SECRET"


def manifest(tag=TAG):
    groups = D.PREPARE.expected_assets(tag)
    names = set(groups) | {"appcast.xml"}
    for values in groups.values():
        names.update(values)
    return dict(tag_name=tag, name=f"Jcode Desktop {tag[9:]}", draft=False,
                prerelease="-beta." in tag, assets=[dict(name=n, size=3, sha256=hashlib.sha256(b"abc").hexdigest(),
                browser_download_url=f"{D.WEBSITE}/releases/{tag}/{n}") for n in sorted(names)])


def release(repository=D.SOURCE_REPOSITORY, tag=TAG):
    return dict(id=42, tag_name=tag, draft=False, prerelease="-beta." in tag,
                published_at="2026-09-20T00:00:00Z", body="- Improved Desktop terminals.\n",
                html_url=f"https://github.com/{repository}/releases/tag/{tag}",
                assets=[dict(name=a["name"], state="uploaded") for a in manifest(tag)["assets"]]
                       + [dict(name="latest.json", state="uploaded")])


class AnnouncementTests(unittest.TestCase):
    def setUp(self):
        self.fetch = self.enterContext(patch.object(D, "fetch_release", return_value=release()))
        self.verify = self.enterContext(patch.object(D, "verify_publication", return_value=release(D.PUBLIC_REPOSITORY)))
        self.post = self.enterContext(patch.object(D, "post_to_discord", return_value={"id": "100"}))
        self.mark = self.enterContext(patch.object(D, "mark_release_announced"))

    def announce(self, **kwargs):
        return D.announce_release(**(dict(repository=D.SOURCE_REPOSITORY, tag=TAG, token="token", webhook_url=WEBHOOK) | kwargs))

    def test_success_refetches_and_marks_only_source(self):
        latest = release() | {"body": "Concurrent edit\n<!-- other-marker -->"}
        self.fetch.side_effect = [release(), latest]
        self.assertEqual(self.announce(), "100")
        self.assertEqual(self.fetch.call_count, 2)
        self.assertEqual(self.mark.call_args.kwargs["release"], latest)
        self.assertEqual(self.mark.call_args.kwargs["repository"], D.SOURCE_REPOSITORY)
        content = self.post.call_args.kwargs["content"]
        self.assertIn("Jcode Desktop 0.2.0", content)
        self.assertIn(D.PUBLIC_REPOSITORY, content)
        self.assertNotIn(f"/{D.SOURCE_REPOSITORY}/releases", content)

    def test_duplicate_skips_network_and_post(self):
        self.fetch.return_value["body"] += D.announcement_marker(TAG)
        self.assertIsNone(self.announce())
        self.verify.assert_not_called()
        self.post.assert_not_called()
        self.mark.assert_not_called()

    def test_failed_post_never_marks(self):
        self.post.side_effect = RuntimeError("failed")
        with self.assertRaises(RuntimeError):
            self.announce()
        self.mark.assert_not_called()
        self.assertEqual(self.fetch.call_count, 1)

    def test_source_identity_and_publication_guard(self):
        for update in ({"draft": True}, {"published_at": None}, {"tag_name": "desktop-v0.1.0"},
                       {"html_url": "https://github.com/other/repo"}, {"prerelease": True}, {"id": "42"}):
            with self.subTest(update=update):
                self.fetch.return_value = release() | update
                with self.assertRaises(ValueError):
                    self.announce()
        self.post.assert_not_called()

    def test_invalid_tag_and_repository_never_fetch(self):
        for kwargs in ({"tag": "v0.2.0"}, {"tag": "desktop-v0.2.0\n"}, {"repository": D.PUBLIC_REPOSITORY}):
            with self.assertRaises(ValueError):
                self.announce(**kwargs)
        self.fetch.assert_not_called()

    def test_missing_public_release_and_failed_hash_never_post(self):
        for error in (ValueError("missing release"), ValueError("bad hash")):
            self.verify.side_effect = error
            with self.assertRaises(ValueError):
                self.announce()
        self.post.assert_not_called()
        self.mark.assert_not_called()

    def test_superseded_beta_does_not_post_or_mark(self):
        self.verify.return_value = None
        self.assertIsNone(self.announce())
        self.post.assert_not_called()
        self.mark.assert_not_called()

    def test_marker_failure_warns_without_secret_and_does_not_fail_post(self):
        self.mark.side_effect = RuntimeError(WEBHOOK)
        with patch("sys.stderr", new_callable=io.StringIO) as stderr:
            self.assertEqual(self.announce(), "100")
        self.assertIn("Do not rerun", stderr.getvalue())
        self.assertNotIn("SECRET", stderr.getvalue())

    def test_refetch_failure_is_also_safe_after_post(self):
        self.fetch.side_effect = [release(), RuntimeError(WEBHOOK)]
        with patch("sys.stderr", new_callable=io.StringIO) as stderr:
            self.assertEqual(self.announce(), "100")
        self.assertNotIn("SECRET", stderr.getvalue())
        self.mark.assert_not_called()

    def test_replaced_source_is_not_marked(self):
        self.fetch.side_effect = [release(), release() | {"id": 43}]
        with patch("sys.stderr", new_callable=io.StringIO):
            self.announce()
        self.mark.assert_not_called()


class PublicGuardTests(unittest.TestCase):
    def setUp(self):
        self.public = release(D.PUBLIC_REPOSITORY)
        self.live = manifest()
        self.fetch = self.enterContext(patch.object(D, "fetch_release", return_value=self.public))
        self.json = self.enterContext(patch.object(D, "request_json", return_value=self.live))
        self.verify = self.enterContext(patch.object(D, "verify_website_asset"))

    def test_live_immutable_metadata_and_all_hashes_checked_anonymously(self):
        self.assertEqual(D.verify_publication(TAG), self.public)
        self.fetch.assert_called_once_with(repository=D.PUBLIC_REPOSITORY, tag=TAG)
        self.assertEqual(self.verify.call_count, len(self.live["assets"]))
        self.assertEqual(self.json.call_count, 3)
        self.assertTrue(all("token" not in c.kwargs for c in self.json.call_args_list))

    def test_public_draft_unpublished_or_wrong_identity_rejected(self):
        for update in ({"draft": True}, {"published_at": None}, {"tag_name": "desktop-v0.1.0"},
                       {"html_url": release()["html_url"]}, {"prerelease": True}):
            self.fetch.return_value = self.public | update
            with self.assertRaises(ValueError):
                D.verify_publication(TAG)
        self.verify.assert_not_called()

    def test_missing_public_release_rejected(self):
        self.fetch.side_effect = RuntimeError("404")
        with self.assertRaises(RuntimeError):
            D.verify_publication(TAG)
        self.json.assert_not_called()

    def test_live_wrong_tag_rejected(self):
        self.json.return_value = manifest("desktop-v0.2.1")
        with self.assertRaisesRegex(ValueError, "does not advertise"):
            D.verify_publication(TAG)

    def test_superseded_beta_skips_gracefully_only_with_valid_stable_metadata(self):
        beta = "desktop-v0.1.0-beta.30"
        self.fetch.side_effect = [release(D.PUBLIC_REPOSITORY, beta), self.public]
        self.assertIsNone(D.verify_publication(beta))
        self.verify.assert_not_called()
        self.fetch.side_effect = [release(D.PUBLIC_REPOSITORY, beta), self.public | {"prerelease": True}]
        with self.assertRaises(ValueError):
            D.verify_publication(beta)

    def test_invalid_manifest_metadata_rejected(self):
        for update in ({"draft": True}, {"prerelease": True}, {"name": "Jcode CLI"}, {"assets": []}):
            self.json.return_value = self.live | update
            with self.assertRaises(ValueError):
                D.verify_publication(TAG)

    def test_invalid_asset_metadata_rejected(self):
        for update in ({"size": 0}, {"sha256": "bad"}, {"browser_download_url": "https://example.com/package"}):
            bad = copy.deepcopy(self.live)
            bad["assets"][0].update(update)
            self.json.return_value = bad
            with self.assertRaises(ValueError):
                D.verify_publication(TAG)

    def test_immutable_manifest_mismatch_rejected(self):
        self.json.side_effect = [self.live, self.live | {"name": "other"}]
        with self.assertRaisesRegex(ValueError, "manifests differ"):
            D.verify_publication(TAG)
        self.verify.assert_not_called()

    def test_missing_public_assets_rejected(self):
        self.public["assets"].pop()
        with self.assertRaisesRegex(ValueError, "incomplete"):
            D.verify_publication(TAG)
        self.verify.assert_not_called()

    def test_channel_change_during_verification_rejected(self):
        self.json.side_effect = [self.live, self.live, manifest("desktop-v0.2.1")]
        with self.assertRaisesRegex(ValueError, "changed during"):
            D.verify_publication(TAG)

    def test_bad_hash_rejected(self):
        self.verify.side_effect = ValueError("integrity")
        with self.assertRaises(ValueError):
            D.verify_publication(TAG)


class FormattingAndTransportTests(unittest.TestCase):
    def test_notes_limit_links_version_comments_and_mentions(self):
        for body in ("", "- " + "x" * 10000, "- @everyone <@123> <!-- secret -->\n- Fix terminals"):
            message = D.format_message(tag=TAG, body=body, url=release(D.PUBLIC_REPOSITORY)["html_url"])
            self.assertLessEqual(len(message), 2000)
            self.assertIn("Jcode Desktop 0.2.0", message)
            self.assertIn(D.WEBSITE, message)
            self.assertIn(release(D.PUBLIC_REPOSITORY)["html_url"], message)
            self.assertNotIn("secret", message)
            self.assertNotIn("@everyone", message)
            self.assertNotIn("<@123>", message)
        self.assertNotIn("old feature", D.release_notes_for_discord("Fix\n## Previous releases\nold feature"))

    def test_actual_changelog_shape_uses_current_feature_bullets_only(self):
        body = """## What's new

### Jcode Desktop 0.2.1

- Native document panels.
- Integrated terminal support.
  Includes clipboard and image scrollback.

Downloads are available at https://jcode.sh/desktop after all platform builds and public download checks pass.

## Previous update

- Older session tabs.

Development builds additionally report uncommitted files.
"""
        notes = D.release_notes_for_discord(body)
        self.assertEqual(notes, "- Native document panels.\n- Integrated terminal support.\n  Includes clipboard and image scrollback.")
        message = D.format_message(tag="desktop-v0.2.1", body=body,
                                   url=release(D.PUBLIC_REPOSITORY, "desktop-v0.2.1")["html_url"])
        self.assertEqual(message.count("Jcode Desktop 0.2.1"), 1)
        for unwanted in ("What's new", "Previous update", "Older session", "after all", "Development builds"):
            self.assertNotIn(unwanted, message)

    def test_repository_changelog_excludes_historical_and_pending_text(self):
        body = (ROOT / "CHANGELOG.md").read_text()
        notes = D.release_notes_for_discord(body)
        current, history = body.split("## Previous releases", 1)
        self.assertIn("### Jcode Desktop 0.3.0", current)
        self.assertIn("### Jcode Desktop 0.2.1", history)
        expected = [line for line in current.splitlines() if line.startswith("- ")]
        self.assertTrue(expected)
        self.assertEqual(notes.splitlines(), expected)
        for old_highlight in (line for line in history.splitlines() if line.startswith("- ")):
            self.assertNotIn(old_highlight, notes.splitlines())
        self.assertNotIn("0.2.1", notes)
        self.assertNotIn("Downloads are available", notes)
        self.assertNotIn("###", notes)

    def test_marker_preserves_body_exactly_and_is_idempotent(self):
        source = release() | {"body": "Notes  \n<!-- other -->\n"}
        with patch.object(D, "request_json") as request:
            D.mark_release_announced(repository=D.SOURCE_REPOSITORY, release=source, tag=TAG, token="token")
            body = request.call_args.kwargs["payload"]["body"]
            self.assertTrue(body.startswith(source["body"]))
            self.assertIn(D.announcement_marker(TAG), body)
            request.reset_mock()
            D.mark_release_announced(repository=D.SOURCE_REPOSITORY, release=source | {"body": body}, tag=TAG, token="token")
            request.assert_not_called()

    def test_discord_wait_and_no_mentions(self):
        with patch.object(D, "request_json", return_value={"id": "1"}) as request:
            D.post_to_discord(webhook_url=WEBHOOK + "?wait=false", content="@everyone")
        self.assertTrue(request.call_args.args[0].endswith("?wait=true"))
        self.assertEqual(request.call_args.kwargs["payload"]["allowed_mentions"], {"parse": []})

    def test_webhook_failure_never_exposes_secret(self):
        with patch.object(D, "request_json", side_effect=RuntimeError(WEBHOOK)):
            with self.assertRaises(RuntimeError) as error:
                D.post_to_discord(webhook_url=WEBHOOK, content="test")
        self.assertNotIn("SECRET", str(error.exception))
        self.assertTrue(error.exception.__suppress_context__)

    def test_discord_requires_message_id(self):
        with patch.object(D, "request_json", return_value={}):
            with self.assertRaises(RuntimeError):
                D.post_to_discord(webhook_url=WEBHOOK, content="test")

    def test_cli_logs_successful_tag_and_message_id_not_webhook(self):
        with patch("sys.argv", ["script", "--tag", TAG]), patch.dict(D.os.environ, {"GH_TOKEN": "token", "DISCORD_RELEASE_WEBHOOK": WEBHOOK}), patch.object(D, "announce_release", return_value="1234567890"), patch("sys.stdout", new_callable=io.StringIO) as stdout:
            self.assertEqual(D.main(), 0)
            self.assertIn(f"Posted {TAG} to Discord as message 1234567890", stdout.getvalue())
            self.assertNotIn("SECRET", stdout.getvalue())

    def test_cli_does_not_log_untrusted_message_id_or_claim_skipped_post(self):
        for message_id in (None, WEBHOOK):
            with patch("sys.argv", ["script", "--tag", TAG]), patch.dict(D.os.environ, {"GH_TOKEN": "token", "DISCORD_RELEASE_WEBHOOK": WEBHOOK}), patch.object(D, "announce_release", return_value=message_id), patch("sys.stdout", new_callable=io.StringIO) as stdout:
                self.assertEqual(D.main(), 0)
                self.assertNotIn("SECRET", stdout.getvalue())
                self.assertNotIn("Posted", stdout.getvalue())

    def test_cli_failure_never_logs_exception_url(self):
        with patch("sys.argv", ["script", "--tag", TAG]), patch.dict(D.os.environ, {"GH_TOKEN": "token", "DISCORD_RELEASE_WEBHOOK": WEBHOOK}), patch.object(D, "announce_release", side_effect=RuntimeError(WEBHOOK)), patch("sys.stderr", new_callable=io.StringIO) as stderr:
            self.assertEqual(D.main(), 1)
            self.assertNotIn("SECRET", stderr.getvalue())

    def test_asset_verifier_hash_and_size(self):
        asset = manifest()["assets"][0]
        with patch.object(D.urllib.request, "urlopen", return_value=io.BytesIO(b"abc")):
            D.verify_website_asset(asset)
        for bad in (asset | {"size": 4}, asset | {"sha256": "0" * 64}):
            with patch.object(D.urllib.request, "urlopen", return_value=io.BytesIO(b"abc")):
                with self.assertRaises(ValueError):
                    D.verify_website_asset(bad)


class WorkflowContractTests(unittest.TestCase):
    def test_standalone_manual_only_source_write_and_per_tag_serialization(self):
        text = (ROOT / ".github/workflows/discord-release.yml").read_text()
        self.assertIn("  workflow_dispatch:", text)
        self.assertNotRegex(text, r"(?m)^  (release|push|workflow_run):")
        for required in ("contents: write", "group: desktop-discord-release-${{ inputs.tag }}", "cancel-in-progress: false", "ref: main", "persist-credentials: false", "${{ secrets.DISCORD_RELEASE_WEBHOOK }}", "${{ github.token }}", '--tag "$RELEASE_TAG"', "if: failure()", "::warning::"):
            self.assertIn(required, text)

    def test_publisher_dispatch_after_website_verification_and_nonblocking(self):
        text = (ROOT / ".github/workflows/publish-public-release.yml").read_text()
        self.assertLess(text.index("digest.hexdigest()"), text.index("gh workflow run discord-release.yml"))
        self.assertIn("id: website_verification", text)
        self.assertIn("steps.website_verification.outcome == 'success'", text)
        discord = text[text.index("      - name: Dispatch verified public Desktop Discord announcement"):]
        self.assertIn("continue-on-error: true", discord)
        self.assertIn("--ref main", discord)
        self.assertIn('tag="$RELEASE_TAG"', discord)
        self.assertIn("steps.discord_dispatch.outcome == 'failure'", discord)
        self.assertIn("::warning::", discord)
        self.assertIn("macos-public-release-verify.yml", text)


if __name__ == "__main__":
    unittest.main()
