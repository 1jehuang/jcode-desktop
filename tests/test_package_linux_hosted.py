"""Hosted compilation orchestration tests. Compiler outputs are synthetic ELF fixtures,
not evidence that the real hosted Desktop build or native smoke passed."""
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-unknown-linux-gnu"
SPEC = importlib.util.spec_from_file_location("hosted_evidence", ROOT / "scripts/verify-hosted-build.py")
EVIDENCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVIDENCE)


class HostedEvidenceTests(unittest.TestCase):
    def report(self):
        return {"exit": 0, "remote": {"machine": "hosted"}, "start": 100, "toolchain": "1.90.0",
                "args": ["build", "--release", "--locked", "--target", TARGET, "--bin", "jcode-desktop"]}

    def test_requires_current_successful_hosted_locked_release(self):
        report = self.report()
        EVIDENCE.verify(report, "jcode-desktop", TARGET, 99, "1.90.0")
        changes = [{"toolchain": "1.99.0"}, {"toolchain": None}, {"exit": 1}, {"remote": {}}, {"remote": {"machine": "aws"}}, {"start": 98},
                   {"args": report["args"][:-1]}, {"args": ["check"]},
                   {"args": [a for a in report["args"] if a != "--locked"]}]
        for change in changes:
            with self.subTest(change=change), self.assertRaises(ValueError):
                EVIDENCE.verify({**report, **change}, "jcode-desktop", TARGET, 99, "1.90.0")
        with self.assertRaises(ValueError):
            EVIDENCE.verify(report, "jcode", TARGET, 99, "1.90.0")


