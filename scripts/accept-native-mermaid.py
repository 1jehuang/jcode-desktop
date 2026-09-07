#!/usr/bin/env python3
"""Pixel/OCR acceptance for the exact six-node Mermaid desktop fixture.

Inputs are screenshot.py --transcript mermaid --mermaid-source
assets/previews/mermaid-tall.mmd outputs, neutral-light 1920x1080 and
neutral-dark 900x1080. Fixed crops deliberately verify actual on-screen text,
not source strings or internal layout. --self-test requires the oracle to reject
both clipped bottom nodes and a mismatched diagram background.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile

from PIL import Image, ImageDraw

LABELS = [
    ("Start with an idea", (367, 210, 523, 239), r"start with an idea"),
    ("Make a plan", (389, 298, 490, 327), r"make [a&] plan"),
    ("Build it", (403, 386, 481, 415), r"build it"),
    ("Does it work?", (383, 522, 498, 551), r"does it work\?"),
    ("Ship it", (315, 665, 388, 693), r"ship it"),
    ("Improve it", (459, 665, 556, 693), r"improve it"),
]


def verify(image, scratch):
    image = image.convert("RGB")
    # These points are empty canvas and empty transcript at the same height.
    # The previous hard-coded dark image would fail this in the light theme.
    canvas = image.getpixel((300, 200))
    transcript = image.getpixel((700, 200))
    assert canvas == transcript, ("canvas does not follow transcript palette", canvas, transcript)
    observed = {}
    for index, (label, bounds, pattern) in enumerate(LABELS):
        crop = image.crop(bounds)
        crop = crop.resize((crop.width * 4, crop.height * 4))
        path = scratch / f"label-{index}.png"
        crop.save(path)
        text = subprocess.run(
            ["tesseract", str(path), "stdout", "--psm", "7"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
        assert re.search(pattern, text.lower()), ("missing or clipped visible label", label, text)
        observed[label] = text
    return {"visible_nodes": len(observed), "labels": observed, "canvas_rgb": canvas,
            "transcript_rgb": transcript}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("light", type=Path)
    parser.add_argument("dark", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    results = {}
    # Keep all generated files beside the requested screenshot artifacts.
    with tempfile.TemporaryDirectory(prefix="native-mermaid-ocr-", dir=args.light.parent) as tmp:
        scratch = Path(tmp)
        for name, path, expected_size in (("light", args.light, (1920, 1080)),
                                           ("dark", args.dark, (900, 1080))):
            image = Image.open(path).convert("RGB")
            assert image.size == expected_size, (name, image.size, expected_size)
            results[name] = verify(image, scratch)
            if args.self_test:
                clipped = image.copy()
                ImageDraw.Draw(clipped).rectangle((290, 640, 595, 710),
                                                  fill=image.getpixel((700, 200)))
                mismatched = image.copy()
                mismatched.putpixel((300, 200), (255, 0, 255))
                for label, invalid in (("clipped_bottom", clipped), ("wrong_palette", mismatched)):
                    try:
                        verify(invalid, scratch)
                    except AssertionError:
                        results[name][f"rejects_{label}"] = True
                    else:
                        raise AssertionError(f"oracle incorrectly accepted {name}/{label}")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
