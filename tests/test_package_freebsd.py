import ast
import hashlib
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tarfile
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-unknown-freebsd"
BINARIES = ("jcode-desktop", "jcode", "jcode-harness-api-bridge")


@unittest.skipUnless(all(shutil.which(c) for c in ("bash", "tar", "install", "python3")), "packaging tools required")
class FreeBSDPackagingTests(unittest.TestCase):
    def fixture(self, root):
        desktop, cli = root / "desktop", root / "cli"
        for repo in (desktop, cli):
            (repo / "target" / TARGET / "release").mkdir(parents=True)
            (repo / "Cargo.toml").write_text('[package]\nversion = "1.2.3"\n')
        for relative in ("scripts", "assets/app-icon", "packaging/linux"):
            (desktop / relative).mkdir(parents=True)
        for script in ("package-freebsd.sh", "verify-release-package.py"):
            shutil.copy(ROOT / "scripts" / script, desktop / "scripts" / script)
        (desktop / "assets/app-icon/icon-1024.png").write_bytes(b"fixture icon")
        (desktop / "packaging/linux/jcode.desktop").write_text("[Desktop Entry]\nName=Jcode\n")
        header = bytearray(64)
        header[:7] = b"\x7fELF\x02\x01\x01"
        header[7] = 9  # ELFOSABI_FREEBSD
        struct.pack_into("<H", header, 18, 62)
        for binary in BINARIES:
            repo = desktop if binary == "jcode-desktop" else cli
            (repo / "target" / TARGET / "release" / binary).write_bytes(header)
        env = {**os.environ, "SKIP_BUILD": "1", "VERSION": "desktop-v1.2.3", "TARGET": TARGET,
               "JCODE_REPO": str(cli), "OUT_DIR": str(root / "out")}
        return desktop, cli, env

    def run_package(self, desktop, env):
        return subprocess.run(["bash", str(desktop / "scripts/package-freebsd.sh")],
                              env=env, text=True, capture_output=True)

    def test_stages_all_files_verifies_and_checksums_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            desktop, _, env = self.fixture(Path(directory))
            result = self.run_package(desktop, env)
            self.assertEqual(result.returncode, 0, result.stderr)
            out = Path(env["OUT_DIR"])
            name = "Jcode-1.2.3-freebsd-x86_64.tar.gz"
            digest = hashlib.sha256((out / name).read_bytes()).hexdigest()
            self.assertEqual((out / "SHA256SUMS-freebsd-x86_64").read_text(), f"{digest}  {name}\n")
            with tarfile.open(out / name) as archive:
                files = {Path(m.name).name: m for m in archive.getmembers() if m.isfile()}
                self.assertEqual(set(files), set(BINARIES) | {"jcode.desktop", "jcode.png"})
                for binary in BINARIES:
                    self.assertEqual(files[binary].mode & 0o777, 0o755)
            self.assertFalse(list(out.glob(".stage.*")))

    def test_rejects_wrong_architecture_companion(self):
        with tempfile.TemporaryDirectory() as directory:
            desktop, cli, env = self.fixture(Path(directory))
            binary = cli / "target" / TARGET / "release/jcode-harness-api-bridge"
            header = bytearray(binary.read_bytes())
            struct.pack_into("<H", header, 18, 183)
            binary.write_bytes(header)
            result = self.run_package(desktop, env)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("architecture mismatch", result.stderr)
            self.assertFalse((Path(env["OUT_DIR"]) / "SHA256SUMS-freebsd-x86_64").exists())

    def test_rejects_missing_companion(self):
        with tempfile.TemporaryDirectory() as directory:
            desktop, cli, env = self.fixture(Path(directory))
            (cli / "target" / TARGET / "release/jcode").unlink()
            self.assertNotEqual(self.run_package(desktop, env).returncode, 0)

    def test_rejects_wrong_target_and_unsafe_version(self):
        with tempfile.TemporaryDirectory() as directory:
            desktop, _, env = self.fixture(Path(directory))
            for change in ({"TARGET": "x86_64-unknown-linux-gnu"}, {"VERSION": "../../bad"}):
                with self.subTest(change=change):
                    result = self.run_package(desktop, {**env, **change})
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(Path(env["OUT_DIR"]).exists())

    def test_workflow_embedded_smoke_compiles_and_is_gated(self):
        workflow = (ROOT / ".github/workflows/freebsd-release.yml").read_text()
        source = workflow.split("            python3 - <<'PY'\n", 1)[1].split("            PY\n", 1)[0]
        ast.parse(textwrap.dedent(source))
        self.assertIn('release: "15.1"', workflow)
        self.assertIn("name: desktop-freebsd-x86_64\n", workflow)
        self.assertIn("assert mapped", source)
        self.assertNotIn("continue-on-error:", workflow)
        upload = workflow.split("- name: Upload verified FreeBSD package", 1)[1].split("- name:", 1)[0]
        self.assertNotIn("if: always()", upload)
