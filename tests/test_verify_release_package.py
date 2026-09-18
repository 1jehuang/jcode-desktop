import hashlib
import io
import os
import pathlib
import platform
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile

ROOT = pathlib.Path(__file__).parents[1]
VERIFY = ROOT / "scripts/verify-release-package.py"
PACKAGE_LINUX = ROOT / "scripts/package-linux.sh"
BINARIES = ("jcode-desktop", "jcode", "jcode-harness-api-bridge")
LINUX_TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu")
WINDOWS_TARGETS = ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")


def binary_header(target):
    if "windows" not in target:
        header = bytearray(64)
        header[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<H", header, 18, 62 if target.startswith("x86_64") else 183)
    else:
        header = bytearray(154)
        header[:2] = b"MZ"
        struct.pack_into("<I", header, 60, 128)
        header[128:132] = b"PE\0\0"
        struct.pack_into("<H", header, 132, 0x8664 if target.startswith("x86_64") else 0xAA64)
        header[152:154] = b"\x0b\x02"
    return bytes(header)


class VerifyReleasePackageTests(unittest.TestCase):
    def run_verify(self, path, target):
        return subprocess.run([sys.executable, VERIFY, path, "--target", target], capture_output=True, text=True)

    def archive(self, root, target, overrides=None, omit=None):
        windows = "windows" in target
        suffix = ".exe" if windows else ""
        payload = {name + suffix: binary_header(target) for name in BINARIES}
        payload.update({"Jcode.png": b"", "jcode-desktop.exe.manifest": b""} if windows else {"jcode.desktop": b"", "jcode.png": b""})
        payload.update(overrides or {})
        if omit:
            del payload[omit]
        path = pathlib.Path(root) / ("release.zip" if windows else "release.tar.gz")
        if windows:
            with zipfile.ZipFile(path, "w") as archive:
                for name, data in payload.items():
                    archive.writestr("Jcode/" + name, data)
        else:
            with tarfile.open(path, "w:gz") as archive:
                for name, data in payload.items():
                    info = tarfile.TarInfo("Jcode/" + name)
                    info.size = len(data)
                    archive.addfile(info, io.BytesIO(data))
        return path

    def test_all_architectures(self):
        for target in LINUX_TARGETS + WINDOWS_TARGETS + ("x86_64-unknown-freebsd",):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root:
                result = self.run_verify(self.archive(root, target), target)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_every_binary_architecture_is_checked(self):
        for targets in (LINUX_TARGETS, WINDOWS_TARGETS):
            for target, wrong in (targets, targets[::-1]):
                for binary in BINARIES:
                    name = binary + (".exe" if "windows" in target else "")
                    with self.subTest(target=target, binary=name), tempfile.TemporaryDirectory() as root:
                        path = self.archive(root, target, {name: binary_header(wrong)})
                        result = self.run_verify(path, target)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn(name, result.stderr)
                        self.assertIn("architecture mismatch", result.stderr)

    def test_invalid_and_truncated_headers_fail(self):
        for target in LINUX_TARGETS + WINDOWS_TARGETS + ("x86_64-unknown-freebsd",):
            valid = binary_header(target)
            malformed = [b"", b"not a binary", valid[:20], valid[:2] + b"broken" + valid[8:]]
            if "windows" not in target:
                malformed += [valid[:4] + b"\x01" + valid[5:], valid[:5] + b"\x02" + valid[6:]]
            else:
                malformed += [valid[:128] + b"NOPE" + valid[132:], valid[:152] + b"\x0b\x01"]
                # Altering DOS reserved bytes is valid, not a corrupt PE header.
                malformed.pop(3)
            for data in malformed:
                with self.subTest(target=target, header=data[:20]), tempfile.TemporaryDirectory() as root:
                    name = "jcode.exe" if "windows" in target else "jcode"
                    result = self.run_verify(self.archive(root, target, {name: data}), target)
                    self.assertNotEqual(result.returncode, 0)

    def test_missing_companion_fails(self):
        for target in (LINUX_TARGETS[0], WINDOWS_TARGETS[0]):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root:
                name = "jcode.exe" if "windows" in target else "jcode"
                result = self.run_verify(self.archive(root, target, omit=name), target)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("missing " + name, result.stderr)

    def test_freebsd_checks_every_binary(self):
        target = "x86_64-unknown-freebsd"
        for name in BINARIES:
            with self.subTest(binary=name), tempfile.TemporaryDirectory() as root:
                result = self.run_verify(self.archive(root, target, {name: binary_header(LINUX_TARGETS[1])}), target)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("architecture mismatch", result.stderr)
                self.assertIn(name, result.stderr)

    def test_target_required_and_platform_must_match(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.archive(root, LINUX_TARGETS[0])
            result = subprocess.run([sys.executable, VERIFY, path], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotEqual(self.run_verify(path, WINDOWS_TARGETS[0]).returncode, 0)
            self.assertNotEqual(self.run_verify(path, "arm-unknown-linux-gnu").returncode, 0)

    def test_release_filename_inference_and_explicit_conflict(self):
        for target in LINUX_TARGETS + WINDOWS_TARGETS + ("x86_64-unknown-freebsd",):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root:
                windows = "windows" in target
                arch = target.split("-")[0]
                name = f"Jcode-1.2.3-{'windows' if windows else ('freebsd' if 'freebsd' in target else 'linux')}-{arch}{'.zip' if windows else '.tar.gz'}"
                path = self.archive(root, target).rename(pathlib.Path(root) / name)
                result = subprocess.run([sys.executable, VERIFY, path], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                wrong = next(t for t in (WINDOWS_TARGETS if windows else LINUX_TARGETS) if t != target)
                result = self.run_verify(path, wrong)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("filename architecture", result.stderr)

    def test_duplicate_binary_fails(self):
        for target in (LINUX_TARGETS[0], WINDOWS_TARGETS[0]):
            name = "jcode.exe" if "windows" in target else "jcode"
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root:
                result = self.run_verify(self.archive(root, target, {"other/" + name: binary_header(target)}), target)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("duplicate payload", result.stderr)


@unittest.skipUnless(sys.platform.startswith("linux") and all(shutil.which(c) for c in ("bash", "cargo", "dpkg-deb", "tar", "install", "sha256sum")), "Linux packaging tools required")
class LinuxPackagingTests(unittest.TestCase):
    def fixture(self, root, target, real_binary=False):
        desktop, cli = pathlib.Path(root) / "desktop", pathlib.Path(root) / "cli"
        for repo in (desktop, cli):
            repo.mkdir()
            (repo / "Cargo.toml").write_text('[package]\nversion = "1.2.3"\n')
            (repo / "target" / target / "release").mkdir(parents=True)
        for relative in ("scripts", "assets/app-icon", "packaging/linux"):
            (desktop / relative).mkdir(parents=True)
        shutil.copy(PACKAGE_LINUX, desktop / "scripts")
        shutil.copy(VERIFY, desktop / "scripts")
        (desktop / "assets/app-icon/icon-1024.png").write_bytes(b"fixture icon")
        (desktop / "packaging/linux/jcode.desktop").write_text("[Desktop Entry]\nName=Jcode\n")
        for name in BINARIES:
            repo = desktop if name == "jcode-desktop" else cli
            destination = repo / "target" / target / "release" / name
            if real_binary:
                shutil.copy(shutil.which("true"), destination)
            else:
                destination.write_bytes(binary_header(target))
        env = {**os.environ, "TARGET": target, "JCODE_REPO": str(cli), "SKIP_BUILD": "1", "OUT_DIR": str(pathlib.Path(root) / "out"), "VERSION": "desktop-v1.2.3"}
        return desktop, cli, env

    def run_package(self, desktop, env):
        return subprocess.run(["bash", desktop / "scripts/package-linux.sh"], env=env, capture_output=True, text=True)

    def check_packages(self, env, target):
        arch = target.split("-")[0]
        deb_arch = "amd64" if arch == "x86_64" else "arm64"
        out = pathlib.Path(env["OUT_DIR"])
        names = [f"Jcode-1.2.3-linux-{arch}.tar.gz", f"Jcode-1.2.3-linux-{deb_arch}.deb"]
        sums = "SHA256SUMS-linux" + ("-aarch64" if arch == "aarch64" else "")
        expected = "".join(f"{hashlib.sha256((out / name).read_bytes()).hexdigest()}  {name}\n" for name in names)
        self.assertEqual((out / sums).read_text(), expected)
        for name in names:
            result = subprocess.run([sys.executable, VERIFY, out / name, "--target", target], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        control = subprocess.check_output(["dpkg-deb", "--field", out / names[1], "Architecture"], text=True).strip()
        self.assertEqual(control, deb_arch)
        return out / names[1]

    def test_real_packaging_both_architectures(self):
        for target in LINUX_TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root:
                desktop, _, env = self.fixture(root, target)
                result = self.run_package(desktop, env)
                self.assertEqual(result.returncode, 0, result.stderr)
                deb = self.check_packages(env, target)
                wrong_target = next(t for t in LINUX_TARGETS if t != target)
                result = subprocess.run([sys.executable, VERIFY, deb, "--target", wrong_target], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("does not match", result.stderr)

    def test_native_autodetect_with_real_elf_binaries(self):
        target = platform.machine() + "-unknown-linux-gnu"
        if target not in LINUX_TARGETS:
            self.skipTest("unsupported native architecture")
        with tempfile.TemporaryDirectory() as root:
            desktop, _, env = self.fixture(root, target, real_binary=True)
            del env["TARGET"]
            result = self.run_package(desktop, env)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.check_packages(env, target)

    def test_mislabeled_binary_stops_packaging_before_checksums(self):
        for name in BINARIES:
            with self.subTest(binary=name), tempfile.TemporaryDirectory() as root:
                desktop, cli, env = self.fixture(root, LINUX_TARGETS[1])
                repo = desktop if name == "jcode-desktop" else cli
                (repo / "target" / LINUX_TARGETS[1] / "release" / name).write_bytes(binary_header(LINUX_TARGETS[0]))
                result = self.run_package(desktop, env)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("architecture mismatch", result.stderr)
                self.assertFalse(list(pathlib.Path(env["OUT_DIR"]).glob("SHA256SUMS*")))

    def test_debian_payload_binaries_are_checked_independently(self):
        target = LINUX_TARGETS[1]
        for name in BINARIES:
            with self.subTest(binary=name), tempfile.TemporaryDirectory() as root:
                desktop, _, env = self.fixture(root, target)
                result = self.run_package(desktop, env)
                self.assertEqual(result.returncode, 0, result.stderr)
                out = pathlib.Path(env["OUT_DIR"])
                (out / "deb-root/usr/bin" / name).write_bytes(binary_header(LINUX_TARGETS[0]))
                deb = out / "corrupt.deb"
                subprocess.run(["dpkg-deb", "--root-owner-group", "--build", out / "deb-root", deb], check=True, capture_output=True)
                result = subprocess.run([sys.executable, VERIFY, deb, "--target", target], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("architecture mismatch", result.stderr)
                self.assertIn(name, result.stderr)

    def test_build_commands_have_explicit_target(self):
        with tempfile.TemporaryDirectory() as root:
            target = LINUX_TARGETS[1]
            desktop, _, env = self.fixture(root, target)
            stub = pathlib.Path(root) / "bin"
            stub.mkdir()
            log = pathlib.Path(root) / "cargo.log"
            (stub / "cargo").write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$CARGO_LOG"\n')
            (stub / "cargo").chmod(0o755)
            env.update(SKIP_BUILD="0", PATH=str(stub) + os.pathsep + env["PATH"], CARGO_LOG=str(log))
            result = self.run_package(desktop, env)
            self.assertEqual(result.returncode, 0, result.stderr)
            commands = log.read_text().splitlines()
            self.assertEqual(len(commands), 3)
            for command, binary in zip(commands, BINARIES):
                self.assertIn(f"--target {target}", command)
                self.assertIn(f"--bin {binary}", command)


if __name__ == "__main__":
    unittest.main()
