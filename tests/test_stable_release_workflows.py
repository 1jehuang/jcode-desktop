"""Execute tagged release transport and publication gates offline with a stub gh."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = ("cross-platform-release.yml", "macos-beta.yml", "freebsd-release.yml")
UPLOADS = dict(zip(WORKFLOWS, (
    "Upload verified platform packages to private draft",
    "Upload verified macOS packages to private draft",
    "Upload verified FreeBSD package to private draft",
)))


def workflow_step(workflow, name):
    text = (ROOT / ".github/workflows" / workflow).read_text()
    return re.search(rf"^      - name: {re.escape(name)}\n(.*?)(?=^      - |^  [\w-]+:|\Z)",
                     text, re.M | re.S)[1]


def script(step):
    # Exclude workflow-level comments separating steps.
    return "\n".join(line[10:] for line in step.split("        run: |\n", 1)[1].splitlines()
                     if line.startswith("          ") or not line.strip())


STUB = r'''
sleep() { :; }
gh() {
  printf '%s\n' "$*" >> "$CALLS"
  case "$1 $2" in
    "api "*) printf '%s\n' "$PRIVATE"; return "$API_STATUS" ;;
    "release view")
      [[ "$3" != desktop-updates ]] || return 1
      [[ -f release-exists ]] || return 1
      if [[ "$*" == *--jq* ]]; then
        while [[ "$1" != --jq ]]; do shift; done
        printf '%s\n' "$METADATA" | jq -r "$2"
        return "$VIEW_STATUS"
      fi ;;
    "release create")
      if [[ "$CREATE_RESULT_EXISTS" == true ]]; then touch release-exists; fi
      return "$CREATE_STATUS" ;;
    "release upload") return "$UPLOAD_STATUS" ;;
    "release download") return "$DOWNLOAD_STATUS" ;;
    "run list")
      while [[ "$1" != --jq ]]; do shift; done
      printf '%s\n' "$RUNS" | jq -r "$2"
      return "$RUN_STATUS" ;;
  esac
}
'''


@unittest.skipUnless(shutil.which("bash") and shutil.which("jq"), "bash and jq required for workflow shell tests")
class StableWorkflowTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.log = self.directory / "calls"
        self.exists = self.directory / "release-exists"
        self.exists.touch()
        self.env = {**os.environ, "CALLS": str(self.log), "GITHUB_REPOSITORY": "example/private",
                    "GITHUB_SHA": "a" * 40, "GITHUB_REF_NAME": "desktop-v0.2.0",
                    "PRIVATE": "false", "API_STATUS": "0", "VIEW_STATUS": "0", "CREATE_STATUS": "0",
                    "UPLOAD_STATUS": "0", "DOWNLOAD_STATUS": "0", "RUN_STATUS": "0",
                    "CREATE_RESULT_EXISTS": "true", "CHECKSUM_STATUS": "0"}
        self.prepare = {w: script(workflow_step(w, "Prepare private draft release")) for w in WORKFLOWS}
        self.uploads = {w: script(workflow_step(w, UPLOADS[w])) for w in WORKFLOWS}
        self.wait = script(workflow_step(WORKFLOWS[0], "Wait for secure macOS workflow"))
        self.publish = script(workflow_step(WORKFLOWS[0], "Validate complete private draft and publish"))

    def metadata(self, tag="desktop-v0.2.0"):
        return json.dumps(dict(tagName=tag, isDraft=True, isPrerelease="-beta." in tag))

    def run_script(self, body, stub=STUB, **env):
        self.log.write_text("")
        result = subprocess.run(["bash", "-e", "-c", stub + "\n" + body], cwd=self.directory,
                                env={**self.env, "METADATA": self.metadata(), **env},
                                capture_output=True, text=True, timeout=20)
        return result, self.log.read_text().splitlines()

    def test_prepare_is_idempotent_race_safe_and_preserves_stable_metadata(self):
        for workflow, body in self.prepare.items():
            for tag, flag in [("desktop-v0.2.0", "false"), ("desktop-v0.2.1-beta.1", "true")]:
                for exists, create_status in [(True, "0"), (False, "0"), (False, "1")]:
                    with self.subTest(workflow=workflow, tag=tag, exists=exists, race=create_status):
                        self.exists.unlink(missing_ok=True)
                        if exists:
                            self.exists.touch()
                        result, calls = self.run_script(body, GITHUB_REF_NAME=tag, METADATA=self.metadata(tag),
                                                        CREATE_STATUS=create_status)
                        self.assertEqual(result.returncode, 0, result.stderr)
                        creates = [c for c in calls if c.startswith("release create ")]
                        self.assertEqual(len(creates), int(not exists))
                        if creates:
                            self.assertIn(f"--verify-tag --draft --prerelease={flag}", creates[0])
                        self.assertFalse(any(c.startswith("release edit ") for c in calls))

    def test_failed_create_without_concurrent_draft_fails_closed(self):
        self.exists.unlink()
        for workflow, body in self.prepare.items():
            with self.subTest(workflow=workflow):
                result, calls = self.run_script(body, CREATE_STATUS="1", CREATE_RESULT_EXISTS="false")
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(c.startswith(("release edit ", "release upload ")) for c in calls))

    def test_all_mutations_reject_published_or_wrong_release_state(self):
        for body in [*self.prepare.values(), *self.uploads.values(), self.publish]:
            for change in ({"VIEW_STATUS": "1"},
                           {"METADATA": self.metadata().replace('"isDraft": true', '"isDraft": false')},
                           {"METADATA": self.metadata().replace('"isPrerelease": false', '"isPrerelease": true')},
                           {"METADATA": self.metadata("desktop-v0.3.0")}, {"METADATA": "{}"}):
                with self.subTest(body=body[:100], change=change):
                    result, calls = self.run_script(body, **change)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(any(c.startswith(("release create ", "release upload ", "release edit "))
                                         for c in calls), calls)

    def test_invalid_tag_never_reaches_github(self):
        for tag in ("desktop-v0.2.0-rc.1", "desktop-v0.2.0;false", "main"):
            for body in [*self.prepare.values(), *self.uploads.values(), self.wait, self.publish]:
                with self.subTest(tag=tag, body=body[:50]):
                    result, calls = self.run_script(body, GITHUB_REF_NAME=tag)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(calls, [])

    def test_uploads_require_real_files_and_propagate_transport_failures_without_promoting(self):
        # Run the Linux, Windows (Git Bash), macOS and FreeBSD host upload bodies.
        cases = [
            (WORKFLOWS[0], "linux", "SHA256SUMS-linux", ["Jcode-fixture.tar.gz", "Jcode-fixture.deb"]),
            (WORKFLOWS[0], "windows", "SHA256SUMS-windows", ["Jcode-fixture.zip"]),
            (WORKFLOWS[1], "macos", "SHA256SUMS", ["Jcode-0.2.0-macOS-universal.zip", "Jcode-macOS-universal.dmg", "appcast.xml"]),
            (WORKFLOWS[2], "freebsd", "SHA256SUMS-freebsd-x86_64", ["Jcode-fixture.tar.gz"]),
        ]
        for workflow, platform, checksum, names in cases:
            dist = self.directory / "jcode-desktop/dist" / platform
            dist.mkdir(parents=True)
            for name in [*names, checksum]:
                (dist / name).write_text("fixture")
            # Checksums are validated by the real full-release validator below.
            stub = STUB + '\nshasum() { return "$CHECKSUM_STATUS"; }\nsha256sum() { return "$CHECKSUM_STATUS"; }\n'
            for failure in ("0", "1"):
                with self.subTest(platform=platform, failure=failure):
                    result, calls = self.run_script(self.uploads[workflow], stub=stub, PLATFORM=platform,
                                                    CHECKSUMS=checksum, UPLOAD_STATUS=failure)
                    self.assertEqual(result.returncode == 0, failure == "0", result.stderr)
                    uploads = [c for c in calls if c.startswith("release upload ")]
                    self.assertEqual(len(uploads), 1)
                    self.assertIn(checksum, uploads[0])
                    self.assertTrue(all(name in uploads[0] for name in names))
                    self.assertFalse(any(c.startswith("release edit ") for c in calls))
            if platform in ("macos", "freebsd"):
                result, calls = self.run_script(self.uploads[workflow], stub=stub, CHECKSUM_STATUS="1")
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(c.startswith("release upload ") for c in calls))
            (dist / names[0]).unlink()
            result, calls = self.run_script(self.uploads[workflow], stub=stub, PLATFORM=platform, CHECKSUMS=checksum)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(any(c.startswith("release upload ") for c in calls))

    def test_wait_requires_latest_matching_macos_run_success_not_assets_alone(self):
        success = dict(status="completed", conclusion="success", event="push",
                       headSha=self.env["GITHUB_SHA"], headBranch=self.env["GITHUB_REF_NAME"])
        cases = [([success], True), ([{**success, "event": "workflow_dispatch"}], True), ([], False)]
        for change in ({"conclusion": "failure"}, {"conclusion": "cancelled"}, {"conclusion": "skipped"},
                       {"conclusion": "timed_out"}, {"status": "in_progress"}, {"headSha": "b" * 40},
                       {"headBranch": "main"}, {"event": "pull_request"}):
            cases.append(([{**success, **change}], False))
        cases.append(([{**success, "conclusion": "failure"}, success], False))
        for runs, allowed in cases:
            with self.subTest(runs=runs):
                result, calls = self.run_script(self.wait, RUNS=json.dumps(runs))
                self.assertEqual(result.returncode == 0, allowed, result.stderr)
                self.assertTrue(any("--workflow macos-beta.yml" in c and "--commit " in c for c in calls))
                self.assertFalse(any(c.startswith("release ") for c in calls))
        result, _ = self.run_script(self.wait, RUNS=json.dumps([success]), RUN_STATUS="1")
        self.assertNotEqual(result.returncode, 0)

    def test_complete_draft_validation_is_required_before_stable_or_beta_promotion(self):
        import test_prepare_public_release as fixtures
        scripts = self.directory / "jcode-desktop/scripts"
        scripts.mkdir(parents=True)
        shutil.copy(ROOT / "scripts/prepare-public-release.py", scripts)
        for tag in ("desktop-v0.2.0", "desktop-v0.2.1-beta.1"):
            fixture = fixtures.PublicReleaseTests()
            fixture.tag = tag
            fixture.setUp()
            self.addCleanup(fixture.doCleanups)
            artifacts = self.directory / "artifacts"
            shutil.copytree(fixture.directory, artifacts, dirs_exist_ok=True)
            for failure in (None, "missing", "checksum", "appcast", "download"):
                with self.subTest(tag=tag, failure=failure):
                    shutil.rmtree(artifacts)
                    shutil.copytree(fixture.directory, artifacts)
                    if failure == "missing":
                        (artifacts / "SHA256SUMS-freebsd-x86_64").unlink()
                    elif failure == "checksum":
                        (artifacts / fixture.archive).write_text("corrupt")
                    elif failure == "appcast":
                        (artifacts / "appcast.xml").write_text("unsigned")
                    result, calls = self.run_script(self.publish, GITHUB_REF_NAME=tag, METADATA=self.metadata(tag),
                                                    DOWNLOAD_STATUS="1" if failure == "download" else "0")
                    self.assertEqual(result.returncode == 0, failure is None, result.stderr)
                    edits = [c for c in calls if c.startswith("release edit ")]
                    self.assertEqual(len(edits), int(failure is None))
                    if edits:
                        self.assertIn(f"--draft=false --prerelease={str('-beta.' in tag).lower()}", edits[0])
                        self.assertTrue((artifacts / "latest.json").is_file())
                        self.assertTrue(calls[-1].startswith("release edit "))
                    else:
                        self.assertFalse(any(c.startswith("release upload desktop-updates ") for c in calls))


class WorkflowTransportContractTests(unittest.TestCase):
    def test_gates_and_transport_order(self):
        cross, mac, freebsd = [(ROOT / ".github/workflows" / w).read_text() for w in WORKFLOWS]
        self.assertIn("name: Linux and Windows desktop release\n", cross)
        self.assertIn("name: macOS beta\n", mac)
        self.assertIn("  actions: read\n", cross)
        self.assertIn("  contents: write\n", freebsd)
        self.assertIn("  build:\n    needs: prepare\n", cross)
        self.assertIn("  build-freebsd:\n    needs: prepare\n", cross)
        self.assertIn("needs: [build, build-freebsd]", cross)
        self.assertNotIn("actions/download-artifact", cross)
        self.assertNotIn("--draft=false", mac + freebsd)
        self.assertIn("fail-fast: false", cross)
        for workflow, text, gates in [
            (WORKFLOWS[0], cross, ("Verify Linux package", "Smoke-test Linux package on X11",
                                  "Smoke-test Linux package on native Wayland", "Verify Windows package", "Smoke-test Windows package")),
            (WORKFLOWS[1], mac, ("Validate release credentials", "Build universal app, DMG, and ZIP",
                                "Verify app bundle and DMG install path", "Sign automatic update and generate appcast")),
            (WORKFLOWS[2], freebsd, ("Build, package, and smoke-test inside FreeBSD", "FreeBSD native X11 window smoke passed")),
        ]:
            for gate in gates:
                self.assertLess(text.index(gate), text.index(UPLOADS[workflow]))
                if gate != "FreeBSD native X11 window smoke passed":
                    native_gate = workflow_step(workflow, gate)
                    self.assertNotIn("continue-on-error:", native_gate)
                    self.assertNotIn("if: always()", native_gate)
            upload = workflow_step(workflow, UPLOADS[workflow])
            self.assertIn("if: startsWith(github.ref, 'refs/tags/desktop-v')", upload)
            self.assertNotIn("always()", upload)
            self.assertNotIn("continue-on-error:", upload)
            # Tagged dispatches cannot package an unrelated default input version.
            self.assertIn("startsWith(github.ref, 'refs/tags/desktop-v') && github.ref_name || inputs.version", text)
        self.assertIn("Tagged macOS releases require Developer ID signing and notarization credentials", mac)
        self.assertIn("Tagged macOS releases require Sparkle public and private keys", mac)
        self.assertIn('FIRST_LAUNCH_CHECK: "1"', mac)
        publish = cross.split("\n  publish:\n", 1)[1]
        self.assertNotIn("always()", publish)
        self.assertNotIn("continue-on-error:", publish)
        self.assertLess(publish.index("Wait for secure macOS workflow"), publish.index("Validate complete private draft and publish"))

    def test_artifact_quota_cannot_block_tag_transport_but_branch_artifacts_remain(self):
        for workflow in WORKFLOWS:
            text = (ROOT / ".github/workflows" / workflow).read_text()
            artifacts = [s for s in re.split(r"^      - ", text, flags=re.M) if "uses: actions/upload-artifact@" in s]
            # Draft releases are private even when the build repository is public.
            self.assertNotIn("--jq .private", text)
            self.assertTrue(artifacts)
            for step in artifacts:
                if "smoke-diagnostics" in step:
                    self.assertIn("continue-on-error: true", step)
                elif workflow == "macos-beta.yml":
                    self.assertIn("continue-on-error: ${{ startsWith(github.ref, 'refs/tags/desktop-v') }}", step)
                    self.assertLess(text.index(UPLOADS[workflow]), text.index("Upload workflow artifact"))
                else:
                    self.assertIn("!startsWith(github.ref, 'refs/tags/desktop-v')", step)
                    self.assertNotIn("continue-on-error:", step)


if __name__ == "__main__":
    unittest.main()