@unittest.skipUnless(all(shutil.which(c) for c in ("bash", "tar", "dpkg-deb", "install", "python3")), "Linux package tools required")
class HostedPackagingTests(unittest.TestCase):
    def fixture(self, root):
        desktop, runtime, commands = root / "desktop", root / "runtime", root / "commands"
        for repo in (desktop, runtime):
            repo.mkdir()
            (repo / "Cargo.toml").write_text('[package]\nversion = "1.2.3"\n')
            (repo / "Cargo.lock").write_text("# fixture\n")
        (runtime / "crates/jcode-build-meta").mkdir(parents=True)
        for directory in ("scripts", "packaging/linux", "assets/app-icon"):
            (desktop / directory).mkdir(parents=True)
        for script in ("package-linux.sh", "build-linux-hosted.sh", "verify-hosted-build.py", "verify-release-package.py"):
            shutil.copy(ROOT / "scripts" / script, desktop / "scripts" / script)
            (desktop / "scripts" / script).chmod(0o755)
        (desktop / "packaging/linux/jcode.desktop").write_text("[Desktop Entry]\nName=Jcode\n")
        (desktop / "assets/app-icon/icon-1024.png").write_bytes(b"fixture")
        commands.mkdir()
        scripts = {
            "git": '''#!/usr/bin/env python3
import os,sys
args=sys.argv[1:]
if "status" in args:
    print(" M source.rs" if os.getenv("DIRTY_SOURCE") else "",end="")
elif "rev-parse" in args:
    print("a"*7 if "--short" in args else "a"*40)
elif "describe" in args:
    print("v1.2.3")
elif "log" in args:
    if any("%H%x1e" in a for a in args): print("a"*40+"\\x1eHEAD\\x1e2026-10-14 UTC\\x1eRelease\\x1f")
    elif "--format=%ct" in args: print("100")
    elif "--format=%ci" in args: print("2026-10-14 00:00:00 +0000")
    elif any("%h|%ct" in a for a in args): print("aaaaaaa|100|tag: v1.2.3|Release")
    else: print("Release")
else: sys.exit(1)
''',
            "justrust": '''#!/usr/bin/env python3
import json,os,pathlib,struct,sys,time
args=sys.argv[1:]
if args[:2]==["remote","use"]: sys.exit(0)
if args[0]=="show":
    print(pathlib.Path("target/mock-report.json").read_text()); sys.exit(0)
assert args[0]=="build"
assert os.environ["JUSTRUST_REMOTE_REQUIRED"]=="1"
assert os.environ["JUSTRUST_TOOLCHAIN"]=="1.90.0"
assert os.environ["JUSTRUST_TOOLCHAIN_REQUIRED"]=="1"
assert os.environ["RUSTUP_TOOLCHAIN"]=="1.90.0"
assert os.environ["JCODE_DESKTOP_VERSION"]=="1.2.3"
assert "--remote" in args and "--locked" in args and "--release" in args
binary=args[args.index("--bin")+1]
with open(os.environ["MOCK_CALLS"],"a") as f: f.write(binary+"\\n")
if os.getenv("BUILD_FAIL"): sys.exit(42)
if binary=="jcode-desktop":
    assert pathlib.Path(".jcode-release-metadata/history.txt").read_text().startswith("a"*40)
    assert os.environ["JCODE_DESKTOP_BUILD_METADATA"]=="1"
else:
    assert os.environ["JCODE_BUILD_SEMVER"]=="1.2.3"
    assert os.environ["JCODE_RELEASE_BUILD"]=="1"
    assert os.environ["JCODE_BUILD_METADATA_FILE"]=="../../.jcode-release-metadata/build.txt"
    assert "changelog_raw<<" in (pathlib.Path("crates/jcode-build-meta")/os.environ["JCODE_BUILD_METADATA_FILE"]).read_text()
path=pathlib.Path("target")/args[args.index("--target")+1]/"release"/binary
path.parent.mkdir(parents=True,exist_ok=True)
if not os.getenv("MISSING_ARTIFACT"):
    header=bytearray(64); header[:6]=b"\\x7fELF\\x02\\x01"; struct.pack_into("<H",header,18,183 if os.getenv("WRONG_ARCH") else 62)
    path.write_bytes(header); path.chmod(0o755)
report={"exit":0,"toolchain":"1.90.0","start":time.time(),"args":[a for a in args if a!="--remote"],"remote":{"machine":"local" if os.getenv("LOCAL_FALLBACK") else "hosted"}}
pathlib.Path("target/mock-report.json").write_text(json.dumps(report))
''',
            "cargo": "#!/usr/bin/env bash\necho 'unexpected local compiler' >&2\nexit 90\n",
            "zstd": "#!/usr/bin/env bash\nexit 0\n",
            "rsync": "#!/usr/bin/env bash\nexit 0\n",
            "ssh": "#!/usr/bin/env bash\nexit 0\n",
        }
        for name, script in scripts.items():
            (commands / name).write_text(script)
            (commands / name).chmod(0o755)
        env = {**os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"],
               "LINUX_BUILD_BACKEND": "hosted", "VERSION": "1.2.3", "TARGET": TARGET,
               "RUSTUP_TOOLCHAIN": "1.90.0", "JCODE_REPO": str(runtime),
               "OUT_DIR": str(root / "out"), "MOCK_CALLS": str(root / "calls")}
        for name in ("SKIP_BUILD", "JCODE_BUILD_METADATA_FILE", "HOSTED_EVIDENCE_DIR"):
            env.pop(name, None)
        return desktop, runtime, env

    def package(self, desktop, env):
        return subprocess.run(["bash", str(desktop / "scripts/package-linux.sh")], env=env, capture_output=True, text=True)

    def test_hosted_builds_three_binaries_and_locally_verifies_both_packages(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            desktop, runtime, env = self.fixture(root)
            result = self.package(desktop, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual((root / "calls").read_text().splitlines(), ["jcode-desktop", "jcode", "jcode-harness-api-bridge"])
            self.assertEqual(len([p for p in (root / "out").glob("Jcode-*.*") if p.is_file()]), 2)
            checksums = subprocess.run(["sha256sum", "-c", "SHA256SUMS-linux"], cwd=root / "out", capture_output=True)
            self.assertEqual(checksums.returncode, 0, checksums.stderr)
            self.assertFalse((desktop / ".jcode-release-metadata").exists())
            self.assertFalse((runtime / ".jcode-release-metadata").exists())
            self.assertEqual(len(list((desktop / "target/hosted-release-evidence").glob("*.json"))), 3)

    def test_failures_never_package_or_fall_back(self):
        for change in ({"BUILD_FAIL": "1"}, {"LOCAL_FALLBACK": "1"}, {"MISSING_ARTIFACT": "1"},
                       {"DIRTY_SOURCE": "1"}, {"SKIP_BUILD": "1"}, {"TARGET": "aarch64-unknown-linux-gnu"},
                       {"LINUX_BUILD_BACKEND": "invalid"}, {"RUSTUP_TOOLCHAIN": "stable"}):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                desktop, runtime, env = self.fixture(Path(directory))
                result = self.package(desktop, {**env, **change})
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertFalse(Path(env["OUT_DIR"]).exists())
                self.assertFalse((desktop / ".jcode-release-metadata").exists())
                self.assertFalse((runtime / ".jcode-release-metadata").exists())

    def test_real_package_verifier_still_rejects_wrong_architecture(self):
        with tempfile.TemporaryDirectory() as directory:
            desktop, _, env = self.fixture(Path(directory))
            result = self.package(desktop, {**env, "WRONG_ARCH": "1"})
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("architecture mismatch", result.stderr)
            self.assertFalse((Path(env["OUT_DIR"]) / "SHA256SUMS-linux").exists())

    def test_manual_workflow_preserves_native_smokes_and_never_publishes(self):
        workflow = (ROOT / ".github/workflows/linux-hosted-validation.yml").read_text()
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("if: github.ref == 'refs/heads/main'", workflow)
        self.assertIn("contents: read", workflow)
        self.assertNotIn("contents: write", workflow)
        for forbidden in ("pull_request:", "push:", "gh release", "release upload", "PUBLIC_RELEASE_TOKEN"):
            self.assertNotIn(forbidden, workflow)
        self.assertIn("secrets.JCODE_API_KEY", workflow)
        self.assertIn("persist-credentials: false", workflow)
        self.assertIn("justrust_revision", workflow)
        self.assertIn("RUSTUP_TOOLCHAIN: ${{ inputs.rust_version }}", workflow)
        original = (ROOT / ".github/workflows/cross-platform-release.yml").read_text()
        for name in ("Smoke-test Linux package on X11", "Smoke-test Linux package on native Wayland"):
            def body(text):
                return text.split("      - name: " + name, 1)[1].split("      - name:", 1)[0].replace("        if: matrix.platform == 'linux'\n", "")
            self.assertEqual(body(original), body(workflow))
            self.assertLess(workflow.index(name), workflow.index("Preserve successful validation packages"))
        self.assertNotIn("LINUX_BUILD_BACKEND", original)


@unittest.skipUnless(shutil.which("rustc"), "standalone build-script compiler required")
class DesktopSnapshotIntegrationTests(unittest.TestCase):
    def test_real_build_script_preserves_snapshot_and_keeps_normal_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "build-script"
            compile_result = subprocess.run(["rustc", "--edition=2024", str(ROOT / "crates/jcode-desktop-ui/build.rs"), "-o", str(binary)], capture_output=True, text=True)
            self.assertEqual(compile_result.returncode, 0, compile_result.stderr)
            manifest = root / "source/crates/jcode-desktop-ui"
            manifest.mkdir(parents=True)
            source = root / "source"
            (source / "Cargo.toml").write_text('[package]\nversion = "1.2.3"\n')
            (source / "CHANGELOG.md").write_text("Curated release notes\n")
            out = root / "out"
            out.mkdir()
            env = {**os.environ, "CARGO_MANIFEST_DIR": str(manifest), "OUT_DIR": str(out),
                   "JCODE_DESKTOP_VERSION": "1.2.3", "SOURCE_DATE_EPOCH": "1234"}
            env.pop("JCODE_DESKTOP_BUILD_METADATA", None)
            normal = subprocess.run([str(binary)], env=env, capture_output=True, text=True)
            self.assertEqual(normal.returncode, 0, normal.stderr)
            self.assertIn("JCODE_DESKTOP_GIT_HASH=unknown", normal.stdout)
            snapshot = source / ".jcode-release-metadata"
            snapshot.mkdir()
            history = "a" * 40 + "\x1eHEAD\x1e2026-10-14 UTC\x1eSnapshot release\x1f\n"
            for name, contents in (("hash", "a" * 40), ("status", ""), ("subjects", "Snapshot release\n"), ("history", history)):
                (snapshot / f"{name}.txt").write_text(contents)
            hosted = subprocess.run([str(binary)], env={**env, "JCODE_DESKTOP_BUILD_METADATA": "1"}, capture_output=True, text=True)
            self.assertEqual(hosted.returncode, 0, hosted.stderr)
            self.assertIn("JCODE_DESKTOP_VERSION=1.2.3", hosted.stdout)
            self.assertIn("JCODE_DESKTOP_GIT_HASH=" + "a" * 40, hosted.stdout)
            self.assertIn("JCODE_DESKTOP_GIT_DIRTY=false", hosted.stdout)
            self.assertIn("JCODE_DESKTOP_BUILT_AT=1234", hosted.stdout)
            self.assertEqual((out / "update-history.txt").read_text(), history)
            self.assertIn("Snapshot release", (out / "changelog.md").read_text())
            (snapshot / "status.txt").write_text(" M source.rs\n")
            invalid = subprocess.run([str(binary)], env={**env, "JCODE_DESKTOP_BUILD_METADATA": "1"}, capture_output=True, text=True)
            self.assertNotEqual(invalid.returncode, 0)
            self.assertIn("clean source checkout", invalid.stderr)


if __name__ == "__main__":
    unittest.main()
