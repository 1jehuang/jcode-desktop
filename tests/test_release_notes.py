"""Release notes shared by the public release page and Discord announcement."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("release_notes", ROOT / "scripts/release_notes.py")
N = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(N)
CHANGELOG = """## What's new

### Jcode Desktop 0.4.0

Voice everywhere

#### Themes
- Global voice.

#### Highlights
- Onboarding.
  With inline sign-in.
#### Downloads
- Not a feature
#### Fixes
- Scrolling.

```md
### Jcode Desktop 9.9.9
- Example only
```

Downloads are available at https://jcode.sh/desktop.

## Previous releases

### Jcode Desktop 0.3.0

- Old highlight.
"""


class ChangelogTests(unittest.TestCase):
    def test_exact_section_with_headline_and_known_sections(self):
        notes = N.notes_for_tag("desktop-v0.4.0", changelog=CHANGELOG)
        self.assertEqual(notes["source"], "changelog")
        self.assertEqual(notes["headline"], "Voice everywhere")
        self.assertEqual(notes["sections"], [("Themes", ["Global voice."]),
                                             ("Highlights", ["Onboarding. With inline sign-in."]),
                                             ("Fixes", ["Scrolling."])])
        text = N.to_markdown(notes)
        for unwanted in ("Not a feature", "Example only", "Old highlight", "Downloads are"):
            self.assertNotIn(unwanted, text)

    def test_unsectioned_previous_release_is_highlights(self):
        notes = N.notes_for_tag("desktop-v0.3.0", changelog=CHANGELOG)
        self.assertEqual(notes["sections"], [("Highlights", ["Old highlight."])])

    def test_no_prefix_match_for_other_versions(self):
        self.assertIsNone(N.changelog_section(CHANGELOG, "0.4"))
        self.assertIsNone(N.changelog_section(CHANGELOG, "9.9.9"))

    def test_limit_drops_whole_bullets_from_later_sections_first(self):
        notes = {"headline": "H", "sections": [("Themes", ["t" * 50]), ("Fixes", ["f" * 50] * 20)]}
        text = N.to_markdown(notes, limit=300)
        self.assertLessEqual(len(text), 300)
        self.assertIn("t" * 50, text)
        self.assertIn("more changes in the full notes", text)
        self.assertNotIn("…", text)

    def test_repository_changelog_has_current_version(self):
        import tomllib
        version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        notes = N.notes_for_tag(f"desktop-v{version}")
        self.assertEqual(notes["source"], "changelog")
        self.assertTrue(notes["sections"])


class CommitFallbackTests(unittest.TestCase):
    def repo(self):
        directory = tempfile.TemporaryDirectory()
        path = Path(directory.name)
        def git(*args):
            subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.com", *args],
                           cwd=path, check=True, capture_output=True)
        git("init", "-q")
        (path / "CHANGELOG.md").write_text("## What's new\n")
        for message, tag in (("Initial", "desktop-v0.1.0"), ("Voice: global hold", None),
                             ("Merge branch x", None), ("Release Desktop 0.1.1-beta.1", "desktop-v0.1.1-beta.1"),
                             ("Sidebar: projects", "desktop-v0.1.1-beta.2")):
            git("commit", "-q", "--allow-empty", "-m", message)
            if tag:
                git("tag", tag)
        return directory, path

    def test_beta_uses_commits_since_previous_tag_without_noise(self):
        directory, path = self.repo()
        with directory:
            notes = N.notes_for_tag("desktop-v0.1.1-beta.2", root=path)
            self.assertEqual(notes, {"headline": "", "sections": [("Changes", ["Sidebar: projects"])], "source": "commits"})
            notes = N.notes_for_tag("desktop-v0.1.1-beta.1", root=path)
            self.assertEqual(notes["sections"], [("Changes", ["Voice: global hold"])])

    def test_missing_tag_or_git_yields_none(self):
        directory, path = self.repo()
        with directory:
            self.assertEqual(N.notes_for_tag("desktop-v9.0.0", root=path)["source"], "none")
        with tempfile.TemporaryDirectory() as empty:
            (Path(empty) / "CHANGELOG.md").write_text("")
            self.assertEqual(N.notes_for_tag("desktop-v9.0.0", root=Path(empty))["source"], "none")

    def test_stable_compares_against_previous_stable(self):
        tags = ["desktop-v0.3.2", "desktop-v0.3.3-beta.1", "desktop-v0.3.3-beta.2", "desktop-v0.3.3"]
        self.assertEqual(N.previous_tag("desktop-v0.3.3", tags), "desktop-v0.3.2")
        self.assertEqual(N.previous_tag("desktop-v0.3.3-beta.2", tags), "desktop-v0.3.3-beta.1")
        self.assertIsNone(N.previous_tag("desktop-v0.3.2", tags))


class ConsumerTests(unittest.TestCase):
    def load(self, name):
        spec = importlib.util.spec_from_file_location(name, ROOT / f"scripts/{name}.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_discord_prefers_changelog_over_placeholder_body(self):
        D = self.load("post_discord_release")
        notes = N.notes_for_tag("desktop-v0.4.0", changelog=CHANGELOG)
        placeholder = "Verified platform packages. All release gates must pass before publication."
        message = D.format_message(tag="desktop-v0.4.0", body=placeholder, url="https://example.com/r", notes=notes)
        self.assertIn("### Highlights\n- Onboarding. With inline sign-in.", message)
        self.assertIn("Voice everywhere", message)
        self.assertNotIn("New Desktop release available", message)
        self.assertNotIn(placeholder, message)

    def test_discord_sanitizes_and_bounds_long_notes(self):
        D = self.load("post_discord_release")
        notes = {"headline": "@everyone", "sections": [("Fixes", ["@here <!-- x --> " + "y" * 300] * 30)]}
        message = D.format_message(tag="desktop-v0.4.0", body="", url="https://example.com/r", notes=notes)
        self.assertLessEqual(len(message), D.DISCORD_LIMIT)
        self.assertNotIn("@everyone", message)
        self.assertNotIn("@here", message)
        self.assertNotIn("<!--", message)
        self.assertIn(D.WEBSITE, message)

    def test_public_release_body_includes_notes(self):
        P = self.load("publish-public-release")
        body = P.release_body("desktop-v0.3.3")
        self.assertTrue(body.startswith(P.INTRO))
        self.assertIn("## What's new", body)
        self.assertIn("### Highlights", body)

    def test_announcing_workflows_fetch_full_history(self):
        for workflow in ("discord-release.yml", "publish-public-release.yml"):
            self.assertIn("fetch-depth: 0", (ROOT / ".github/workflows" / workflow).read_text())


if __name__ == "__main__":
    unittest.main()
