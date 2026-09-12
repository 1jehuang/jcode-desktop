"""Native login acceptance on screenshot.py's isolated offline X11 display.

Only dummy credentials are used. No submission/authentication or harness actions.
Evidence is native clicks, OCR, masked glyph pixels, and restored draft pixels.
"""
import json
import subprocess
import time

from PIL import Image

from fresh_session_acceptance import composer_bounds
from model_picker_acceptance import normalized, parse_words, phrase_bounds

DRAFT = "Keep this draft safe"
SECRET = "TEST-SECRET-7a9b2c4d6e8f"
CLIPBOARD_OWNER = """
import gi, pathlib, sys
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk
clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
clipboard.set_text(sys.argv[1], -1)
pathlib.Path(sys.argv[2]).touch()
Gtk.main()
"""


def visible_words(tsv, bounds):
    # Standalone carets/punctuation must not inflate the next button bounds.
    return [word for word in parse_words(tsv, bounds) if normalized(word["text"])]


def masked_glyphs(image, bounds, expected):
    """Require a row of separate, dot-sized ink runs rather than blank/redacted OCR."""
    image = image.convert("RGB")
    left, top, right, bottom = map(round, bounds)
    runs = []
    start = None
    for x in range(left, right):
        ink = any(min(image.getpixel((x, y))) > 150 for y in range(top, bottom))
        if ink and start is None:
            start = x
        if not ink and start is not None:
            runs.append((start, x))
            start = None
    if start is not None:
        runs.append((start, right))
    dots = [(a, b) for a, b in runs if 2 <= b - a <= 8]
    assert expected <= len(dots) <= expected + 1, f"Expected {expected} masked dots, saw {len(dots)} runs: {runs}"
    heights = []
    for a, b in dots[:expected]:
        ys = [y for y in range(top, bottom)
              if any(min(image.getpixel((x, y))) > 150 for x in range(a, b))]
        heights.append(max(ys) - min(ys) + 1)
    assert max(heights) <= 9, f"Secret glyphs are not dot-sized: {heights}"
    return {"dot_count": len(dots), "heights": heights}


