#!/usr/bin/env python3
"""Native screenshot-paste acceptance on screenshot.py's private Xvfb display.

Usage (after building the desktop binary):
  python3 scripts/paste_preview_acceptance.py target/paste-right.png --no-build
  python3 scripts/paste_preview_acceptance.py target/paste-left.png --no-build --side left
  python3 scripts/paste_preview_acceptance.py target/paste-compact.png --no-build --side compact

No production clipboard, display, settings, or credentials are used. The existing
launcher is imported and its image-cache verifier is temporarily replaced, never
edited. Artifacts include opening/flight/settled PNGs and timestamped JSON evidence.
Requires the launcher's dependencies plus GTK3/PyGObject, Pillow and tesseract.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time

from PIL import Image, ImageChops

import image_cache_acceptance
import screenshot


RED = (240, 20, 60)
DRAFT = "Preserve my screenshot draft"


def red_bounds(image):
    """Find the pasted fixture without OCR, tolerant of minor color conversion."""
    difference = ImageChops.difference(image.convert("RGB"), Image.new("RGB", image.size, RED))
    channels = difference.split()
    maximum = ImageChops.lighter(ImageChops.lighter(channels[0], channels[1]), channels[2])
    mask = maximum.point(lambda value: 255 if value <= 5 else 0)
    return mask.getbbox(), mask


def geometry(state):
    """Only stable geometry/identity fields, excluding unrelated paint counters."""
    navigation = json.loads(next(line.split("=", 1)[1] for line in state.splitlines()
                                 if line.startswith("navigation=")))
    row = navigation["rows"][navigation["active_row"]]
    return {"active_row": navigation["active_row"],
            "focused_slot": navigation["focused_slot"],
            "keyboard_panel": navigation["keyboard_panel"],
            "viewport": navigation["viewport"],
            "canvas_width": navigation["canvas_width"],
            "camera": row["camera"], "camera_target": row["camera_target"],
            "panels": [(panel["id"], panel["width"]) for panel in row["panels"]]}


def verify(output, env, root, panels=2, side="right"):
    assert panels == (1 if side == "compact" else 2), "unexpected fixture panel count"
    assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1" and env.get("HOME") == str(root / "home"), "refusing non-isolated environment"
    fixture = root / "paste-fixture.png"
    Image.new("RGB", (640, 520), RED).save(fixture)
    helper = root / "paste-clipboard-owner.py"
    helper.write_text(image_cache_acceptance.CLIPBOARD_OWNER)
    ready = root / "paste-clipboard-ready"
    clipboard = subprocess.Popen(["python3", str(helper), str(fixture), str(ready)],
                                 env=env, cwd=root, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.PIPE)
    capture_helper = root / "paste-capture.py"
    capture_helper.write_text("""import gi, sys
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk
root = Gdk.get_default_root_window()
for line in sys.stdin:
    pixbuf = Gdk.pixbuf_get_from_window(root, 0, 0, root.get_width(), root.get_height())
    data = pixbuf.get_pixels()
    header = f'{pixbuf.get_width()} {pixbuf.get_height()} {pixbuf.get_rowstride()} {len(data)}\\n'
    sys.stdout.buffer.write(header.encode())
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
""")
    recorder = subprocess.Popen(["python3", str(capture_helper)], env=env, cwd=root,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    frames = []
    evidence = {"side": side, "draft": DRAFT, "frames": []}

    def command(*args):
        return subprocess.run(args, env=env, cwd=root, check=True, timeout=15,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def capture():
        # Persistent native GDK capture avoids ImageMagick's ~1.5s startup per
        # frame. No compression or file writes occur during the 420ms flight.
        recorder.stdin.write(b"capture\n")
        recorder.stdin.flush()
        header = recorder.stdout.readline()
        assert header, "private GTK screenshot recorder exited unexpectedly"
        width, height, stride, length = map(int, header.split())
        data = recorder.stdout.read(length)
        assert len(data) == length, "incomplete private GTK screenshot"
        return Image.frombytes("RGB", (width, height), data, "raw", "RGB", stride)

    def save(image, name):
        path = output.with_name(output.stem + "-" + name + ".png")
        image.save(path)
        return path

    def draft_text(image, left, right, name):
        crop = image.crop((left, 800, right, image.height)).resize(((right-left)*2, (image.height-800)*2))
        path = save(crop, name)
        text = command("tesseract", str(path), "stdout", "--psm", "6").stdout.decode()
        # The native caret can be recognized as punctuation inside the last
        # word. Keep every alphanumeric character but ignore OCR separators.
        return "".join(character for character in text.lower() if character.isalnum())

    try:
        deadline = time.monotonic() + 10
        while not ready.exists():
            if clipboard.poll() is not None or time.monotonic() > deadline:
                error = clipboard.stderr.read().decode() if clipboard.poll() is not None else "timeout"
                raise AssertionError("GTK image clipboard failed: " + error)
            time.sleep(.03)
        # Native clicks select the desired panel and its real composer.
        if side == "compact":
            window = command("xdotool", "getactivewindow").stdout.decode().strip()
            command("xdotool", "windowstate", "--remove", "MAXIMIZED_VERT", window)
            command("xdotool", "windowstate", "--remove", "MAXIMIZED_HORZ", window)
            command("xdotool", "windowsize", "--sync", window, "800", "1000")
            command("xdotool", "windowmove", "--sync", window, "0", "0")
            time.sleep(.7)
        x = 560 if side == "right" else (400 if side == "compact" else 1130)
        left, right = (276, 850) if side == "right" else ((0, 800) if side == "compact" else (855, 1430))
        command("xdotool", "mousemove", str(x), "950", "click", "1")
        time.sleep(.5)
        # Leave the caret separated from the final letter for deterministic OCR.
        command("xdotool", "type", "--delay", "0", DRAFT + "  ")
        command("xdotool", "mousemove", "20", "980")
        time.sleep(.3)
        before = capture()
        save(before, "before")
        baseline = geometry((root / "state").read_text())
        if side == "compact":
            assert baseline["viewport"][0] == 800, "private compact resize failed"
        evidence["geometry"] = baseline
        assert DRAFT.lower().replace(" ", "") in draft_text(before, left, right, "draft-before").lower(), "draft did not reach intended composer"
        assert red_bounds(before)[0] is None, "red fixture color already present before paste"
        start = time.monotonic()
        command("xdotool", "key", "--clearmodifiers", "ctrl+v")
        while time.monotonic() - start < 2.3:
            image = capture()
            elapsed = time.monotonic() - start
            bounds, _ = red_bounds(image)
            current = geometry((root / "state").read_text())
            frames.append((elapsed, image, bounds))
            evidence["frames"].append({"seconds": round(elapsed, 4), "red_bounds": bounds, "geometry": current})
            assert current == baseline, f"paste moved active panel or stole focus at {elapsed:.3f}s: {current} != {baseline}"
            time.sleep(.015)
        for index, (elapsed, image, _) in enumerate(frames):
            save(image, f"frame-{index:02d}-{elapsed:.3f}s")
        # Examine only the space beside the active panel for the opening. The
        # settled thumbnail can coexist with the transient preview in composer.
        beside = ((0, 95, 800, 900) if side == "compact" else
                  ((right, 95, 1440, 900) if side == "right" else (276, 95, left, 900)))
        openings = []
        for elapsed, image, _ in frames:
            bounds, _ = red_bounds(image.crop(beside))
            if bounds and bounds[2]-bounds[0] >= 200 and bounds[3]-bounds[1] >= 120:
                openings.append((elapsed, image, bounds))
        assert openings, f"no large opening preview on {side} side where room exists"
        opening_time, opening, opening_bounds = openings[0]
        save(opening, "opening")
        held = [elapsed for elapsed, _, bounds in openings if bounds == opening_bounds]
        assert len(held) >= 2 and max(held) - min(held) >= .35, "large preview did not remain visible through the opening hold"
        # A flight frame must have a different red extent than the held opening
        # and still be substantially larger than a settled 64x52 thumbnail.
        opening_full = red_bounds(opening)[0]
        flight = [(elapsed, image, bounds) for elapsed, image, bounds in frames
                  if elapsed > opening_time + .45 and bounds and bounds != opening_full
                  and (bounds[2]-bounds[0] > 80 or bounds[3]-bounds[1] > 70)]
        assert flight, "no intermediate flight frame observed between preview and attachment"
        save(flight[len(flight)//2][1], "flight")
        settled = capture()
        save(settled, "settled")
        bounds, _ = red_bounds(settled)
        assert bounds, "attachment vanished after preview"
        width, height = bounds[2]-bounds[0], bounds[3]-bounds[1]
        assert 62 <= width <= 66 and 50 <= height <= 54, f"attachment not in fixed 64x52 thumbnail: {bounds}"
        assert left <= bounds[0] < bounds[2] <= right and bounds[1] >= 750, f"thumbnail not in active composer: {bounds}"
        if side == "compact":
            assert opening_full[3] < bounds[1], f"compact preview not above attachment: {opening_full} -> {bounds}"
        def distance_to_target(rect):
            return sum(((rect[i] + rect[i+2]) / 2 - (bounds[i] + bounds[i+2]) / 2) ** 2 for i in (0, 1))
        assert any(distance_to_target(rect) < distance_to_target(opening_full) * .64
                   and rect[2] - rect[0] < opening_full[2] - opening_full[0]
                   for _, _, rect in flight), "preview did not shrink and fly toward attachment"
        text = draft_text(settled, left, right, "draft-settled")
        evidence["settled_draft_ocr"] = text
        assert DRAFT.lower().replace(" ", "") in text.lower(), f"paste lost existing draft: {text!r}"
        time.sleep(.25)
        stable = capture()
        assert red_bounds(stable)[0] == bounds, "attachment geometry did not settle"
        assert geometry((root / "state").read_text()) == baseline, "settled paste changed panel geometry"
        evidence["passed"] = True
        print(f"Paste preview acceptance passed: {side} preview, {len(frames)} native frames, flight, preserved draft and panel geometry, thumbnail {width}x{height}")
    finally:
        output.with_suffix(".paste-evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
        # Keep captured evidence even if an assertion interrupted frame sampling.
        if not evidence.get("passed"):
            for index, (elapsed, image, _) in enumerate(frames):
                save(image, f"failure-{index:02d}-{elapsed:.3f}s")
        recorder.terminate()
        try:
            recorder.wait(timeout=5)
        except subprocess.TimeoutExpired:
            recorder.kill()
            recorder.wait()
        clipboard.terminate()
        try:
            clipboard.wait(timeout=5)
        except subprocess.TimeoutExpired:
            clipboard.kill()
            clipboard.wait()
        log = root / "logs/jcode-desktop/jcode-desktop.log"
        if log.exists():
            output.with_suffix(".paste-diagnostics.log").write_text(log.read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("output", type=Path)
    parser.add_argument("--side", choices=("right", "left", "compact"), default="right")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--binary", type=Path)
    args = parser.parse_args()
    if not shutil.which("tesseract"):
        parser.error("draft preservation verification requires tesseract")
    old_verify, old_argv = image_cache_acceptance.verify, sys.argv
    try:
        image_cache_acceptance.verify = lambda output, env, root, panels=2: verify(output, env, root, panels, args.side)
        sys.argv = [str(Path(screenshot.__file__)), str(args.output), "--image-cache-interact", "--transcript", "image", "--panels", "1" if args.side == "compact" else "2"]
        if args.no_build:
            sys.argv.append("--no-build")
        if args.binary:
            sys.argv.extend(["--binary", str(args.binary)])
        screenshot.main()
    finally:
        image_cache_acceptance.verify, sys.argv = old_verify, old_argv


if __name__ == "__main__":
    main()
