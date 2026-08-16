import argparse
import base64
import importlib.util
import plistlib
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "render_macos_plist", ROOT / "scripts" / "render-macos-plist.py"
)
RENDERER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(RENDERER)


class RenderMacOSPlistTests(unittest.TestCase):
    def render(self, **overrides):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        output = Path(temporary.name) / "Info.plist"
        values = {
            "template": ROOT / "packaging" / "macos" / "Info.plist.in",
            "output": output,
            "version": "desktop-v1.2.3-beta.4",
            "build": "42",
            "public_key": "",
            "feed_url": "",
            "require_updates": False,
        }
        values.update(overrides)
        RENDERER.render(argparse.Namespace(**values))
        with output.open("rb") as rendered:
            return plistlib.load(rendered)

    def test_local_build_omits_incomplete_update_configuration(self):
        info = self.render()
        self.assertEqual(info["CFBundleShortVersionString"], "1.2.3")
        self.assertEqual(info["CFBundleVersion"], "42")
        self.assertNotIn("SUFeedURL", info)
        self.assertNotIn("SUPublicEDKey", info)

    def test_release_enables_signed_automatic_updates(self):
        public_key = base64.b64encode(bytes(range(32))).decode()
        info = self.render(
            public_key=public_key,
            feed_url="https://github.com/1jehuang/jcode-desktop/releases/download/desktop-updates/appcast.xml",
            require_updates=True,
        )
        self.assertEqual(info["SUPublicEDKey"], public_key)
        self.assertTrue(info["SUEnableAutomaticChecks"])
        self.assertTrue(info["SUAutomaticallyUpdate"])
        self.assertEqual(info["SUScheduledCheckInterval"], 86400)

    def test_release_rejects_missing_update_credentials(self):
        with self.assertRaisesRegex(ValueError, "required"):
            self.render(require_updates=True)

    def test_rejects_insecure_feed(self):
        with self.assertRaisesRegex(ValueError, "HTTPS"):
            self.render(
                public_key=base64.b64encode(bytes(32)).decode(),
                feed_url="http://example.com/appcast.xml",
            )

    def test_rejects_malformed_public_key(self):
        with self.assertRaisesRegex(ValueError, "32-byte"):
            self.render(public_key="YWJj", feed_url="https://example.com/appcast.xml")

    def test_rejects_non_numeric_build(self):
        with self.assertRaisesRegex(ValueError, "positive integer"):
            self.render(build="beta.1")


if __name__ == "__main__":
    unittest.main()
