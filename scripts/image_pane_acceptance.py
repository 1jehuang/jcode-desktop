"""Native image-pane acceptance on screenshot.py's private offline Xvfb.

The caller invokes verify(output, env, root) after the initial screenshot with
--transcript image, one panel, and the default warm-neutral theme. Dependencies:
Pillow, tesseract, xdotool (including windowstate), ImageMagick import, and the
harness's Xvfb/Openbox. The 1000x600 fixture canvas is checked for clipping.
Only pixels/OCR and real X11 input are evidence, never debug selectors or APIs.
Writes per-step PNG/OCR artifacts, a JSON report, and the final open pane at
output. No prompt is submitted and no live desktop environment is inherited.
"""
import json
from pathlib import Path
import re
import shutil
import subprocess

from default_directory_acceptance import NativeUI
from fresh_session_acceptance import composer_bounds
from image_preview_acceptance import chart_pixels
from model_picker_acceptance import normalized, phrase_bounds


DRAFT = "Keep my image draft safe"
FOLLOWUP = " and keep typing"


def verify(output, env, root):
    output, root = Path(output), Path(root)
    # NativeUI also checks the isolated HOME/runtime and absence of Wayland.
    assert env.get("JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT") == "image", "Use --transcript image"
    assert re.fullmatch(r":\d+(?:\.0)?", env.get("DISPLAY", "")), "Private local X11 required"
    assert env.get("DBUS_SESSION_BUS_ADDRESS") == "unix:path=" + str(root / "no-dbus"), "Isolated D-Bus required"
    ui = NativeUI(output, env, root)
    for command in ("xdotool", "import", "tesseract"):
        assert shutil.which(command, path=env.get("PATH")), f"Missing native acceptance dependency: {command}"
    report = {"checks": {}, "scope": "private Xvfb native input and rendered OCR only",
              "limitations": ["One static image fixture verifies Following latest text, not incoming-image updates."]}
    stage = "initial-inline"

    def record(name, **evidence):
        report["checks"][name] = evidence or True
        print("Image pane check passed: " + name, flush=True)

    def words(image, label):
        # phrase_bounds permits punctuation between words, but punctuation-only
        # fragments elsewhere on the row must not expand a button click box.
        return [word for word in ui.words(image, (0, 0, image.width, image.height), label, psm=11)
                if normalized(word["text"])]

    def text(rows):
        return normalized(" ".join(row["text"] for row in rows))

    def input_bounds(image):
        try:
            bounds = composer_bounds(image)
            # The shared helper excludes the expanded sidebar at x < 280.
            # Compact mode has no such sidebar, so recover the complete edge.
            xs = [x for x in range(image.width)
                  if image.getpixel((x, bounds[1]))[:3] == (135, 121, 107)]
            return (min(xs), bounds[1], max(xs) + 1, bounds[3])
        except AssertionError:
            # Clicking the close control may legitimately blur the editor.
            # Match the warm-neutral unfocused outline in the footer, rather
            # than mistaking loss of the focus ring for loss of the draft.
            pixels = image.load()
            edges = []
            for y in range(image.height * 3 // 4, image.height):
                xs = [x for x in range(image.width)
                      if pixels[x, y][:3] == (81, 72, 63)]
                if len(xs) >= 100:
                    edges.append((y, min(xs), max(xs)))
            assert len(edges) == 2, f"Expected two unfocused composer borders: {edges}"
            top, bottom = edges
            return (top[1], top[0], top[2] + 1, bottom[0] + 1)

    def editor(image, expected, label):
        bounds = input_bounds(image)
        rows = ui.words(image, bounds, label + "-editor")
        assert normalized(expected) in text(rows), (expected, text(rows))
        return bounds

    def inline(image, label, draft=None):
        rows = words(image, label)
        assert "sessionimages" not in text(rows), "Image pane did not close"
        toggle = phrase_bounds(rows, "Images 1")
        count, point = chart_pixels(image)
        if draft:
            editor(image, draft, label)
        return toggle, count, point

    def pane(image, label, draft=DRAFT):
        rows = words(image, label)
        anchor = phrase_bounds(rows, "Session images")
        close_anchor = phrase_bounds(rows, "Inline")
        # Sparse full-window OCR can reorder the header's isolated count and
        # close control. Re-read its real visual row in left-to-right mode.
        header_crop = (max(0, int(anchor[0]) - 8), max(0, int(anchor[1]) - 6),
                       min(image.width, int(close_anchor[2]) + 20),
                       min(image.height, int(anchor[3]) + 6))
        header_words = ui.words(image, header_crop, label + "-header", psm=7)
        header = phrase_bounds(header_words, "Session images 1")
        close = phrase_bounds(header_words, "Inline")  # OCR often drops ×.
        assert close[0] > header[0], ("Close control must share the pane header", header, close)
        assert abs(close[1] - header[1]) < 20, (header, close)
        left, right = max(0, int(header[0]) - 8), min(image.width, int(close[2]) + 32)
        # The fixture chart has a near-white canvas. Locate its largest run of
        # bright scanlines, excluding the small thumbnail. Keeping that bright
        # canvas out of caption OCR prevents Tesseract from dropping dim text.
        runs = []
        start = None
        pixels = image.load()
        for y in range(int(header[3]) + 4, image.height):
            bright = sum(min(pixels[x, y][:3]) > 220 for x in range(left, right))
            if bright > (right - left) * .2:
                if start is None:
                    start = y
            elif start is not None:
                runs.append((start, y))
                start = None
        if start is not None:
            runs.append((start, image.height))
        assert runs, "Main chart canvas is not visible"
        merged = []
        for run in runs:
            if merged and run[0] - merged[-1][1] <= 3:
                merged[-1] = (merged[-1][0], run[1])
            else:
                merged.append(run)
        top, bottom = max(merged, key=lambda run: run[1] - run[0])
        bright_columns = [x for x in range(left, right)
                          if any(min(pixels[x, y][:3]) > 220 for y in range(top, bottom))]
        canvas_width = max(bright_columns) - min(bright_columns) + 1
        canvas_height = bottom - top
        # assets/previews/image-preview.png is 1000x600. A clipped image can
        # still expose blue pixels, so verify its full visible canvas ratio.
        assert abs(canvas_width / canvas_height - 5 / 3) < .06, (
            "Main chart canvas is clipped or distorted", canvas_width, canvas_height)
        report.setdefault("canvas_sizes", {})[label] = [canvas_width, canvas_height]
        crop_bounds = (left, top, right, bottom)
        count, point = chart_pixels(image.crop(crop_bounds))
        point = (point[0] + left, point[1] + top)
        caption_words = ui.words(image, (left, bottom, right, image.height),
                                 label + "-caption", psm=6)
        phrase_bounds(caption_words, "Following latest")
        if draft:
            editor(image, draft, label)
        return close, count, point

    def click_point(point):
        ui.native("mousemove", *point, "click", "1")

    def type_text(value):
        ui.native("type", "--clearmodifiers", "--delay", "25", value)

    def native_output(*args):
        return subprocess.check_output(["xdotool", *map(str, args)], env=env,
                                       cwd=root, text=True, timeout=15).strip()

    try:
        toggle, initial_count, _ = ui.wait_frame(stage, lambda image: inline(image, stage))
        record(stage, chart_pixels=initial_count)
        bounds = ui.wait_frame("initial-composer", composer_bounds)
        ui.click((bounds[0] + 12, bounds[1] + 8, bounds[2] - 12, bounds[1] + 36))
        type_text(DRAFT)
        stage = "draft-before-toggle"
        toggle, _, _ = ui.wait_frame(stage, lambda image: inline(image, stage, DRAFT))
        record(stage)

        ui.click(toggle)
        stage = "pane-open"
        _, pane_count, point = ui.wait_frame(stage, lambda image: pane(image, stage))
        record(stage, main_chart_pixels=pane_count)
        click_point(point)
        stage = "pane-image-enlarged"

        def enlarged(image):
            count, _ = chart_pixels(image)
            assert count > pane_count * 2, ("Main image did not enlarge", pane_count, count)
            return count

        enlarged_count = ui.wait_frame(stage, enlarged)
        record(stage, chart_pixels=enlarged_count)
        ui.native("key", "--clearmodifiers", "Escape")
        stage = "escape-retains-pane"

        def restored(image):
            close, count, point = pane(image, stage)
            assert abs(count - pane_count) < pane_count * .05, (pane_count, count)
            return close

        close = ui.wait_frame(stage, restored)
        record(stage)
        ui.click(close)
        stage = "close-restores-inline"

        def closed(image):
            toggle, count, point = inline(image, stage, DRAFT)
            assert abs(count - initial_count) < initial_count * .05, (initial_count, count)
            return toggle

        ui.wait_frame(stage, closed)
        record(stage)
        bounds = ui.wait_frame("restored-composer", input_bounds)
        ui.click((bounds[0] + 12, bounds[1] + 8, bounds[2] - 12, bounds[1] + 36))
        ui.native("key", "--clearmodifiers", "ctrl+End")
        type_text(FOLLOWUP)
        stage = "typing-after-close"
        toggle, _, _ = ui.wait_frame(stage, lambda image: inline(image, stage, DRAFT + FOLLOWUP))
        record(stage)
        ui.click(toggle)
        stage = "pane-reopened"
        ui.wait_frame(stage, lambda image: pane(image, stage, DRAFT + FOLLOWUP))
        record(stage)

        window = native_output("getactivewindow")
        geometry = dict(line.split("=", 1) for line in native_output("getwindowgeometry", "--shell", window).splitlines() if "=" in line)
        width, height = int(geometry["WIDTH"]), int(geometry["HEIGHT"])
        # Native resize exercises the stacked narrow layout. Restore the window
        # even on assertion failure so subsequent harness cleanup stays simple.
        stage = "narrow-pane"
        try:
            # screenshot.py starts Openbox clients maximized. EWMH removal is
            # required before a native size request can take effect.
            ui.native("windowstate", "--remove", "MAXIMIZED_HORZ", window)
            ui.native("windowstate", "--remove", "MAXIMIZED_VERT", window)
            ui.native("sleep", ".25", "windowsize", "--sync", window, 800, height)
            ui.wait_frame(stage, lambda image: pane(image, stage, DRAFT + FOLLOWUP))
            actual = native_output("getwindowgeometry", "--shell", window)
            assert "WIDTH=800\n" in actual + "\n", actual
            record(stage, width=800)
        finally:
            ui.native("windowsize", window, width, height)
            ui.native("windowmove", window, geometry["X"], geometry["Y"])
            ui.native("windowstate", "--add", "MAXIMIZED_HORZ", window)
            ui.native("windowstate", "--add", "MAXIMIZED_VERT", window)
        stage = "final-pane"
        ui.wait_frame(stage, lambda image: pane(image, stage, DRAFT + FOLLOWUP))
        shutil.copyfile(ui.artifact(stage + ".png"), output)
        record(stage)
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, failed_stage=stage, error=str(error))
        ui.capture("image-pane-failure")
        raise
    finally:
        ui.artifact("image-pane.acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
    print("Image pane native acceptance passed: toggle, lightbox, Escape, inline restore, draft typing, narrow resize")
