#!/usr/bin/env python3
"""Fast, no-display/no-network safety checks for native voice acceptance."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("native_voice", Path(__file__).with_name("accept-voice-native.py"))
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


class NativeVoiceSafetyTests(unittest.TestCase):
    def test_app_environment_denies_ambient_sockets_and_daemon_path(self):
        root = Path("/private/evidence")
        env = native.native_env(root, ":999", Path("/usr/share/lvp.json"))
        self.assertNotIn("JCODE_DESKTOP_SCREENSHOT", env)
        self.assertNotIn("NARI_API_KEY", env)
        self.assertNotIn("WAYLAND_DISPLAY", env)
        self.assertEqual(env["PATH"], str(root / "empty-bin"))
        self.assertEqual(env["JCODE_SOCKET"], str(root / "runtime/absent-daemon.sock"))
        self.assertEqual(env["JCODE_API_SOCKET"], str(root / "runtime/absent-api.sock"))
        self.assertEqual(env["ALSA_CONFIG_PATH"], str(root / "asound.conf"))

    def test_sandbox_has_private_device_and_process_namespaces(self):
        args = native.sandbox(Path("/private/evidence"))
        pairs = list(zip(args, args[1:]))
        self.assertIn(("--dev", "/dev"), pairs)
        self.assertIn("--unshare-pid", args)
        self.assertIn("--die-with-parent", args)
        self.assertNotIn("/dev/snd", args)
        self.assertNotIn("/run", args)
        self.assertNotIn("/home", args)
        self.assertNotIn("--share-pid", args)

    def test_only_one_unsubmitted_startup_draft_is_accepted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            file = root / "logs/jcode-desktop/crash-recovery.json"
            file.parent.mkdir(parents=True)
            panel = {"session_id": "startup://draft", "draft": {"content": native.PREFIX}}
            payload = {"snapshot": {"slots": [{"panel": panel}]}}
            file.write_text(json.dumps(payload))
            self.assertEqual(native.draft_snapshot(root), panel)
            payload["snapshot"]["slots"].append({"panel": panel})
            file.write_text(json.dumps(payload))
            self.assertIsNone(native.draft_snapshot(root))
            file.write_text("incomplete")
            self.assertIsNone(native.draft_snapshot(root))

    def test_expected_text_normalization_does_not_depend_on_punctuation(self):
        self.assertEqual(native.words("Hello. Welcome to Nari Labs."), native.EXPECTED)

    def test_unverified_audio_is_rejected_before_creating_a_device(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "unverified.pcm"
            source.write_bytes(bytes(320))
            with self.assertRaisesRegex(ValueError, "verified public"):
                native.VirtualAlsa(root / "audio", source)
            self.assertFalse((root / "audio/capture-drain.fifo").exists())

    def test_alsa_path_cannot_inject_additional_device_configuration(self):
        with self.assertRaisesRegex(ValueError, "unsafe ALSA"):
            native.VirtualAlsa(Path('/invalid/"injected'), Path("not-read"))

    def test_cloud_requires_explicit_consent_before_creating_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "not-created"
            result = subprocess.run([sys.executable, native.__file__, str(root),
                                     "--binary", "/not-executed"], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"cloud mode requires", result.stderr)
            self.assertFalse(root.exists())

    def test_prepare_mode_cannot_implicitly_enable_cloud(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "not-created"
            result = subprocess.run([sys.executable, native.__file__, str(root),
                                     "--binary", "/not-executed", "--prepare-only", "--allow-cloud"],
                                    capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"choose either", result.stderr)
            self.assertFalse(root.exists())


if __name__ == "__main__":
    unittest.main()
