"""Execute the release workflow shell gates offline with a stubbed GitHub CLI."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def workflow_step(workflow, name):
    text = (ROOT / ".github/workflows" / workflow).read_text()
    return re.search(rf"^      - name: {re.escape(name)}\n(.*?)(?=^      - |\Z)",
                     text, re.M | re.S)[1]


def script(step):
    return "\n".join(line[10:] for line in step.split("        run: |\n", 1)[1].splitlines())


@unittest.skipUnless(shutil.which("bash") and shutil.which("jq"), "bash and jq required for workflow shell tests")
class StableWorkflowTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.log = self.directory / "calls"
        self.log.touch()
        self.env = {**os.environ, "CALLS": str(self.log), "GITHUB_REPOSITORY": "example/private",
                    "GITHUB_SHA": "a" * 40, "GITHUB_REF_NAME": "desktop-v0.2.0"}
        self.mac = script(workflow_step("macos-beta.yml", "Publish tagged desktop release"))
        self.cross = script(workflow_step("cross-platform-release.yml", "Wait for secure macOS workflow and upload to its release"))

    def run_script(self, stub, body, **env):
        self.log.write_text("")
        result = subprocess.run(["bash", "-e", "-c", stub + "\n" + body], cwd=self.directory,
                                env={**self.env, **env}, capture_output=True, text=True, timeout=20)
        return result, self.log.read_text().splitlines()

    def test_macos_publishes_correct_metadata_only_after_asset_upload(self):
        dist = self.directory / "jcode-desktop/dist/macos"
        dist.mkdir(parents=True)
        for name in ("Jcode-macOS-universal.zip", "Jcode-macOS-universal.dmg", "appcast.xml"):
            (dist / name).write_text("fixture")
        stub = r'''
shasum() { echo "fixture checksum"; }
gh() {
  printf '%s\n' "$*" >> "$CALLS"
  if [[ "$1 $2" == "release view" ]]; then
    [[ "$3" == "$GITHUB_REF_NAME" && "$EXISTS" == true ]]
  elif [[ "$1 $2" == "release upload" && "$3" == "$GITHUB_REF_NAME" ]]; then
    [[ "$FAIL_UPLOAD" == false ]]
  fi
}
'''
        for tag, flag in [("desktop-v0.2.0", "false"), ("desktop-v0.2.1-beta.1", "true")]:
            for exists, failure in [("false", "false"), ("true", "false"), ("false", "true")]:
                with self.subTest(tag=tag, exists=exists, failure=failure):
                    result, calls = self.run_script(stub, self.mac, GITHUB_REF_NAME=tag, EXISTS=exists, FAIL_UPLOAD=failure)
                    self.assertEqual(result.returncode == 0, failure == "false", result.stderr)
                    creates = [c for c in calls if c.startswith(f"release create {tag} ")]
                    self.assertEqual(len(creates), int(exists == "false"))
                    if creates:
                        self.assertIn(f"--draft --prerelease={flag}", creates[0])
                    edits = [c for c in calls if c.startswith(f"release edit {tag} ")]
                    self.assertEqual(len(edits), int(failure == "false"))
                    if edits:
                        self.assertIn(f"--draft=false --prerelease={flag}", edits[0])
                        upload = next(c for c in calls if c.startswith(f"release upload {tag} "))
                        self.assertLess(calls.index(upload), calls.index(edits[0]))
                        channel = next(c for c in calls if c.startswith("release create desktop-updates "))
                        self.assertIn("--prerelease", channel)

    def test_cross_platform_wait_accepts_only_published_matching_metadata(self):
        artifacts = self.directory / "artifacts"
        artifacts.mkdir()
        (artifacts / "fixture.zip").write_text("fixture")
        stub = r'''
sleep() { :; }
gh() {
  printf '%s\n' "$*" >> "$CALLS"
  if [[ "$1 $2" == "release view" ]]; then
    while [[ "$1" != --jq ]]; do shift; done
    printf '%s\n' "$METADATA" | jq -r "$2"
    return "$VIEW_STATUS"
  fi
}
'''
        for tag, prerelease in [("desktop-v0.2.0", False), ("desktop-v0.2.1-beta.1", True)]:
            for metadata, view_status, allowed in [
                ({"isDraft": False, "isPrerelease": prerelease}, "0", True),
                ({"isDraft": True, "isPrerelease": prerelease}, "0", False),
                ({"isDraft": False, "isPrerelease": not prerelease}, "0", False),
                ({}, "0", False),
                ({"isDraft": False, "isPrerelease": prerelease}, "1", False),
            ]:
                with self.subTest(tag=tag, metadata=metadata, view_status=view_status):
                    result, calls = self.run_script(stub, self.cross, GITHUB_REF_NAME=tag,
                                                    METADATA=json.dumps(metadata), VIEW_STATUS=view_status)
                    self.assertEqual(result.returncode == 0, allowed, result.stderr)
                    self.assertEqual(any(c.startswith("release upload ") for c in calls), allowed)

    def test_invalid_tag_never_reaches_github(self):
        stub = 'gh() { echo "$*" >> "$CALLS"; }'
        for tag in ("desktop-v0.2.0-rc.1", "desktop-v0.2.0;false", "main"):
            for body in (self.mac, self.cross):
                with self.subTest(tag=tag, body=body[:50]):
                    result, calls = self.run_script(stub, body, GITHUB_REF_NAME=tag)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(calls, [])

    def test_packaging_and_security_gates_still_precede_publication(self):
        mac = (ROOT / ".github/workflows/macos-beta.yml").read_text()
        for gate in ("Validate release credentials", "Build universal app, DMG, and ZIP",
                     "Verify app bundle and DMG install path", "Sign automatic update and generate appcast"):
            self.assertLess(mac.index(gate), mac.index("Publish tagged desktop release"))
        self.assertIn("Tagged macOS releases require Developer ID signing and notarization credentials", mac)
        self.assertIn("Tagged macOS releases require Sparkle public and private keys", mac)
        cross = (ROOT / ".github/workflows/cross-platform-release.yml").read_text()
        self.assertIn("needs: [build, build-freebsd]", cross)
        self.assertIn("if: startsWith(github.ref, 'refs/tags/desktop-v')", cross)
        for gate in ("Verify Linux package", "Smoke-test Linux package on X11",
                     "Smoke-test Linux package on native Wayland", "Verify Windows package", "Smoke-test Windows package"):
            self.assertLess(cross.index(gate), cross.index("  publish:"))


if __name__ == "__main__":
    unittest.main()
