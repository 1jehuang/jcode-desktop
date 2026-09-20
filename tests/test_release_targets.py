"""Keep build runners, bundled runtimes, and public release assets in agreement."""
from pathlib import Path
import re
import unittest

import test_prepare_public_release as fixtures

ROOT = Path(__file__).resolve().parents[1]


class ReleaseTargetTests(unittest.TestCase):
    def test_native_linux_windows_matrix_covers_both_architectures(self):
        workflow = (ROOT / ".github/workflows/cross-platform-release.yml").read_text()
        matrix = workflow.split("        include:\n", 1)[1].split("    runs-on:", 1)[0]
        rows = re.findall(
            r"- os: (\S+)\s+platform: (\S+)\s+arch: (\S+)\s+target: (\S+)\s+checksums: (\S+)", matrix
        )
        self.assertEqual(set(rows), {
            ("ubuntu-24.04", "linux", "x86_64", "x86_64-unknown-linux-gnu", "SHA256SUMS-linux"),
            ("ubuntu-24.04-arm", "linux", "aarch64", "aarch64-unknown-linux-gnu", "SHA256SUMS-linux-aarch64"),
            ("windows-2025", "windows", "x86_64", "x86_64-pc-windows-msvc", "SHA256SUMS-windows"),
            ("windows-11-arm", "windows", "aarch64", "aarch64-pc-windows-msvc", "SHA256SUMS-windows-aarch64"),
        })
        self.assertEqual(len(rows), 4)
        self.assertEqual(len({row[4] for row in rows}), 4, "Merged artifacts must not overwrite checksums")
        expected = fixtures.RELEASE.expected_assets(fixtures.TAG)
        for _, platform, arch, _, checksums in rows:
            suffix = "tar.gz" if platform == "linux" else "zip"
            self.assertIn(f"Jcode-0.1.0-beta.29-{platform}-{arch}.{suffix}", expected[checksums])
        self.assertEqual(workflow.count("TARGET: ${{ matrix.target }}"), 2)
        self.assertEqual(workflow.count("--target '${{ matrix.target }}'"), 2)
        self.assertIn("name: linux-smoke-diagnostics-${{ matrix.arch }}", workflow)
        self.assertIn("name: desktop-linux-${{ matrix.arch }}", workflow)
        self.assertIn("name: desktop-${{ matrix.platform }}-${{ matrix.arch }}", workflow)
        self.assertIn("arch: ${{ matrix.arch == 'aarch64' && 'amd64_arm64' || 'amd64' }}", workflow)

    def test_macos_keeps_both_universal_slices(self):
        workflow = (ROOT / ".github/workflows/macos-beta.yml").read_text()
        self.assertIn("targets: aarch64-apple-darwin,x86_64-apple-darwin", workflow)
        packager = (ROOT / "scripts/package-macos.sh").read_text()
        for target in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
            self.assertIn(target, packager)

    def test_native_voice_build_dependencies_are_installed(self):
        # CPAL uses ALSA on FreeBSD as well as Linux. These must be installed
        # before the desktop build, not just supplied on a developer machine.
        linux = (ROOT / ".github/workflows/cross-platform-release.yml").read_text()
        linux_prepare = linux.split("- name: Install Linux build dependencies", 1)[1].split(
            "- name: Build and package Linux", 1
        )[0]
        self.assertRegex(linux_prepare, r"\blibasound2-dev\b")
        freebsd = (ROOT / ".github/workflows/freebsd-release.yml").read_text()
        freebsd_prepare = freebsd.split("          prepare: |\n", 1)[1].split(
            "          run: |\n", 1
        )[0]
        self.assertRegex(freebsd_prepare, r"\balsa-lib\b")

    def test_linux_package_declares_native_voice_runtime(self):
        # Native capture creates a DT_NEEDED entry for libasound.so.2 even
        # before the user activates Voice. Support both Debian package names.
        packager = (ROOT / "scripts/package-linux.sh").read_text()
        depends = re.search(r"^Depends: (.+)$", packager, re.MULTILINE).group(1)
        self.assertIn("libasound2t64 | libasound2", depends.split(", "))

    def test_cli_release_target_parity(self):
        cli = ROOT.parent / "jcode/.github/workflows/release.yml"
        if not cli.exists():
            self.skipTest("Adjacent CLI checkout is needed for upstream parity check")
        cli_targets = set(re.findall(r"target: ([a-z0-9_]+-[a-z0-9_-]+)", cli.read_text()))
        cli_targets |= set(re.findall(r"Build \(([a-z0-9_]+-[a-z0-9_-]+)\)", cli.read_text()))
        desktop = (ROOT / ".github/workflows/cross-platform-release.yml").read_text()
        targets = set(re.findall(r"target: ([a-z0-9_]+-[a-z0-9_-]+)", desktop))
        targets |= {"aarch64-apple-darwin", "x86_64-apple-darwin"}
        freebsd = (ROOT / ".github/workflows/freebsd-release.yml").read_text()
        self.assertIn("x86_64-unknown-freebsd", freebsd)
        targets.add("x86_64-unknown-freebsd")
        self.assertIn("needs: [build, build-freebsd]", desktop)
        self.assertEqual(cli_targets, targets,
                         "CLI added a release target that Desktop must account for")


if __name__ == "__main__":
    unittest.main()
