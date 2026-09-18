"""Static voice permission checks, safe to run without a microphone or macOS."""
import pathlib
import plistlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class VoicePackageTests(unittest.TestCase):
    def test_macos_explains_recording_and_external_transcription(self):
        info = plistlib.loads((ROOT / "packaging/macos/Info.plist.in").read_bytes())
        purpose = info["NSMicrophoneUsageDescription"]
        self.assertIn("when you choose Voice", purpose)
        self.assertIn("Groq", purpose)
        self.assertIn("subscription", purpose)

    def test_hardened_runtime_allows_explicit_microphone_capture(self):
        entitlements = plistlib.loads((ROOT / "packaging/macos/Jcode.entitlements").read_bytes())
        self.assertIs(entitlements["com.apple.security.device.audio-input"], True)


if __name__ == "__main__":
    unittest.main()
