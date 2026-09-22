#!/usr/bin/env python3
"""Native transcript drag/copy/paste acceptance on screenshot.py's private Xvfb.

Run after building: python3 scripts/transcript_selection_acceptance.py target/selection.png
Only the existing binary runs. No live display, clipboard, daemon or credentials
are used. Produces baseline, highlighted and pasted screenshots plus JSON proof.
Requires screenshot.py dependencies, Pillow, tesseract and GTK3/PyGObject.
"""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time

from PIL import Image, ImageChops

import fresh_session_acceptance
import screenshot

TEXT = "Native transcript selection copies exactly"
CLIPBOARD_READ = """import gi, sys
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk
text = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()
if text is None: raise SystemExit('clipboard has no text')
sys.stdout.write(text)
"""


def verify(output, env, root):
    assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1"
    assert env.get("HOME") == str(root / "home")

    def command(*args):
        return subprocess.check_output(args, env=env, cwd=root, timeout=15)

    def native(*args):
        command("xdotool", *map(str, args))

    def capture(label):
        time.sleep(.5)
        path = output.with_name(output.stem + "-" + label + ".png")
        command("import", "-window", "root", "png:" + str(path))
        return path, Image.open(path).convert("RGB")

    def words(path):
        data = command("tesseract", str(path), "stdout", "--psm", "11", "tsv").decode()
        return [row for row in csv.DictReader(io.StringIO(data), delimiter="\t")
                if row["text"].strip()]

    def locate(path, minimum_y=0, maximum_y=1000):
        rows = [row for row in words(path) if minimum_y <= int(row["top"]) < maximum_y]
        expected = TEXT.split()
        for start in range(len(rows) - len(expected) + 1):
            group = rows[start:start + len(expected)]
            if [row["text"].rstrip("|") for row in group] == expected:
                return (min(int(row["left"]) for row in group),
                        min(int(row["top"]) for row in group),
                        max(int(row["left"]) + int(row["width"]) for row in group),
                        max(int(row["top"]) + int(row["height"]) for row in group))
        raise AssertionError("Expected rendered text absent: " + " ".join(row["text"] for row in rows))

    initial = Image.open(output).convert("RGB")
    composer = fresh_session_acceptance.composer_bounds(initial)
    native("mousemove", composer[0] + 32, composer[1] + 28, "click", 1)
    native("type", "--clearmodifiers", "--delay", 15, TEXT)
    native("key", "Return")
    baseline_path, baseline = capture("baseline")
    left, top, right, bottom = locate(baseline_path, maximum_y=250)
    y = (top + bottom) // 2
    # Start in the first glyph's left half, inside the selectable block. Drag
    # to the outer window margin, beyond the transcript leaf and panel.
    start = (left + 1, y)
    end = (baseline.width - 2, y)
    native("mousemove", *start, "mousedown", 1)
    time.sleep(.2)
    for step in range(1, 13):
        native("mousemove", round(start[0] + (end[0] - start[0]) * step / 12), y)
        time.sleep(.035)
    native("mouseup", 1)
    highlight_path, highlighted = capture("highlighted")
    native("key", "--clearmodifiers", "ctrl+c")
    copied = command("/usr/bin/python3", "-c", CLIPBOARD_READ).decode()
    assert copied == TEXT, ("Transcript clipboard mismatch", copied, TEXT)
    crop = (left, top - 3, right, bottom + 3)
    difference = ImageChops.difference(baseline.crop(crop), highlighted.crop(crop))
    pixels = difference.load()
    changed = sum(max(pixels[x, y]) >= 3
                  for y in range(difference.height) for x in range(difference.width))
    total = difference.width * difference.height
    assert changed > total * .25, ("Selection did not visibly repaint text background", changed, total)
    native("mousemove", composer[0] + 32, composer[1] + 28, "click", 1)
    native("key", "--clearmodifiers", "ctrl+v")
    pasted_path, _ = capture("pasted")
    locate(pasted_path, minimum_y=composer[1], maximum_y=composer[3])
    native("key", "--clearmodifiers", "ctrl+a", "ctrl+c")
    pasted = command("/usr/bin/python3", "-c", CLIPBOARD_READ).decode()
    assert pasted == TEXT, ("Composer paste roundtrip mismatch", pasted, TEXT)
    evidence = {"passed": True, "expected": TEXT, "copied": copied,
                "pasted_roundtrip": pasted, "drag_start": start, "drag_end": end,
                "text_bounds": [left, top, right, bottom], "highlight_changed_pixels": changed,
                "highlight_crop_pixels": total, "highlighted_screenshot": str(highlight_path),
                "baseline_screenshot": str(baseline_path), "pasted_screenshot": str(pasted_path)}
    output.with_suffix(".selection.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path,
                        default=Path(__file__).resolve().parents[1] / "target/debug/jcode-desktop")
    args = parser.parse_args()
    if not shutil.which("tesseract"):
        parser.error("tesseract is required")
    binary_sha256 = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    original, argv = fresh_session_acceptance.verify, sys.argv
    try:
        fresh_session_acceptance.verify = verify
        sys.argv = [screenshot.__file__, str(args.output), "--binary", str(args.binary),
                    "--no-build", "--fresh-interact", "--transcript", "empty"]
        screenshot.main()
        evidence_path = args.output.with_suffix(".selection.json")
        evidence = json.loads(evidence_path.read_text())
        assert hashlib.sha256(args.binary.read_bytes()).hexdigest() == binary_sha256, "Binary changed during acceptance"
        evidence["binary_sha256"] = binary_sha256
        evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
    finally:
        fresh_session_acceptance.verify, sys.argv = original, argv


if __name__ == "__main__":
    main()
