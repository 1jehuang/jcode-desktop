#!/usr/bin/env python3
"""Offline tests of the real backup module. Never invokes SSH or AWS."""
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("cloud_alpha_backup", Path(__file__).with_name("backup.py"))
backup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(backup)
REAL_POPEN = subprocess.Popen


class BackupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir(mode=0o700)

    def archive(self, entries=(), roots=True):
        path = self.bundle / "archive.tar"
        with tarfile.open(path, "w", format=tarfile.USTAR_FORMAT) as output:
            if roots:
                for name in backup.ROOTS:
                    item = tarfile.TarInfo(name)
                    item.type = tarfile.DIRTYPE
                    output.addfile(item)
            for entry in entries:
                if isinstance(entry, tarfile.TarInfo):
                    output.addfile(entry)
                else:
                    name, content = entry
                    item = tarfile.TarInfo(name)
                    item.size = len(content)
                    item.mode = 0o7777
                    output.addfile(item, io.BytesIO(content))
        return path

    def inspect(self):
        with backup.open_regular(self.bundle / "archive.tar") as source:
            return backup.inspect_archive(source, backup.DEFAULT_MAX)

    def manifest(self):
        result = self.inspect()
        (self.bundle / "manifest.json").write_text(json.dumps(result))
        return result

    def test_success_fresh_restore_and_permissions(self):
        self.archive([("workspaces/project/main.py", b"print('hello')\n"),
                      (".jcode/sessions/history.json", b'{"message":"hello"}')])
        result = self.manifest()
        destination = self.root / "recovery"
        self.assertEqual(backup.verify(self.bundle, destination), result)
        self.assertEqual((destination / "workspaces/project/main.py").read_bytes(), b"print('hello')\n")
        self.assertEqual((destination / ".jcode/sessions/history.json").read_bytes(), b'{"message":"hello"}')
        self.assertEqual(destination.stat().st_mode & 0o777, 0o700)
        self.assertEqual((destination / "workspaces/project/main.py").stat().st_mode & 0o7777, 0o600)
        with self.assertRaises(FileExistsError):
            backup.verify(self.bundle, destination)

    def test_malicious_names(self):
        for name in ("../outside", "/etc/passwd", "workspaces/../../outside", "workspaces//bad",
                     "workspaces/./bad", "workspaces/evil\\path", "other/file", ".jcode/config.json",
                     "workspaces/line\nbreak"):
            with self.subTest(name=name):
                self.archive([(name, b"bad")])
                with self.assertRaises(backup.BackupError):
                    self.inspect()
        self.assertFalse((self.root / "outside").exists())

    def test_links_devices_and_extensions(self):
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.CHRTYPE, tarfile.BLKTYPE,
                     tarfile.FIFOTYPE, tarfile.GNUTYPE_SPARSE, tarfile.XHDTYPE, tarfile.GNUTYPE_LONGNAME):
            with self.subTest(kind=kind):
                item = tarfile.TarInfo("workspaces/malicious")
                item.type = kind
                if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
                    item.linkname = "/etc/passwd"
                self.archive([item])
                with self.assertRaises(backup.BackupError):
                    self.inspect()

    def test_privacy_exclusions_everywhere(self):
        for name in (".env", ".env.production", "credentials.json", "auth.json", "provider-auth.json",
                     "id_ed25519", "server.pem", "private.key", ".aws/config", ".ssh/config",
                     "environment.json", "api-token.txt", ".git/config", "secrets/data", ".netrc"):
            with self.subTest(name=name):
                self.archive([("workspaces/project/nested/" + name, b"sensitive")])
                with self.assertRaises(backup.BackupError):
                    self.inspect()

    def test_content_secrets_and_chunk_boundary(self):
        for content in (b"-----BEGIN OPENSSH PRIVATE KEY-----", b'"api_key": "abcdefgh12345678"',
                        b"x" * 65525 + b"-----BEGIN RSA PRIVATE KEY-----"):
            self.archive([("workspaces/innocent.txt", content)])
            with self.assertRaises(backup.BackupError):
                self.inspect()

    def test_duplicate_and_file_directory_conflicts(self):
        for entries in ([('workspaces/a', b'a'), ('workspaces/a', b'b')],
                        [('workspaces/a', b'a'), ('workspaces/a/b', b'b')],
                        [('workspaces/a/b', b'a'), ('workspaces/a', b'b')]):
            self.archive(entries)
            with self.assertRaises(backup.BackupError):
                self.inspect()

    def test_size_checksum_truncation_and_trailing_garbage(self):
        path = self.archive([("workspaces/a", b"original")])
        self.manifest()
        with backup.open_regular(path) as source:
            with self.assertRaises(backup.BackupError):
                backup.inspect_archive(source, 1024)
        data = path.read_bytes()
        path.write_bytes(data.replace(b"original", b"modified"))
        with self.assertRaises(backup.BackupError):
            backup.verify(self.bundle, self.root / "not-created")
        self.assertFalse((self.root / "not-created").exists())
        for bad in (data[:100], data[:2048], data + b"unexpected"):
            path.write_bytes(bad)
            with self.assertRaises((backup.BackupError, tarfile.TarError)):
                self.inspect()

    def test_missing_roots_and_manifest(self):
        self.archive(roots=False)
        with self.assertRaises(backup.BackupError):
            self.inspect()
        self.archive()
        with self.assertRaises(FileNotFoundError):
            backup.verify(self.bundle)

    def test_local_symlinks_and_private_directory(self):
        path = self.archive()
        self.manifest()
        alias = self.root / "alias"
        alias.symlink_to(self.bundle, target_is_directory=True)
        with self.assertRaises(backup.BackupError):
            backup.verify(alias)
        path.rename(self.root / "real.tar")
        path.symlink_to(self.root / "real.tar")
        with self.assertRaises(OSError):
            backup.verify(self.bundle)
        self.bundle.chmod(0o755)
        with self.assertRaises(backup.BackupError):
            backup.verify(self.bundle)

    def local_transport(self, payload, code=0, delay=0):
        # Substitute only the SSH boundary with a local child process and real pipes.
        fixture = self.root / "wire.tar"
        fixture.write_bytes(payload)
        def launch(command, **kwargs):
            self.assertEqual(command[0], "ssh")
            self.assertIn("-oStrictHostKeyChecking=yes", command)
            program = "import sys,time; time.sleep(float(sys.argv[3])); sys.stdout.buffer.write(open(sys.argv[1],'rb').read()); sys.exit(int(sys.argv[2]))"
            return REAL_POPEN([sys.executable, "-c", program, str(fixture), str(code), str(delay)], **kwargs)
        return mock.patch.object(backup.subprocess, "Popen", side_effect=launch)

    def test_backup_success_and_unique_bundles(self):
        data = self.archive([("workspaces/a", b"hello")]).read_bytes()
        with self.local_transport(data):
            one = backup.backup("alpha", self.root / "backups")
            two = backup.backup("alpha", self.root / "backups")
        self.assertNotEqual(one, two)
        backup.verify(one, self.root / "restored")
        self.assertEqual((one / "archive.tar").stat().st_mode & 0o777, 0o600)
        self.assertEqual((one / "manifest.json").stat().st_mode & 0o777, 0o600)

    def test_ssh_failure_size_and_timeout_keep_incomplete(self):
        data = self.archive().read_bytes()
        for number, (payload, code, cap, delay, timeout) in enumerate((
                (data, 255, backup.DEFAULT_MAX, 0, 5),
                (data, 0, 1024, 0, 5),
                (b"broken", 0, backup.DEFAULT_MAX, 0, 5),
                (data, 0, backup.DEFAULT_MAX, 1, 0.05))):
            destination = self.root / f"failed-{number}"
            with self.local_transport(payload, code, delay):
                with self.assertRaises((backup.BackupError, tarfile.TarError)):
                    backup.backup("alpha", destination, cap, timeout)
            bundles = list(destination.iterdir())
            self.assertEqual(len(bundles), 1)
            self.assertFalse((bundles[0] / "manifest.json").exists())
            self.assertLessEqual((bundles[0] / "archive.tar").stat().st_size, cap)

    def test_host_injection_never_launches(self):
        with mock.patch.object(backup.subprocess, "Popen") as launch:
            for host in ("-oProxyCommand=bad", "a;bad", "a b", "$(bad)"):
                with self.assertRaises(backup.BackupError):
                    backup.backup(host, self.root)
            launch.assert_not_called()

    def export_home(self):
        home = self.root / "home"
        (home / "workspaces/project").mkdir(parents=True)
        (home / ".jcode/sessions").mkdir(parents=True)
        (home / "workspaces/project/readme.txt").write_text("saved work")
        (home / ".jcode/sessions/one.json").write_text('{"message":"hello"}')
        return home

    def run_export(self, home, cap=backup.DEFAULT_MAX):
        return subprocess.run([sys.executable, str(Path(backup.__file__)), "--remote-export", str(cap)],
                              env={**os.environ, "HOME": str(home)}, capture_output=True, timeout=10)

    def test_real_export_excludes_private_files_and_restores(self):
        home = self.export_home()
        for name in (".env", "credentials.json", "provider-auth.json", "id_rsa", "private.pem"):
            (home / "workspaces/project" / name).write_text("DO NOT EXPORT")
        (home / ".jcode/auth.json").write_text("DO NOT EXPORT")
        result = self.run_export(home)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(b"DO NOT EXPORT", result.stdout)
        (self.bundle / "archive.tar").write_bytes(result.stdout)
        self.manifest()
        backup.verify(self.bundle, self.root / "restored")
        self.assertEqual((self.root / "restored/workspaces/project/readme.txt").read_text(), "saved work")

    def test_script_over_stdin_end_to_end_and_cli_recovery(self):
        home = self.export_home()
        def local_ssh(command, **kwargs):
            self.assertEqual(command[-1], "python3 - --remote-export " + str(backup.DEFAULT_MAX))
            return REAL_POPEN([sys.executable, "-", "--remote-export", str(backup.DEFAULT_MAX)],
                              env={**os.environ, "HOME": str(home)}, **kwargs)
        with mock.patch.object(backup.subprocess, "Popen", side_effect=local_ssh):
            bundle = backup.backup("fixture-alpha", self.root / "backups")
        destination = self.root / "cli-recovery"
        result = subprocess.run([sys.executable, backup.__file__, "verify", str(bundle),
                                 "--restore-to", str(destination)], capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((destination / "workspaces/project/readme.txt").read_text(), "saved work")
        self.assertEqual((destination / ".jcode").stat().st_mode & 0o777, 0o700)
        again = subprocess.run([sys.executable, backup.__file__, "verify", str(bundle),
                                "--restore-to", str(destination)], capture_output=True, timeout=10)
        self.assertNotEqual(again.returncode, 0)

    def test_untouched_vm_missing_sessions_fails_closed(self):
        home = self.root / "untouched"
        (home / "workspaces").mkdir(parents=True)
        (home / ".jcode").mkdir()
        def local_ssh(command, **kwargs):
            return REAL_POPEN([sys.executable, "-", "--remote-export", str(backup.DEFAULT_MAX)],
                              env={**os.environ, "HOME": str(home)}, **kwargs)
        with mock.patch.object(backup.subprocess, "Popen", side_effect=local_ssh):
            with self.assertRaisesRegex(backup.BackupError, "sessions both exist"):
                backup.backup("fixture-alpha", self.root / "backups")
        bundle = next((self.root / "backups").iterdir())
        self.assertFalse((bundle / "manifest.json").exists())
        self.assertFalse((home / ".jcode/sessions").exists())

    def test_member_cap_and_invalid_metadata(self):
        self.archive([("workspaces/a", b"a")])
        with mock.patch.object(backup, "MAX_MEMBERS", 2):
            with self.assertRaises(backup.BackupError):
                self.inspect()
        item = tarfile.TarInfo("workspaces/invalid")
        item.linkname = "outside"
        self.archive([item])
        with self.assertRaises(backup.BackupError):
            self.inspect()

    def test_real_export_refuses_links_special_files_and_size(self):
        home = self.export_home()
        target = home / "workspaces/bad"
        target.symlink_to("/etc/passwd")
        self.assertNotEqual(self.run_export(home).returncode, 0)
        target.unlink()
        os.link(home / "workspaces/project/readme.txt", target)
        self.assertNotEqual(self.run_export(home).returncode, 0)
        target.unlink()
        os.mkfifo(target)
        self.assertNotEqual(self.run_export(home).returncode, 0)
        target.unlink()
        self.assertNotEqual(self.run_export(home, 1024).returncode, 0)
        (home / "workspaces/project/readme.txt").write_text("-----BEGIN RSA PRIVATE KEY-----")
        result = self.run_export(home)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(b"-----BEGIN RSA PRIVATE KEY-----", result.stdout)


if __name__ == "__main__":
    unittest.main()
