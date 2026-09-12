"""Native slash-menu acceptance on screenshot.py's private offline X11 display.

Warm-neutral, empty transcript, 1x scale. No application state or command catalog
is used as evidence: keyboard traversal discovers the last command from pixels.
Per-step PNGs/OCR crops and OUTPUT.acceptance.json remain available on failure.
"""
import json
import shutil
import subprocess
import time

from PIL import Image

from fresh_session_acceptance import composer_bounds
from model_picker_acceptance import dialog_bounds as suggestion_bounds
from model_picker_acceptance import parse_words, phrase_bounds

TEXT = (228, 221, 211)
HEADER_BG = (48, 43, 39)
TEXT_FAINT = (155, 144, 132)
BORDER = (135, 121, 107)


def dialog_bounds(image):
    """Use shared dialog detection, then include its rounded side extents."""
    left, top, right, bottom = suggestion_bounds(image)
    y = (top + bottom) // 2
    edges = [x for x in range(max(280, left - 16), min(image.width, right + 16))
             if image.getpixel((x, y)) == BORDER]
    assert len(edges) == 2, ("Expected two menu side borders", edges)
    return (edges[0], top, edges[-1] + 1, bottom)


def runs(values):
    result = []
    for value in values:
        if result and value == result[-1][-1] + 1:
            result[-1].append(value)
        else:
            result.append([value])
    return result


