"""Offline release orchestration policy and side-effect boundary tests."""
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release_desktop", ROOT / "scripts/release-desktop.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
TAG = "desktop-v0.3.0"
SHA = "a" * 40


def run(id=1, **kwargs):
    return dict(id=id, event="workflow_dispatch", head_branch=TAG, head_sha=SHA,
                status="completed", conclusion="success", **kwargs)


class OrchestratorTests(unittest.TestCase):
    def test_explicit_versions(self):
        self.assertEqual(release.tag_for("0.3.0"), TAG)
        self.assertEqual(release.tag_for("0.3.0-beta.1"), TAG + "-beta.1")
        for version in ("../main", "0.3.0;echo hi", "v0.3.0", "0.3.0-beta.1\n"):
            with self.assertRaises(ValueError):
                release.tag_for(version)

    def test_exact_sha_tag_and_latest_attempt(self):
        wrong = run(3)
        wrong["head_sha"] = "b" * 40
        old = run(1)
        newer = run(2)
        newer["conclusion"] = "failure"
        match = release.matching_run([wrong, old, newer], TAG, SHA)
        self.assertEqual(match, newer)
        with self.assertRaisesRegex(RuntimeError, "Release gate failed"):
            release.run_state(match)

    def test_downstream_exact_tag_identity(self):
        good = run(1, display_title=f"Accept {TAG}")
        wrong = run(2, display_title=f"Accept {TAG}-beta.1")
        self.assertEqual(release.matching_run([good, wrong], TAG, SHA, "Accept"), good)
        self.assertIsNone(release.matching_run([wrong], TAG, SHA, "Accept"))

    def test_every_non_success_is_fatal(self):
        for conclusion in ("failure", "cancelled", "skipped", "timed_out", "neutral"):
            item = run()
            item["conclusion"] = conclusion
            with self.assertRaises(RuntimeError):
                release.run_state(item)

    def test_resolve_annotated_tag_and_ignore_prefixes(self):
        gh = Mock()
        gh.api.side_effect = [[{"ref": "refs/tags/" + TAG, "object": {"type": "tag", "sha": "obj"}},
                               {"ref": "refs/tags/" + TAG + "-beta.1", "object": {"type": "commit", "sha": "bad"}}],
                              {"object": {"type": "commit", "sha": SHA}}]
        self.assertEqual(release.resolve_tag(gh, TAG), SHA)
        gh.api.side_effect = [[{"ref": "refs/tags/" + TAG + "-beta.1"}]]
        self.assertIsNone(release.resolve_tag(gh, TAG))

    def test_read_failure_is_not_missing_tag(self):
        gh = Mock()
        gh.api.side_effect = RuntimeError("network")
        with self.assertRaisesRegex(RuntimeError, "network"):
            release.resolve_tag(gh, TAG)

    def test_monitor_requires_all_five_gates_without_dispatching(self):
        gh = Mock()
        def reads(gh, workflow, tag=None):
            return [run(display_title=f"{release.DOWNSTREAM.get(workflow, '')} {TAG}")]
        with patch.object(release, "read_runs", side_effect=reads):
            release.monitor(gh, TAG, SHA, 10, 1)
        gh.dispatch.assert_not_called()

    def test_missing_dispatch_once_even_when_api_visibility_lags(self):
        gh = Mock()
        with patch.object(release, "read_runs", return_value=[]), patch.object(release.time, "sleep"), \
                patch.object(release.time, "monotonic", side_effect=[0, 0, 0, 0, 0, 1, 20]):
            with self.assertRaises(TimeoutError):
                release.monitor(gh, TAG, SHA, 10, 1, grace=0)
        self.assertEqual(gh.dispatch.call_count, len(release.BUILD))

    def test_failed_build_never_dispatches_or_publishes(self):
        failed = run()
        failed["conclusion"] = "failure"
        gh = Mock()
        with patch.object(release, "read_runs", return_value=[failed]):
            with self.assertRaises(RuntimeError):
                release.monitor(gh, TAG, SHA, 10, 1)
        gh.dispatch.assert_not_called()

    def test_dry_run_does_not_test_tag_dispatch_or_monitor(self):
        gh = Mock()
        gh.api.return_value = {"sha": SHA}
        gh.active_runs.return_value = []
        with patch.object(release.policy, "GitHub", return_value=gh), \
                patch.object(release, "resolve_tag", return_value=None), \
                patch.object(release.policy, "git", return_value=SHA), \
                patch.object(release.policy, "validate_runtime_pins"), \
                patch.object(release, "validate_versions"), \
                patch.object(release, "monitor") as monitor, \
                patch.object(release.subprocess, "run") as command:
            self.assertEqual(release.main(["0.3.0"]), 0)
        gh.api.assert_called_once_with("commits/main")
        command.assert_not_called()
        monitor.assert_not_called()
        gh.dispatch.assert_not_called()

    def test_stale_main_ref_rejected(self):
        gh = Mock()
        gh.api.return_value = {"sha": "b" * 40}
        with patch.object(release.policy, "GitHub", return_value=gh), \
                patch.object(release, "resolve_tag", return_value=None), \
                patch.object(release.policy, "git", return_value=SHA), \
                patch.object(release.policy, "validate_runtime_pins"), \
                patch.object(release, "validate_versions"):
            with self.assertRaisesRegex(ValueError, "current remote main"):
                release.main(["0.3.0", "--apply"])
        gh.dispatch.assert_not_called()

    def test_downstream_workflows_have_correlatable_run_names(self):
        for workflow, title in release.DOWNSTREAM.items():
            source = (ROOT / ".github/workflows" / workflow).read_text()
            self.assertIn(f"run-name: {title} ${{{{ inputs.tag", source)

    @unittest.skipIf(os.name == "nt", "Orchestrator runs on Linux/macOS")
    def test_dispatch_journal_survives_resume_and_locks_concurrent_call(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(release.policy, "git", return_value=directory):
            with release.dispatch_journal("owner/repo", TAG, SHA) as (state, record):
                self.assertEqual(state, set())
                record(release.BUILD[0])
                with self.assertRaisesRegex(RuntimeError, "Another local"):
                    with release.dispatch_journal("owner/repo", TAG, SHA):
                        self.fail("Concurrent owner acquired release lock")
            with release.dispatch_journal("owner/repo", TAG, SHA) as (state, record):
                self.assertEqual(state, {release.BUILD[0]})

    def test_existing_tag_resume_never_creates_or_moves_tag(self):
        gh = Mock()
        from contextlib import nullcontext
        with patch.object(release.policy, "GitHub", return_value=gh), \
                patch.object(release, "resolve_tag", return_value=SHA), \
                patch.object(release.policy, "validate_runtime_pins"), \
                patch.object(release, "verify_website"), \
                patch.object(release, "dispatch_journal", return_value=nullcontext((set(), Mock()))), \
                patch.object(release, "monitor") as monitor:
            self.assertEqual(release.main(["0.3.0", "--apply"]), 0)
        gh.api.assert_not_called()
        gh.active_runs.assert_not_called()
        self.assertEqual(monitor.call_args.args[1:3], (TAG, SHA))

    def test_source_versions_must_all_match(self):
        for wrong_index in range(4):
            manifests = ['[package]\nversion="0.3.0"\n'] * 4
            manifests[wrong_index] = '[package]\nversion="0.2.1"\n'
            with patch.object(release.policy, "git", side_effect=manifests):
                with self.assertRaisesRegex(ValueError, "does not match requested"):
                    release.validate_versions(SHA, "0.3.0")
        with patch.object(release.policy, "git", return_value='[package]\nversion="0.3.0"\n') as git:
            release.validate_versions(SHA, "0.3.0")
        self.assertEqual(git.call_count, 4)

    def website_manifest(self):
        groups = release.prepare.expected_assets(TAG)
        names = set(groups) | {"appcast.xml"}
        names.update(name for group in groups.values() for name in group)
        return dict(tag_name=TAG, draft=False, prerelease=False, assets=[
            dict(name=name, size=1, sha256="a" * 64,
                 browser_download_url=f"{release.prepare.PUBLIC_BASE}/{TAG}/{name}")
            for name in sorted(names)])

    def verify_manifest(self, manifest):
        with patch.object(release.urllib.request, "urlopen", return_value=io.BytesIO(json.dumps(manifest).encode())) as get:
            release.verify_website(TAG)
        self.assertEqual(get.call_args.args[0].full_url, "https://jcode.sh/desktop/latest.json")

    def test_current_stable_website_channel(self):
        self.verify_manifest(self.website_manifest())

    def test_stale_draft_beta_channel_rejected(self):
        for key, value in (("tag_name", "desktop-v0.2.1"), ("draft", True), ("prerelease", True)):
            manifest = self.website_manifest()
            manifest[key] = value
            with self.assertRaisesRegex(ValueError, "has not promoted"):
                self.verify_manifest(manifest)

    def test_missing_duplicate_wrong_url_hash_or_size_rejected(self):
        for mutation in (lambda m: m["assets"].pop(),
                         lambda m: m["assets"].append(m["assets"][0]),
                         lambda m: m["assets"][0].update(size=0),
                         lambda m: m["assets"][0].update(sha256="bad"),
                         lambda m: m["assets"][0].update(browser_download_url="https://wrong.invalid/file")):
            manifest = self.website_manifest()
            mutation(manifest)
            with self.assertRaises(ValueError):
                self.verify_manifest(manifest)

    def test_old_acceptance_not_reused_after_new_publication(self):
        old = run(display_title=f"Accept {TAG}", run_started_at="2026-09-20T10:00:00Z")
        self.assertIsNone(release.matching_run([old], TAG, SHA, "Accept", "2026-09-20T11:00:00Z"))


if __name__ == "__main__":
    unittest.main()
