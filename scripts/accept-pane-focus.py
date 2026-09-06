#!/usr/bin/env python3
"""Verify pane focus colors through native clicks on a private Xvfb display.

Build the desktop first. This uses the production UI with offline transcripts,
never the active desktop or live credentials.
"""
import argparse
from pathlib import Path
import subprocess

from PIL import Image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    repo = Path(__file__).resolve().parents[1]
    for theme in ("parchment", "warm-neutral"):
        for layout in ("folder_tabs", "normal"):
            colors = []
            for focus in (0, 1):
                output = args.output / f"{theme}-{layout}-{focus}.png"
                subprocess.run([
                    "python3", str(repo / "scripts/screenshot.py"), str(output),
                    "--no-build", "--panels", "2", "--theme", theme,
                    "--layout-mode", layout, "--focus-panel", str(focus),
                ], check=True)
                image = Image.open(output).convert("RGB")
                # Sample plain pane padding, away from text, rounded corners,
                # the composer, and tabs. Layouts have different canvas insets.
                left = 276 if layout == "folder_tabs" else 264
                width = (image.width - (288 if layout == "folder_tabs" else 264)) / 2
                samples = tuple(image.getpixel((round(left + width * i + 20), 200)) for i in range(2))
                assert max(abs(a - b) for a, b in zip(*samples)) >= 10, (theme, layout, focus, samples)
                colors.append(samples)
            assert colors[0] == colors[1][::-1], (theme, layout, colors)
            print(f"PASS: {theme} {layout} pane colors swap with native focus: {colors}", flush=True)


if __name__ == "__main__":
    main()