def selection(image, bounds):
    """Find a full-width light row, not light text in an unselected row."""
    left, top, right, bottom = bounds
    sample = range(left + 40, right - 50, 8)
    rows = [y for y in range(top + 1, bottom - 1)
            if image.getpixel((left + 9, y)) == TEXT
            and sum(image.getpixel((x, y)) == TEXT for x in sample) >= len(sample) * .4]
    bands = runs(rows)
    assert len(bands) == 1, ("Expected one visible selected row", bands)
    first, last = bands[0][0], bands[0][-1]
    assert last - first >= 28, ("Selected row is clipped", first, last)
    assert first > top + 2 and last < bottom - 2, "Selection must stay on screen"
    assert image.getpixel((left + 9, (first + last) // 2)) == TEXT
    # The reserved trailing 14px marker is separate from description text.
    marker = (right - 40, first + 2, right - 22, last - 1)
    dark = [(x, y) for y in range(marker[1], marker[3])
            for x in range(marker[0], marker[2])
            if sum(image.getpixel((x, y))) / 3 < 170]
    assert len(dark) >= 7, "Selected trailing checkmark missing"
    xs, ys = zip(*dark)
    assert 5 <= max(xs) - min(xs) <= 14 and 4 <= max(ys) - min(ys) <= 16, (
        "Unexpected selected checkmark shape", dark)
    return (left + 12, first, right - 44, last + 1)


def scrollbar(image, bounds, present=True):
    """Measure the shared 4px thumb inside the menu's right border."""
    left, top, right, bottom = bounds
    pixels = [(x, y) for y in range(top + 3, bottom - 3)
              for x in range(right - 12, right - 2)
              if image.getpixel((x, y)) == TEXT_FAINT]
    if not present:
        assert not pixels, ("Nonoverflow list retained scrollbar", pixels[:8])
        return None
    assert pixels, "Overflow scrollbar thumb is invisible"
    xs, ys = zip(*pixels)
    assert max(xs) - min(xs) + 1 == 4, ("Thumb must be 4px wide", min(xs), max(xs))
    assert 4 <= (right - 1) - max(xs) <= 5, ("Thumb must be inset right+4", right, max(xs))
    assert max(ys) - min(ys) >= 24, "Thumb is too short"
    x = (min(xs) + max(xs)) // 2
    # Track is a low-alpha TEXT fill on HEADER_BG, not an invisible gutter.
    track = [image.getpixel((x, y)) for y in range(top + 8, bottom - 8)
             if y < min(ys) - 3 or y > max(ys) + 3]
    assert track and sum(all(2 <= c - b <= 6 for c, b in zip(color, HEADER_BG))
                         for color in track) >= len(track) * .8, "Scrollbar track missing"
    assert image.getpixel((right - 1, (top + bottom) // 2)) == BORDER, "Menu border missing"
    return [min(xs), min(ys), max(xs) + 1, max(ys) + 1]


def verify(output, env, root):
    assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1", "Offline fixture required"
    assert env.get("JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT") == "empty", "Empty transcript required"
    assert env.get("XDG_RUNTIME_DIR") == str(root / "runtime"), "Private runtime required"
    assert env.get("HOME") == str(root / "home"), "Private home required"
    assert env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"), "Private X11 required"
    report = {"checks": {}, "artifacts": [], "scope": "native warm-neutral slash menu pixels"}
    stage = "initial"

    def native(*args):
        subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root, check=True, timeout=15)

    def type_text(text):
        native("type", "--clearmodifiers", "--delay", "35", text)

    def capture(label):
        path = output.with_name(output.stem + "-" + label + ".png")
        time.sleep(.3)
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=env, cwd=root, check=True, timeout=15)
        if str(path) not in report["artifacts"]:
            report["artifacts"].append(str(path))
        return path, Image.open(path).convert("RGB")

    def ocr(image, bounds, label):
        path = output.with_name(output.stem + "-" + label + "-ocr.png")
        crop = image.crop(bounds)
        crop.resize((crop.width * 3, crop.height * 3)).save(path)
        tsv = subprocess.check_output(["tesseract", str(path), "stdout", "--psm", "6", "tsv"],
                                      env=env, cwd=root, timeout=20, stderr=subprocess.DEVNULL).decode()
        return parse_words(tsv, bounds)

    def frame(label, check):
        deadline = time.monotonic() + 8
        while True:
            path, image = capture(label)
            try:
                return path, image, check(image)
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise

    def menu(image):
        bounds = dialog_bounds(image)
        row = selection(image, bounds)
        return bounds, row, scrollbar(image, bounds)

    try:
        initial = Image.open(output).convert("RGB")
        composer = composer_bounds(initial)
        shutil.copyfile(output, output.with_name(output.stem + "-initial.png"))
        native("mousemove", composer[0] + 32, composer[1] + 24, "click", "1")
        type_text("/")
        stage = "open-selection-colors-checkmark-scrollbar"
        path, image, (bounds, first, thumb) = frame(stage, menu)
        phrase_bounds(ocr(image, bounds, stage), "/help")
        # Away from text and rounded corners, unselected rows retain HEADER_BG.
        unselected = [y for y in range(first[3] + 8, bounds[3] - 8)
                      if image.getpixel((bounds[0] + 9, y)) == HEADER_BG]
        assert len(unselected) >= 30, "Unselected background is not HEADER_BG"
        report["checks"][stage] = {"dialog": bounds, "selected": first, "thumb": thumb}
        shutil.copyfile(path, output)

        stage = "arrow-down-visible-selection"
        native("key", "--clearmodifiers", "Down")
        def moved(image):
            current = menu(image)
            assert current[1][1] > first[1], "Down did not visibly move selection"
            assert image.getpixel((bounds[0] + 9, (first[1] + first[3]) // 2)) == HEADER_BG
            return current
        _, _, current = frame(stage, moved)
        report["checks"][stage] = {"selected": current[1]}

        stage = "wheel-moves-thumb"
        native("mousemove", bounds[2] - 55, bounds[3] - 40,
               "click", "--repeat", "3", "--delay", "80", "5")
        native("mousemove", composer[0] + 32, composer[1] + 24)
        def wheel(image):
            current = scrollbar(image, dialog_bounds(image))
            assert current[1] > thumb[1] + 3, "Wheel did not move scrollbar thumb"
            return current
        _, _, wheel_thumb = frame(stage, wheel)
        report["checks"][stage] = {"before": thumb, "after": wheel_thumb}

        # Reset through real composer input, then discover the end by clamped
        # Down navigation. No hardcoded command names, counts or source parsing.
        native("key", "ctrl+a", "BackSpace")
        type_text("/")
        stage = "keyboard-reset"
        frame(stage, menu)
        stage = "last-command-stays-visible"
        visited = []
        previous = None
        for index in range(256):
            if index % 10 == 0:
                print(f"Slash-menu native traversal: {index} Down steps", flush=True)
            label = f"keyboard-{index:03d}"
            path, image, (bounds, row, current_thumb) = frame(label, menu)
            words = ocr(image, (row[0], row[1], min(row[0] + 300, row[2]), row[3]), label)
            identity = " ".join(word["text"] for word in words).strip().lower()
            assert identity, "Selected command must be readable"
            if identity == previous:
                assert len(visited) > 12, ("Command list appears truncated", visited)
                assert current_thumb[3] >= bounds[3] - 16, "Last command must reach scroll bottom"
                report["checks"][stage] = {"commands": visited, "count": len(visited),
                                           "last": identity, "selected": row, "thumb": current_thumb}
                shutil.copyfile(path, output)
                break
            assert identity not in visited, ("Keyboard traversal repeated a command", identity)
            visited.append(identity)
            previous = identity
            native("key", "--clearmodifiers", "Down")
        else:
            raise AssertionError("No clamped last command after 256 Down steps")

        stage = "filtered-nonoverflow-hides-scrollbar"
        native("key", "ctrl+a")
        type_text("/effort")
        def filtered(image):
            bounds = dialog_bounds(image)
            selection(image, bounds)
            scrollbar(image, bounds, present=False)
            words = ocr(image, bounds, stage)
            phrase_bounds(words, "/effort")
            return bounds
        _, _, filtered_bounds = frame(stage, filtered)
        report["checks"][stage] = {"dialog": filtered_bounds, "scrollbar": "absent"}

        stage = "final-open-menu"
        native("key", "ctrl+a")
        type_text("/")
        path, _, _ = frame(stage, menu)
        shutil.copyfile(path, output)
        report["checks"][stage] = True
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, failed_step=stage, error=str(error))
        raise
    finally:
        output.with_suffix(".acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Slash-menu native acceptance: " + json.dumps(report))