def verify(output, env, root):
    assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1", "Offline fixture required"
    assert env.get("XDG_RUNTIME_DIR") == str(root / "runtime"), "Private runtime required"
    assert env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"), "Private X11 required"
    report = {"checks": {}, "scope": "offline native login navigation, masking and draft restoration; no authentication"}
    stage = "initial"
    clipboard = None

    def navigation():
        for line in (root / "state").read_text().splitlines():
            if line.startswith("navigation="):
                return json.loads(line.split("=", 1)[1])
        raise AssertionError("Missing navigation state")

    def panels():
        return [panel for row in navigation()["rows"] for panel in row["panels"]
                if not panel["closing"]]

    def native(*args):
        subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root, check=True, timeout=15)

    def capture(label):
        path = output.with_name(output.stem + "-" + label + ".png")
        time.sleep(.3)
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=env, cwd=root, check=True, timeout=15)
        return Image.open(path).convert("RGB")

    def ocr(image, label):
        bounds = (280, 60, image.width - 12, image.height)
        path = output.with_name(output.stem + "-" + label + "-ocr.png")
        crop = image.crop(bounds)
        crop.resize((crop.width * 3, crop.height * 3)).save(path)
        tsv = subprocess.check_output(["tesseract", str(path), "stdout", "--psm", "11", "tsv"],
                                      env=env, cwd=root, timeout=20, stderr=subprocess.DEVNULL).decode()
        return visible_words(tsv, bounds)

    def wait_words(label, phrase):
        deadline = time.monotonic() + 12
        while True:
            image = capture(label)
            words = ocr(image, label)
            try:
                return image, words, phrase_bounds(words, phrase)
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise

    def click(bounds):
        native("mousemove", round((bounds[0] + bounds[2]) / 2),
               round((bounds[1] + bounds[3]) / 2), "click", "1")

    try:
        initial = Image.open(output).convert("RGB")
        bounds = composer_bounds(initial)
        click((bounds[0] + 20, bounds[1] + 12, bounds[0] + 80, bounds[1] + 36))
        native("type", "--clearmodifiers", "--delay", "25", DRAFT + "  ")
        _, words, _ = wait_words("draft", DRAFT)
        stage = "native-connect-account-button"
        source = panels()[0]
        click(phrase_bounds(words, "Connect account"))
        provider_image, words, _ = wait_words("providers", "Choose an account to connect")
        opened = panels()
        assert len(opened) == 2 and opened[0]["id"] == source["id"], opened
        assert opened[1]["session"].startswith("accounts://"), opened
        assert opened[1]["focused"] and navigation()["keyboard_panel"] == opened[1]["slot"], navigation()
        assert opened[0]["history_items"] == source["history_items"], opened
        report["checks"]["separate_right_panel_focused_source_unchanged"] = True
        report["checks"][stage] = True

        stage = "color-coded-account-statuses"
        # Inspect actual colored badge pixels, not only a text-only status model.
        # The right-hand account panel occupies the right half of the canvas.
        pixels = list(provider_image.crop((850, 170, 1420, 850)).getdata())
        green = sum(g > r * 1.18 and g > b * 1.08 and g > 90 for r, g, b in pixels)
        red = sum(r > g * 1.25 and r > b * 1.1 and r > 110 for r, g, b in pixels)
        assert green > 25 and red > 25, {"green": green, "red": red}
        report["checks"][stage] = {"green_pixels": green, "red_pixels": red}

        stage = "native-openai-api-key-choice"
        for attempt in range(12):
            try:
                api_choice = phrase_bounds(words, "OpenAI API")
                break
            except AssertionError:
                native("mousemove", 1200, 740, "click", "--repeat", 3, "--delay", 80, "5")
                words = ocr(capture("providers-scrolled"), "providers-scrolled")
        else:
            raise AssertionError("OpenAI API account was not reachable by scrolling")
        click(api_choice)
        empty, words, placeholder = wait_words("api-key", "Paste your API key")
        report["checks"][stage] = True

        stage = "native-clipboard-paste-masked-pixels"
        helper = root / "login-clipboard-owner.py"
        helper.write_text(CLIPBOARD_OWNER)
        ready = root / "login-clipboard-ready"
        clipboard = subprocess.Popen(["/usr/bin/python3", str(helper), SECRET, str(ready)], env=env, cwd=root)
        deadline = time.monotonic() + 10
        while not ready.exists():
            if clipboard.poll() is not None or time.monotonic() >= deadline:
                raise AssertionError("Private clipboard failed to start (GTK3/PyGObject required)")
            time.sleep(.05)
        click(phrase_bounds(words, "Paste from clipboard"))
        pasted = capture("masked")
        input_bounds = (placeholder[0] - 2, placeholder[1] - 6,
                        min(empty.width - 35, placeholder[0] + 500), placeholder[3] + 6)
        report["mask"] = masked_glyphs(pasted, input_bounds, len(SECRET))
        pasted_words = ocr(pasted, "masked")
        text = normalized(" ".join(word["text"] for word in pasted_words))
        assert normalized(SECRET) not in text and "testsecret" not in text, "Secret rendered in plaintext"
        assert "pasteyourapikey" not in text, "Paste left the input empty"
        pasted.save(output)
        report["checks"][stage] = True

        stage = "native-close-restores-draft"
        click(phrase_bounds(pasted_words, "Close"))
        _, words, _ = wait_words("restored", DRAFT)
        assert len(panels()) == 1 and panels()[0]["id"] == source["id"], panels()
        assert panels()[0]["focused"], panels()
        text = normalized(" ".join(word["text"] for word in words))
        assert "chooseanaccount" not in text and "pastefromclipboard" not in text, "Login overlay stayed open"
        assert normalized(SECRET) not in text, "Secret leaked into the restored composer"
        report["checks"][stage] = True
        report["passed"] = True
        print("Login acceptance passed: native account/provider clicks, clipboard paste, masked pixels, restored draft")
    except Exception as error:
        report.update(passed=False, failed_stage=stage, error=str(error))
        raise
    finally:
        if clipboard is not None:
            clipboard.terminate()
            try:
                clipboard.wait(timeout=5)
            except subprocess.TimeoutExpired:
                clipboard.kill()
                clipboard.wait()
        output.with_suffix(".login.json").write_text(json.dumps(report, indent=2) + "\n")
