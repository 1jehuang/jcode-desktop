#!/usr/bin/env python3
"""Real Git commit/push regression tests, isolated from the working checkout."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent


class IdentityGuardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("JCODE_SCRATCH_DIR"))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Test User")
        self.git("config", "user.email", "user@example.org")
        self.git("config", "core.hooksPath", ".githooks")
        shutil.copytree(ROOT / ".githooks", self.repo / ".githooks")
        (self.repo / "scripts").mkdir()
        shutil.copy(ROOT / "scripts/git_identity_guard.py", self.repo / "scripts")
        self.git("commit", "--allow-empty", "-m", "Initial commit")
        self.remote = self.root / "remote.git"
        self.git("init", "--bare", str(self.remote))
        self.git("remote", "add", "origin", str(self.remote))
        self.git("push", "origin", "main")

    def git(self, *args, ok=True, extra_env=None, input=None):
        result = subprocess.run(
            ["git", *args], cwd=self.repo,
            env={**self.env, **(extra_env or {})}, input=input,
            text=True, capture_output=True,
        )
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn("Git identity guard:", result.stderr)
        return result.stdout.strip()

    def test_normal_commit_and_push(self):
        self.git("commit", "--allow-empty", "-m", "Normal")
        self.git("push", "origin", "main")

    def test_command_config_override(self):
        before = self.git("rev-parse", "HEAD")
        self.git("-c", "user.name=Jcode", "-c", "user.email=jcode@users.noreply.github.com",
                 "commit", "--allow-empty", "-m", "Bad", ok=False)
        self.assertEqual(self.git("rev-parse", "HEAD"), before)

    def test_environment_overrides(self):
        for field, value in [("GIT_AUTHOR_NAME", "Jcode"),
                             ("GIT_AUTHOR_EMAIL", "jcode@localhost"),
                             ("GIT_COMMITTER_NAME", "JCODE"),
                             ("GIT_COMMITTER_EMAIL", "jcode@users.noreply.github.com")]:
            with self.subTest(field=field):
                self.git("commit", "--allow-empty", "-m", "Bad", ok=False,
                         extra_env={field: value})

    def test_explicit_author_and_no_verify(self):
        self.git("commit", "--allow-empty", "--no-verify", "--author=Jcode <jcode@localhost>",
                 "-m", "Bad", ok=False)

    def test_contributor_is_not_impersonated(self):
        self.git("commit", "--allow-empty", "--author=Contributor <contributor@example.org>",
                 "-m", "Contribution")
        self.git("push", "origin", "main")
        self.assertEqual(self.git("show", "-s", "--format=%an <%ae>"),
                         "Contributor <contributor@example.org>")

    def plumbing_commit(self, extra_env):
        return self.git("commit-tree", self.git("rev-parse", "HEAD^{tree}"),
                        "-p", "HEAD", input="Plumbing commit\n", extra_env=extra_env)

    def test_push_rejects_plumbing_author_and_committer(self):
        for field in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"]:
            with self.subTest(field=field):
                oid = self.plumbing_commit({field: "jcode@localhost"})
                self.git("push", "origin", f"{oid}:refs/heads/main", ok=False)
                self.git("push", "origin", f"{oid}:refs/heads/new-branch", ok=False)
                self.git("tag", "-f", "-a", "bad-tag", oid, "-m", "Bad")
                self.git("push", "origin", "refs/tags/bad-tag", ok=False)

    def test_new_ref_and_deletion(self):
        self.git("push", "origin", "HEAD:refs/heads/temporary")
        self.git("push", "origin", ":refs/heads/temporary")

    def test_identity_only_force_push(self):
        oid = self.plumbing_commit({})
        self.git("push", "origin", f"{oid}:refs/heads/main")
        # Same parent/tree but different author, simulating a metadata rewrite.
        replacement = self.plumbing_commit({"GIT_AUTHOR_NAME": "Correct User"})
        self.git("push", f"--force-with-lease=refs/heads/main:{oid}",
                 "origin", f"{replacement}:refs/heads/main")


if __name__ == "__main__":
    unittest.main()
