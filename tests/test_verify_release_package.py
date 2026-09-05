import io, pathlib, tarfile, tempfile, unittest, zipfile
import subprocess, sys

ROOT = pathlib.Path(__file__).parents[1]
VERIFY = ROOT / "scripts/verify-release-package.py"
PACKAGE_LINUX = ROOT / "scripts/package-linux.sh"

class VerifyReleasePackageTests(unittest.TestCase):
    def run_verify(self, path):
        return subprocess.run([sys.executable, VERIFY, path], capture_output=True)

    def test_linux_archive_contract(self):
        with tempfile.TemporaryDirectory() as root:
            path = pathlib.Path(root) / "release.tar.gz"
            with tarfile.open(path, "w:gz") as archive:
                for name in ("jcode-desktop", "jcode", "jcode-harness-api-bridge", "jcode.desktop", "jcode.png"):
                    info = tarfile.TarInfo("Jcode/" + name); info.size = 0; archive.addfile(info, io.BytesIO())
            self.assertEqual(self.run_verify(path).returncode, 0)

    def test_windows_archive_contract(self):
        with tempfile.TemporaryDirectory() as root:
            path = pathlib.Path(root) / "release.zip"
            with zipfile.ZipFile(path, "w") as archive:
                for name in ("jcode-desktop.exe", "jcode.exe", "jcode-harness-api-bridge.exe", "Jcode.png", "jcode-desktop.exe.manifest"):
                    archive.writestr("Jcode/" + name, b"")
            self.assertEqual(self.run_verify(path).returncode, 0)

    def test_missing_companion_fails(self):
        with tempfile.TemporaryDirectory() as root:
            path = pathlib.Path(root) / "release.zip"
            with zipfile.ZipFile(path, "w") as archive: archive.writestr("Jcode/jcode-desktop.exe", b"")
            self.assertNotEqual(self.run_verify(path).returncode, 0)

    def test_linux_checksums_use_downloaded_artifact_basenames(self):
        script = PACKAGE_LINUX.read_text()
        checksum_block = script[script.index('  sha256sum "Jcode-'):script.index('\n)', script.index('  sha256sum "Jcode-'))]
        self.assertIn('> SHA256SUMS-linux', checksum_block)
        self.assertNotIn('$OUT/', checksum_block)

if __name__ == "__main__": unittest.main()
