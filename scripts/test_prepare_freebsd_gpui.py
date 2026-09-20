"""Offline safety tests. GPUI_PINNED_SOURCE optionally checks a real source copy."""

import hashlib
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "prepare_freebsd_gpui", Path(__file__).with_name("prepare-freebsd-gpui.py")
)
helper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(helper)

# A deliberately small synthetic fixture, not a substitute for the pinned hash.
FIXTURE = b'''#[cfg(any(
    test,
    target_os = "windows",
    target_os = "linux",
    target_family = "wasm",
    feature = "test-support"
))]
#[expect(missing_docs)]
pub mod queue;
#[cfg(any(target_os = "windows", target_os = "linux", target_family = "wasm"))]
pub use queue::{PriorityQueueReceiver, PriorityQueueSender};
'''


class FreeBSDGPUICompatibilityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.desktop = self.root / "desktop"
        self.desktop.mkdir()
        (self.desktop / "Cargo.toml").write_text('[package]\nname = "fixture"\n')
        self.lock = self.desktop / "Cargo.lock"
        self.lock.write_text(f'[[package]]\nname = "gpui"\nsource = "{helper.SOURCE}"\n')
        self.home = self.root / "isolated-cargo"

    def prepare(self, verify_only=False):
        with patch.object(helper.platform, "system", return_value="FreeBSD"):
            helper.prepare(self.desktop, self.home, helper.TARGET, verify_only)

    def fetched_source(self, *_args, **kwargs):
        self.assertEqual(kwargs["env"]["CARGO_HOME"], str(self.home))
        self.assertIn("--locked", _args[0])
        self.assertEqual(_args[0][-2:], ["--target", helper.TARGET])
        path = self.home / "git/checkouts/zed-fixture/bc538de" / helper.RELATIVE_SOURCE
        path.parent.mkdir(parents=True)
        path.write_bytes(FIXTURE)
        return path

    def test_only_two_freebsd_conditions_added(self):
        result = helper.add_freebsd_cfg(FIXTURE)
        self.assertEqual(result.count(b'target_os = "freebsd"'), 2)
        self.assertIn(b"pub mod queue;", result)
        self.assertIn(b"pub use queue::{PriorityQueueReceiver, PriorityQueueSender};", result)
        for old, new in reversed(helper.EDITS):
            result = result.replace(new, old)
        self.assertEqual(result, FIXTURE)

    def test_missing_duplicate_or_already_patched_context_rejected(self):
        for data in (b"", FIXTURE * 2, helper.add_freebsd_cfg(FIXTURE)):
            with self.subTest(data=data), self.assertRaisesRegex(ValueError, "exactly once"):
                helper.add_freebsd_cfg(data)

    def test_source_hash_drift_is_rejected_without_write(self):
        source = self.root / "gpui.rs"
        source.write_bytes(FIXTURE)
        with self.assertRaisesRegex(ValueError, "source drift"):
            helper.patch_source(source)
        self.assertEqual(source.read_bytes(), FIXTURE)

    def test_output_hash_drift_is_rejected_without_write(self):
        source = self.root / "gpui.rs"
        source.write_bytes(FIXTURE)
        with patch.object(helper, "BEFORE_SHA256", hashlib.sha256(FIXTURE).hexdigest()):
            with self.assertRaisesRegex(ValueError, "patch drift"):
                helper.patch_source(source)
        self.assertEqual(source.read_bytes(), FIXTURE)

    def test_source_links_are_rejected(self):
        source = self.root / "gpui.rs"
        source.write_bytes(FIXTURE)
        for link_type in ("symlink", "hardlink"):
            link = self.root / link_type
            if link_type == "symlink":
                link.symlink_to(source)
            else:
                os.link(source, link)
            with self.assertRaisesRegex(ValueError, "linked"):
                helper.patch_source(link)
        self.assertEqual(source.read_bytes(), FIXTURE)

    def test_non_freebsd_host_or_target_rejected_before_creation(self):
        for host, target in (("Linux", helper.TARGET), ("FreeBSD", "x86_64-unknown-linux-gnu")):
            with patch.object(helper.platform, "system", return_value=host):
                with self.assertRaisesRegex(ValueError, "native FreeBSD"):
                    helper.prepare(self.desktop, self.home, target)
            self.assertFalse(self.home.exists())

    def test_lock_revision_drift_rejected_before_creation(self):
        self.lock.write_text(self.lock.read_text().replace(helper.REVISION, "0" * 40))
        with self.assertRaisesRegex(ValueError, "exact reviewed"):
            self.prepare()
        self.assertFalse(self.home.exists())

    def test_existing_home_and_symlink_rejected(self):
        self.home.mkdir()
        with self.assertRaises(FileExistsError):
            self.prepare()
        self.home.rmdir()
        self.home.symlink_to(self.root / "absent", target_is_directory=True)
        with self.assertRaises(FileExistsError):
            self.prepare()

    def test_checkout_revision_drift_rejected(self):
        with patch.object(helper.subprocess, "run", side_effect=self.fetched_source):
            with patch.object(helper.subprocess, "check_output", return_value="0" * 40):
                with self.assertRaisesRegex(ValueError, "checkout revision"):
                    self.prepare()

    def test_missing_checkout_rejected(self):
        with patch.object(helper.subprocess, "run"):
            with self.assertRaisesRegex(ValueError, "found 0"):
                self.prepare()

    def test_verify_only_does_not_fetch_or_write(self):
        source = self.home / "git/checkouts/zed-fixture/bc538de" / helper.RELATIVE_SOURCE
        source.parent.mkdir(parents=True)
        source.write_bytes(helper.add_freebsd_cfg(FIXTURE))
        before = source.stat().st_mtime_ns
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        with patch.object(helper.subprocess, "run") as fetch:
            outputs = [helper.REVISION, b"crates/gpui/src/gpui.rs\0"] * 2
            with patch.object(helper.subprocess, "check_output", side_effect=outputs):
                with patch.object(helper, "AFTER_SHA256", digest):
                    self.prepare(verify_only=True)
                with self.assertRaisesRegex(ValueError, "patched source drift"):
                    self.prepare(verify_only=True)
            fetch.assert_not_called()
        self.assertEqual(source.stat().st_mtime_ns, before)

    def test_verify_only_rejects_other_tracked_changes(self):
        source = self.home / "git/checkouts/zed-fixture/bc538de" / helper.RELATIVE_SOURCE
        source.parent.mkdir(parents=True)
        source.write_bytes(helper.add_freebsd_cfg(FIXTURE))
        for changed in (b"", b"crates/gpui/src/gpui.rs\0Cargo.toml\0"):
            with patch.object(helper.subprocess, "check_output", side_effect=[helper.REVISION, changed]):
                with self.assertRaisesRegex(ValueError, "tracked diff"):
                    self.prepare(verify_only=True)

    def test_duplicate_checkout_rejected(self):
        for name in ("zed-one", "zed-two"):
            path = self.home / f"git/checkouts/{name}/bc538de" / helper.RELATIVE_SOURCE
            path.parent.mkdir(parents=True)
            path.write_bytes(FIXTURE)
        with self.assertRaisesRegex(ValueError, "found 2"):
            self.prepare(verify_only=True)

    def test_checkout_escape_rejected(self):
        path = self.home / "git/checkouts/zed-fixture/bc538de" / helper.RELATIVE_SOURCE
        path.parent.mkdir(parents=True)
        outside = self.root / "shared.rs"
        outside.write_bytes(FIXTURE)
        path.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "escaped"):
            self.prepare(verify_only=True)
        self.assertEqual(outside.read_bytes(), FIXTURE)

    def test_isolated_fetch_and_patch_sequence(self):
        # Only this synthetic test substitutes fixture digests. The optional
        # real-source test below exercises the exact production digest pair.
        expected = helper.add_freebsd_cfg(FIXTURE)
        with patch.object(helper.subprocess, "run", side_effect=self.fetched_source):
            with patch.object(helper.subprocess, "check_output", return_value=helper.REVISION):
                with patch.object(helper, "BEFORE_SHA256", hashlib.sha256(FIXTURE).hexdigest()):
                    with patch.object(helper, "AFTER_SHA256", hashlib.sha256(expected).hexdigest()):
                        self.prepare()
        source = next(self.home.glob("git/checkouts/zed-*/*/crates/gpui/src/gpui.rs"))
        self.assertEqual(source.read_bytes(), expected)
        self.assertEqual(self.home.stat().st_mode & 0o777, 0o700)

    @unittest.skipUnless(os.environ.get("GPUI_PINNED_SOURCE"), "set GPUI_PINNED_SOURCE for exact upstream test")
    def test_real_pinned_source_copy(self):
        original = Path(os.environ["GPUI_PINNED_SOURCE"])
        original_bytes = original.read_bytes()
        source = self.root / "gpui.rs"
        source.write_bytes(original_bytes)
        helper.patch_source(source)
        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), helper.AFTER_SHA256)
        self.assertEqual(original.read_bytes(), original_bytes)
        with self.assertRaisesRegex(ValueError, "source drift"):
            helper.patch_source(source)


if __name__ == "__main__":
    unittest.main()
