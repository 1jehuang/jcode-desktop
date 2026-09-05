#!/usr/bin/env python3
"""Measure the real 1440x1000 --transcript reasoning screenshot fixture.

Requires local Pillow and Tesseract. No network or desktop interaction.
Fixed fixture coordinates intentionally fail if its layout changes.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

from PIL import Image

THINKING = (292, 720, 1405, 850)
ANSWER = (292, 883, 1405, 903)
OCR_REGION = (284, 712, 1418, 858)
PADDING = ((288, 712, 292, 858), (1415, 712, 1419, 858),
           (288, 712, 1419, 719), (288, 850, 1419, 858))


def luminance(rgb):
    channels = [value / 255 for value in rgb]
    channels = [v / 12.92 if v <= .04045 else ((v + .055) / 1.055) ** 2.4
                for v in channels]
    return sum(v * weight for v, weight in zip(channels, (.2126, .7152, .0722)))


def contrast(a, b):
    low, high = sorted((luminance(a), luminance(b)))
    return (high + .05) / (low + .05)


def foreground(image, box, background):
    pixels = Counter(image.crop(box).get_flattened_data())
    color, count = next((color, count) for color, count in pixels.most_common()
                        if color != background)
    assert count >= 100, "expected a substantial rendered text sample"
    return color


def verify_text(text):
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    normalized = re.sub(r"\s+", " ", text).strip().lower()
    assert len(lines) == 6, f"expected six content lines, not extra UI labels: {lines}"
    assert not re.search(r"\bshow (less|more|all)\b", normalized), "disclosure control found"
    assert not any(re.fullmatch(r"[●.\s]*(reasoning|thinking)[… .]*", line.lower())
                   for line in lines), "thinking/role label found"
    assert normalized.startswith("the content should read like part of the conversation"), "missing beginning"
    assert "the surrounding labels and controls." in normalized, "truncated first paragraph"
    assert "keep the presentation simple" in normalized, "missing Markdown heading"
    assert "no card background or border" in normalized, "missing list content"
    assert normalized.endswith("preserve the full text and markdown formatting"), "missing final content"
    assert "**" not in text and "##" not in text, "Markdown delimiters were painted literally"
    return len(lines)


def verify_image(path, scratch):
    image = Image.open(path).convert("RGB")
    assert image.size == (1440, 1000), "requires the documented fixture dimensions"
    background = image.getpixel((500, 300))
    points = {(x, y) for left, top, right, bottom in PADDING
              for x in range(left, right) for y in range(top, bottom)}
    mismatches = sum(image.getpixel(point) != background for point in points)
    assert mismatches == 0, f"card fill/border/chrome found in {mismatches} padding pixels"
    thinking = foreground(image, THINKING, background)
    answer = foreground(image, ANSWER, background)
    thinking_contrast = contrast(thinking, background)
    answer_contrast = contrast(answer, background)
    assert 4.5 <= thinking_contrast < answer_contrast, "thinking must be readable but dimmer than the answer"
    crop = scratch / (path.stem + "-ocr.png")
    region = image.crop(OCR_REGION)
    region.resize((region.width * 3, region.height * 3)).save(crop)
    text = subprocess.check_output(["tesseract", str(crop), "stdout", "--psm", "6"],
                                   stderr=subprocess.DEVNULL, text=True)
    lines = verify_text(text)
    return {
        "image": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "background_rgb": background, "thinking_rgb": thinking, "answer_rgb": answer,
        "thinking_contrast": round(thinking_contrast, 3),
        "answer_contrast": round(answer_contrast, 3),
        "background_padding_pixels": len(points), "background_mismatches": mismatches,
        "visible_content_lines": lines, "extra_labels_or_disclosure_controls": 0,
        "complete_content_and_rendered_markdown": True, "ocr": text.strip(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dark", type=Path)
    parser.add_argument("light", type=Path)
    parser.add_argument("--before-light", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="thinking-metrics-", dir=args.output.parent) as temp:
        scratch = Path(temp)
        results = {"dark": verify_image(args.dark, scratch),
                   "light": verify_image(args.light, scratch)}
        if args.before_light:
            before = Image.open(args.before_light).convert("RGB")
            bg = before.getpixel((500, 300))
            old_contrast = contrast(foreground(before, THINKING, bg), bg)
            new_contrast = results["light"]["thinking_contrast"]
            assert old_contrast < 4.5 <= new_contrast, "expected a measured readability improvement"
            results["light_improvement"] = {"before_contrast": round(old_contrast, 3),
                                            "after_contrast": new_contrast,
                                            "contrast_multiplier": round(new_contrast / old_contrast, 3)}
        # Negative controls show that the checks reject the old kinds of chrome.
        bad = Image.open(args.dark).convert("RGB")
        bad.putpixel((289, 750), (255, 0, 0))
        bad_path = scratch / "bad-background.png"
        bad.save(bad_path)
        try:
            verify_image(bad_path, scratch)
        except AssertionError as error:
            assert "card fill/border/chrome" in str(error)
        else:
            raise AssertionError("background negative control was not rejected")
        for suffix in ("\nthinking", "\nshow less"):
            try:
                verify_text(results["dark"]["ocr"] + suffix)
            except AssertionError:
                pass
            else:
                raise AssertionError("label negative control was not rejected")
        results["negative_controls_rejected"] = ["card background", "thinking label", "show less"]
        args.output.write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
