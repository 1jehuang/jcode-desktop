import base64
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "prepare_public_release", Path(__file__).resolve().parents[1] / "scripts/prepare-public-release.py"
)
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)
TAG = "desktop-v0.1.0-beta.24"


class PublicReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        for checksum, names in RELEASE.expected_assets(TAG).items():
            lines = []
            for name in names:
                path = self.directory / name
                path.write_bytes(b"release fixture")
                lines.append(f"{RELEASE.digest(path)}  {name}\n")
            (self.directory / checksum).write_text("".join(lines))
        self.archive = "Jcode-0.1.0-beta.24-macOS-universal.zip"
        self.url = f"{RELEASE.PUBLIC_BASE}/{TAG}/{self.archive}"
        self.appcast = self.directory / "appcast.xml"
        self.appcast.write_text(
            f'<rss xmlns:sparkle="{RELEASE.SPARKLE}"><channel><item>'
            f'<enclosure url="{self.url}" length="15" '
            f'sparkle:edSignature="{base64.b64encode(bytes(64)).decode()}"/>'
            '</item></channel></rss>'
        )

    def test_complete_release_has_only_public_package_urls(self):
        manifest = RELEASE.validate(self.directory, TAG)
        self.assertEqual(manifest["tag_name"], TAG)
        self.assertEqual(len(manifest["assets"]), 9)
        self.assertTrue(all(a["browser_download_url"].startswith(RELEASE.PUBLIC_BASE) for a in manifest["assets"]))
        (self.directory / "private-source.tar.gz").write_bytes(b"not an artifact")
        self.assertEqual(len(RELEASE.validate(self.directory, TAG)["assets"]), 9)

    def test_corruption_fails(self):
        (self.directory / self.archive).write_bytes(b"corrupted")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            RELEASE.validate(self.directory, TAG)

    def test_missing_platform_fails(self):
        (self.directory / "SHA256SUMS-windows").write_text("")
        with self.assertRaisesRegex(ValueError, "incomplete checksum"):
            RELEASE.validate(self.directory, TAG)

    def test_private_update_url_fails(self):
        self.appcast.write_text(self.appcast.read_text().replace(self.url, "https://github.com/private/download.zip"))
        with self.assertRaisesRegex(ValueError, "public archive"):
            RELEASE.validate(self.directory, TAG)

    def test_missing_signature_fails(self):
        self.appcast.write_text(self.appcast.read_text().replace(base64.b64encode(bytes(64)).decode(), ""))
        with self.assertRaisesRegex(ValueError, "Ed25519 signature"):
            RELEASE.validate(self.directory, TAG)

    def test_incorrect_archive_length_fails(self):
        self.appcast.write_text(self.appcast.read_text().replace('length="15"', 'length="100"'))
        with self.assertRaisesRegex(ValueError, "length mismatch"):
            RELEASE.validate(self.directory, TAG)

    def test_historical_absolute_checksum_paths_only_match_basenames(self):
        path = self.directory / "SHA256SUMS-linux"
        path.write_text(path.read_text().replace("  Jcode-", "  /home/runner/dist/Jcode-"))
        self.assertEqual(len(RELEASE.validate(self.directory, TAG)["assets"]), 9)

    def test_unexpected_checksum_asset_fails(self):
        path = self.directory / "SHA256SUMS-linux"
        path.write_text(path.read_text() + "0" * 64 + "  source.tar.gz\n")
        with self.assertRaisesRegex(ValueError, "unexpected or duplicate"):
            RELEASE.validate(self.directory, TAG)

    def test_symlink_asset_fails(self):
        path = self.directory / self.archive
        other = self.directory / "other"
        path.rename(other)
        path.symlink_to(other)
        with self.assertRaisesRegex(ValueError, "unsafe"):
            RELEASE.validate(self.directory, TAG)

    def test_tag_path_injection_fails(self):
        for tag in ("../../etc", "desktop-v1.2.3/../secret", "desktop-v1.2.3;id"):
            with self.assertRaisesRegex(ValueError, "invalid desktop release tag"):
                RELEASE.expected_assets(tag)


if __name__ == "__main__":
    unittest.main()
