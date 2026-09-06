"""Verify distinct transcript/clipboard images and stable paints on private X11."""
import subprocess
import time

from PIL import Image


CLIPBOARD_OWNER = """import gi, sys
from pathlib import Path
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk, GdkPixbuf
clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
clipboard.set_image(GdkPixbuf.Pixbuf.new_from_file(sys.argv[1]))
Path(sys.argv[2]).touch()
Gtk.main()
"""


def color_count(image, color):
    return sum(count for count, pixel in image.getcolors(image.width * image.height)
               if all(abs(actual - expected) <= 3 for actual, expected in zip(pixel[:3], color)))


def verify(output, env, root):
    """screenshot.py supplies an isolated environment and an image transcript."""
    red = (240, 20, 60)
    blue = (92, 124, 173)
    fixture = root / "clipboard-red.png"
    Image.new("RGB", (100, 100), red).save(fixture)
    helper = root / "clipboard-owner.py"
    helper.write_text(CLIPBOARD_OWNER)
    ready = root / "clipboard-ready"
    clipboard = subprocess.Popen(
        ["python3", str(helper), str(fixture), str(ready)], env=env, cwd=root,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )

    def command(*args):
        subprocess.run(args, env=env, cwd=root, check=True, timeout=15)

    def capture(suffix):
        path = output.with_name(output.stem + suffix + ".png")
        command("import", "-window", "root", "png:" + str(path))
        return Image.open(path).convert("RGB")

    def wait_for(label, predicate):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            image = capture("-" + label)
            if predicate(image):
                return image
            time.sleep(.2)
        raise AssertionError(f"Image cache acceptance timed out: {label}")

    try:
        deadline = time.monotonic() + 10
        while not ready.exists():
            if clipboard.poll() is not None or time.monotonic() > deadline:
                raise AssertionError("Private image clipboard failed to start (requires GTK3/PyGObject)")
            time.sleep(.05)
        wait_for("loaded", lambda image: color_count(image.crop((280, 80, 1440, 750)), blue) > 1000)
        command("xdotool", "mousemove", "600", "950", "click", "1", "key", "ctrl+v")
        # Regression: both old independent source counters assigned ID 1. The
        # composer showed the existing chart instead of the newly pasted red PNG.
        wait_for("pasted", lambda image: color_count(image.crop((280, 750, 1440, 1000)), red) > 1000)
        for frame in range(8):
            image = capture(f"-attachment-frame-{frame}")
            assert color_count(image.crop((280, 750, 1440, 1000)), red) > 1000, "attachment disappeared or changed image"
            assert color_count(image.crop((280, 80, 1440, 750)), blue) > 1000, "original transcript image changed"
            time.sleep(.12)
        command("xdotool", "type", "--delay", "0", "Inspect this red attachment")
        command("xdotool", "key", "Return")
        wait_for("submitted", lambda image: color_count(image.crop((280, 80, 1440, 900)), red) > 80000)
        counts = []
        for frame in range(8):
            image = capture(f"-transcript-frame-{frame}")
            count = color_count(image.crop((280, 80, 1440, 900)), red)
            assert count > 80000, "submitted image disappeared or changed image"
            counts.append(count)
            time.sleep(.12)
        assert max(counts) - min(counts) < 10, f"image geometry did not settle: {counts}"
        diagnostics = (root / "logs/jcode-desktop/jcode-desktop.log").read_text()
        assert diagnostics.count("desktop-image decode-start") == 2, "submission decoded an already cached image again"
        assert "desktop-image paint-state" in diagnostics, "image paint tracking was not active"
        for warning in ("ready-to-pending", "texture-changed", "became-error", "panel_geometry_oscillation"):
            assert warning not in diagnostics, f"unexpected flicker diagnostic: {warning}"
        print(f"Image cache acceptance passed: distinct clipboard/transcript images, 16 stable image frames, submitted red pixels={counts}")
    finally:
        clipboard.terminate()
        try:
            clipboard.wait(timeout=5)
        except subprocess.TimeoutExpired:
            clipboard.kill()
            clipboard.wait()
        log = root / "logs/jcode-desktop/jcode-desktop.log"
        if log.exists():
            output.with_suffix(".image-diagnostics.log").write_text(log.read_text())
