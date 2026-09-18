#!/usr/bin/env python3
"""Verify timed edit cards in the real desktop on screenshot.py's private display."""
import csv
import io
import json
from pathlib import Path
import subprocess
import sys
import time

from PIL import Image
import screenshot


def verify(output, env, root, run):
    def capture(name):
        path = output.with_name(output.stem + "-" + name + ".png")
        deadline = time.monotonic() + 10
        while True:
            run(["import", "-window", "root", "png:" + str(path)],
                env=env, cwd=root, check=True, timeout=15)
            with Image.open(path).convert("RGB") as image:
                if image.getbbox() is not None:
                    return path
            assert time.monotonic() < deadline, "fixture did not paint"
            time.sleep(.1)

    def words(path):
        result = run(["tesseract", str(path), "stdout", "tsv"],
                     env={**env, "OMP_THREAD_LIMIT": "1"}, cwd=root,
                     check=True, capture_output=True, text=True, timeout=30)
        return [r for r in csv.DictReader(io.StringIO(result.stdout), delimiter="\t", quoting=csv.QUOTE_NONE)
                if r.get("text", "").strip()]

    def text(rows):
        return " ".join(row["text"] for row in rows)

    def bar_width(path):
        # Warm-neutral's exact accent fill. Restrict to transcript rows, excluding
        # window outlines, sidebar and composer. Anti-aliased ends are immaterial.
        with Image.open(path).convert("RGB") as image:
            pixels = image.load()
            return max(sum(pixels[x, y] == (182, 160, 138)
                           for x in range(310, image.width - 24))
                       for y in range(200, min(750, image.height - 150)))

    def geometry(path):
        # The fixture's real warm-neutral surfaces, not a mocked component.
        with Image.open(path).convert("RGB") as image:
            top_tone, bottom_tone = (41, 37, 33), (28, 26, 24)
            x = image.width - 100
            starts, start = [], None
            for y in range(175, 750):
                if image.getpixel((x, y)) == top_tone:
                    if start is None:
                        start = y
                else:
                    if start is not None and y - start >= 20:
                        starts.append(start)
                    start = None
            assert len(starts) == 2, starts
            top, next_top = starts
            row = [x for x in range(280, image.width - 20)
                   if image.getpixel((x, top + 12)) == top_tone]
            left, right, bottom = min(row), max(row), next_top - 8
            assert image.getpixel((left + 16, top + 30)) == bottom_tone
            for edge in (left, right):
                assert image.getpixel((edge, top)) != top_tone, "rounded top corner"
                assert image.getpixel((edge, bottom - 1)) != bottom_tone, "rounded bottom corner"
            assert image.getpixel((left + 16, bottom - 1)) == bottom_tone
            return left, right, top, bottom

    def click(x, y):
        run(["xdotool", "mousemove", str(x), str(y), "click", "1", "mousemove", "100", "100"],
            env=env, cwd=root, check=True, timeout=10)
        time.sleep(.4)

    # A software-rendered first frame can arrive after screenshot.py's
    # initial Escape under build load. Dismiss only the fixture's startup
    # overlay, then wait for the full-width, undimmed card surfaces.
    capture("ready")
    run(["xdotool", "key", "--clearmodifiers", "Escape"],
        env=env, cwd=root, check=True, timeout=10)
    deadline = time.monotonic() + 10
    while True:
        first = capture("expanded")
        try:
            expanded_geometry = geometry(first)
            break
        except AssertionError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(.1)
    first_width = bar_width(first)
    time.sleep(.7)
    second = capture("countdown")
    second_width = bar_width(second)
    assert first_width > 100 and 0 < second_width < first_width - 30, (first_width, second_width)
    time.sleep(3.2)
    collapsed = capture("collapsed")
    left, right, top, bottom = geometry(collapsed)
    assert bottom - top < expanded_geometry[3] - expanded_geometry[2]
    collapsed_text = text(words(collapsed))
    assert "Show diff" not in collapsed_text and "Collapse" not in collapsed_text
    assert "Review ›" not in collapsed_text and "Keep open" not in collapsed_text
    assert "Continue" not in collapsed_text, "code must leave the compact cards"
    assert bar_width(collapsed) < 30, "countdown must reach zero"
    click(left + 80, top + 14)  # Intent/card, not a separate control.
    reopened = capture("reopened")
    geometry(reopened)
    assert "Continue" in text(words(reopened))
    time.sleep(5.5)
    pinned = capture("kept-open")
    assert "Continue" in text(words(pinned)), "manual reopening must not auto-collapse again"
    # The file path is another part of the card, not a review-panel link.
    click(left + 80, top + 40)
    collapsed_again = capture("path-collapsed")
    geometry(collapsed_again)
    assert "Continue" not in text(words(collapsed_again))
    # Even the rounded footer is part of the card's click target.
    click(left + 80, bottom - 3)
    assert "Continue" in text(words(capture("footer-reopened")))
    click(left + 80, top + 14)
    # The top-right change counts open the dedicated review panel.
    click(right - 30, top + 14)
    review = capture("review")
    review_text = text(words(review))
    assert "Unified" in review_text and "Split" in review_text, review_text
    report = {"initial_bar_pixels": first_width, "later_bar_pixels": second_width,
              "rounded_expanded_and_collapsed": True, "two_tone": True,
              "auto_collapsed": True, "manual_reopen_stays_open": True,
              "card_and_path_and_footer_toggle_inline": True, "counts_open_review": True}
    output.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n")
    print("Timed edit acceptance passed:", report)


def main():
    output = Path(sys.argv[1] if len(sys.argv) > 1 else "target/timed-edit-acceptance.png").resolve()
    original_run = subprocess.run
    checked = False

    def capture_and_verify(command, *args, **kwargs):
        nonlocal checked
        result = original_run(command, *args, **kwargs)
        if not checked and command == ["import", "-window", "root", "png:" + str(output)]:
            checked = True
            verify(output, kwargs["env"], kwargs["cwd"], original_run)
        return result

    subprocess.run = capture_and_verify
    try:
        sys.argv = ["screenshot.py", str(output), "--no-build", "--transcript", "diff"]
        screenshot.main()
        assert checked, "capture hook did not run"
    finally:
        subprocess.run = original_run


if __name__ == "__main__":
    main()
