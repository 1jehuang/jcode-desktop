#!/usr/bin/env python3
"""Measure inline tool surfaces and exercise real clicks on a private X11 display.

Uses screenshot.py's real application fixture and isolation, without opening or
controlling the user's desktop. Pass an unused output PNG path. The current
binary must already be built.
"""
from pathlib import Path
import subprocess
import sys
import time

from PIL import Image, ImageChops
import screenshot


def verify(output, env, root, run):
    before = Image.open(output).convert("RGB")
    background = before.getpixel((1000, 300))

    def running_center(image):
        # The warm-neutral fixture's solid gold running indicator. Text uses
        # subpixel antialiasing, so require a multi-pixel solid dot, not a glyph.
        rows = []
        for y in range(100, image.height - 180):
            count = sum(image.getpixel((x, y)) == (208, 177, 125)
                        for x in range(290, 303))
            if count >= 2:
                rows.append(y)
        assert len(rows) >= 4, "running status dot did not paint"
        return rows[len(rows) // 2]

    def same_surface(image, top, bottom):
        # Includes bodies, inter-row gaps, and the former outer left border.
        boxes = [(800, top, 1200, bottom), (288, top, 290, bottom)]
        checked = 0
        for box in boxes:
            region = image.crop(box)
            count = region.width * region.height
            assert region.getcolors(maxcolors=count) == [(count, background)], (
                "tool surface or border differs from surrounding transcript", box)
            checked += count
        return checked

    def changed_pixels(image):
        return sum(count for count, color in
                   image.getcolors(maxcolors=image.width * image.height)
                   if color != (0, 0, 0))

    def click(y):
        run(["xdotool", "mousemove", "450", str(y), "click", "1",
             "mousemove", "100", "100"], env=env, cwd=root,
            check=True, timeout=10)

    def capture(suffix):
        path = output.with_name(output.stem + suffix + ".png")
        time.sleep(0.5)
        run(["import", "-window", "root", "png:" + str(path)],
            env=env, cwd=root, check=True, timeout=15)
        return Image.open(path).convert("RGB")

    y = running_center(before)
    checked = same_surface(before, y - 50, y + 50)
    # This interior strip contains the three status rows, but no long text.
    # A card background or any horizontal border would change these pixels.
    click(y)
    expanded = capture("-expanded")
    expanded_y = running_center(expanded)
    crop = (280, 60, 1420, 1490)  # omit prompt caret and changing build metadata
    difference = ImageChops.difference(before.crop(crop), expanded.crop(crop))
    changed = changed_pixels(difference)
    assert changed > 500, "native header click did not reveal tool details"
    assert expanded_y < y - 30, "expanded detail did not increase row height"
    checked += same_surface(expanded, expanded_y - 10, y + 40)
    click(expanded_y)
    collapsed = capture("-collapsed")
    assert running_center(collapsed) == y, "second click did not restore row layout"
    difference = ImageChops.difference(before.crop(crop), collapsed.crop(crop))
    remaining = changed_pixels(difference)
    assert remaining == 0, f"collapse failed to restore the original transcript: {remaining} pixels"
    print(f"Inline tools acceptance passed: {checked} surface/edge pixels match "
          f"transcript RGB {background}; native expansion changes {changed} pixels "
          f"and moves the header {y - expanded_y}px; collapse restores all transcript pixels.")


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: accept-inline-tools.py OUTPUT.png")
    output = Path(sys.argv[1]).resolve()
    original_run = subprocess.run
    checked = False

    def capture_and_verify(command, *args, **kwargs):
        nonlocal checked
        result = original_run(command, *args, **kwargs)
        if not checked and command == ["import", "-window", "root", "png:" + str(output)]:
            checked = True
            verify(output, kwargs["env"], kwargs["cwd"], original_run)
        return result

    # Hook the capture, while screenshot.py still owns its private display.
    # No production fixture or user's running window is modified.
    subprocess.run = capture_and_verify
    try:
        sys.argv = ["screenshot.py", str(output), "--no-build", "--size", "1440x1600"]
        screenshot.main()
        assert checked, "screenshot capture hook did not run"
    finally:
        subprocess.run = original_run


if __name__ == "__main__":
    main()
