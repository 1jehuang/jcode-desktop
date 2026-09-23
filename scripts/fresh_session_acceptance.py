"""Measure rendered pixels and native keyboard behavior on a private X11 display."""
import csv
import io
import itertools
import json
import subprocess
import time

from PIL import Image


CLIPBOARD_OWNER = """
import gi, pathlib, sys
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk
clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
clipboard.set_text(sys.argv[1], -1)
pathlib.Path(sys.argv[2]).touch()
Gtk.main()
"""


def composer_bounds(image):
    # Warm-neutral's focused border. Look for long horizontal edges, excluding
    # the sidebar and tab strip. No application layout metadata is consulted.
    border = (135, 121, 107)
    edges = []
    for y in range(70, image.height):
        xs = [x for x in range(280, image.width)
              if image.getpixel((x, y))[:3] == border]
        if len(xs) >= 100:
            edges.append((y, min(xs), max(xs)))
    assert len(edges) == 2, f"Expected two composer borders, found {edges}"
    top, bottom = edges
    return [top[1], top[0], top[2] + 1, bottom[0] + 1]


def verify(output, env, root):
    def native(*args):
        subprocess.run(["xdotool", *args], env=env, cwd=root, check=True, timeout=10)

    def capture(suffix):
        path = output.with_name(output.stem + suffix + ".png")
        time.sleep(.4)
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=env, cwd=root, check=True, timeout=10)
        return path, Image.open(path).convert("RGB")

    def assert_text(image, bounds, expected, label):
        crop = image.crop(bounds)
        crop = crop.resize((crop.width * 3, crop.height * 3))
        path = output.with_name(output.stem + "-" + label + "-ocr.png")
        crop.save(path)
        tsv = subprocess.check_output(
            ["tesseract", str(path), "stdout", "--psm", "6", "tsv"],
            env=env, cwd=root, timeout=15, stderr=subprocess.DEVNULL,
        ).decode()
        words = [row for row in csv.DictReader(io.StringIO(tsv), delimiter="\t")
                 if row["text"].strip()]
        text = " ".join(row["text"] for row in words)
        offset = text.lower().find(expected.lower())
        assert offset >= 0, (expected, text)
        cursor = 0
        for word in words:
            if cursor + len(word["text"]) > offset:
                return text, bounds[1] + int(word["top"]) / 3
            cursor += len(word["text"]) + 1
        raise AssertionError("OCR match has no word position")

    initial = Image.open(output).convert("RGB")
    fresh = composer_bounds(initial)
    x1, y1, x2, y2 = fresh
    center = ((x1 + x2) / 2, (y1 + y2) / 2)
    # The empty canvas is the longest solid-color span above the fresh
    # composer. Measure its edges so sidebar-width changes do not invalidate
    # this centering check, while retaining an independent pixel assertion.
    scan_y = y1 // 2
    spans = [list(group) for _, group in itertools.groupby(
        range(initial.width), key=lambda x: initial.getpixel((x, scan_y)))]
    canvas_span = max(spans, key=len)
    assert len(canvas_span) > initial.width / 2, (scan_y, len(canvas_span))
    canvas_bounds_x = [canvas_span[0], canvas_span[-1] + 1]
    expected_center_x = sum(canvas_bounds_x) / 2
    assert abs(center[0] - expected_center_x) <= 2, (center, expected_center_x)
    assert .38 <= center[1] / initial.height <= .60, (center, initial.size)
    assert y2 - y1 >= 110, fresh
    assert x2 - x1 <= 760, fresh

    native("mousemove", str(x1 + 32), str(y1 + 28), "click", "1")
    prompt = "Fresh session typing works"
    native("type", "--clearmodifiers", "--delay", "30", prompt)
    _, typed = capture("-typed")
    assert composer_bounds(typed) == fresh, "Typing moved the fresh composer"
    typed_text, _ = assert_text(typed, fresh, prompt, "typed")

    # Exercise the real editor with a private-display clipboard paste, not
    # fixture text injection or a new Shift+Enter feature. Each paragraph must
    # have its own visible OCR position: flattening or dropping newlines must
    # not pass merely because the draft still exists in application state.
    ready = root / "multiline-clipboard-ready"
    ready.unlink(missing_ok=True)
    clipboard = subprocess.Popen(
        ["/usr/bin/python3", "-c", CLIPBOARD_OWNER, "\nDictated words", str(ready)],
        env=env, cwd=root)
    try:
        deadline = time.monotonic() + 10
        while not ready.exists():
            if clipboard.poll() is not None or time.monotonic() >= deadline:
                raise AssertionError("Private multiline clipboard failed to start (GTK3/PyGObject required)")
            time.sleep(.05)
        native("key", "ctrl+v")
        _, multiline = capture("-multiline")
    finally:
        clipboard.terminate()
        try:
            clipboard.wait(timeout=5)
        except subprocess.TimeoutExpired:
            clipboard.kill()
            clipboard.wait(timeout=5)
    assert composer_bounds(multiline) == fresh, "Two draft lines moved the fresh composer"
    multiline_text, first_line_top = assert_text(
        multiline, fresh, prompt, "multiline-first")
    _, second_line_top = assert_text(
        multiline, fresh, "Dictated words", "multiline-second")
    assert second_line_top - first_line_top >= 10, (
        "Draft paragraphs must render on distinct lines", first_line_top, second_line_top)

    # Select just the last word using native character selection, then replace
    # it. This checks editing on the formerly invisible second paragraph while
    # proving the original draft and newline survive the replacement.
    native("key", "--repeat", "5", "--delay", "30", "shift+Left")
    native("type", "--clearmodifiers", "--delay", "30", "speech")
    _, edited = capture("-multiline-edited")
    assert composer_bounds(edited) == fresh
    edited_text, edited_first_top = assert_text(
        edited, fresh, prompt, "multiline-edited-first")
    _, edited_second_top = assert_text(
        edited, fresh, "Dictated speech", "multiline-edited-second")
    assert "words" not in edited_text.lower(), ("Selected word survived replacement", edited_text)
    assert edited_second_top - edited_first_top >= 10, (
        "Editing collapsed draft paragraphs", edited_first_top, edited_second_top)

    # Select across the newline and restore the original one-line prompt so
    # all existing fresh-session submission and transcript-growth checks run
    # unchanged. No click is allowed to rescue lost editor focus.
    native("key", "ctrl+a")
    native("type", "--clearmodifiers", "--delay", "30", prompt)
    _, restored = capture("-multiline-restored")
    assert composer_bounds(restored) == fresh
    restored_text, _ = assert_text(restored, fresh, prompt, "multiline-restored")
    assert "dictated" not in restored_text.lower(), (
        "Select-all did not replace both paragraphs", restored_text)

    native("key", "Return")
    _, sent = capture("-submitted")
    submitted = composer_bounds(sent)
    assert submitted == fresh, ("First submission moved or resized the composer", fresh, submitted)
    # Constrain the OCR region, not just the text: a prompt echoed at the bottom
    # of a mostly empty transcript must not satisfy this acceptance check.
    sent_text, prompt_top = assert_text(
        sent, (280, 50, sent.width, min(190, submitted[1])), prompt, "submitted")
    assert prompt_top < 150, ("First prompt must start at the top", prompt_top)

    # No second click: submission must retain keyboard focus in the same editor.
    followup = "Followup input retained"
    native("type", "--clearmodifiers", "--delay", "30", followup)
    _, following = capture("-followup")
    assert composer_bounds(following) == fresh
    followup_text, _ = assert_text(following, fresh, followup, "followup")
    native("key", "Return")
    _, second = capture("-second-submitted")
    assert composer_bounds(second) == fresh, "Two short prompts still fit above the composer"

    # The inert fixture accepts and locally echoes real native submissions but
    # never contacts a provider. Fill the transcript until it needs more room.
    growth_bounds = []
    for index in range(16):
        long_prompt = (f"Growth message {index + 1:02d}. "
                       + "Keep the composer steady until the conversation needs more room. " * 3)
        native("type", "--clearmodifiers", "--delay", "1", long_prompt)
        native("key", "Return")
        _, growing = capture(f"-growth-{index + 1:02d}")
        bounds = composer_bounds(growing)
        previous = growth_bounds[-1] if growth_bounds else fresh
        assert bounds[1] >= previous[1], ("Growing content moved input upward", previous, bounds)
        assert bounds[3] <= initial.height, ("Composer escaped the viewport", bounds)
        assert bounds[0] == fresh[0] and bounds[2] == fresh[2], ("Growth changed composer width", bounds)
        assert bounds[3] - bounds[1] == fresh[3] - fresh[1], ("Growth changed composer height", bounds)
        growth_bounds.append(bounds)
    assert growth_bounds[-1][1] > fresh[1], ("Composer never made room for a long transcript", growth_bounds)
    assert growth_bounds[-1] == growth_bounds[-2], ("Composer did not settle at its lower limit", growth_bounds)
    # Under concurrent builds, a captured frame can lag behind the Return key
    # even though the editor has already cleared. Wait for rendered evidence,
    # without typing/clicking again to manufacture an otherwise missing paint.
    deadline = time.monotonic() + 10
    settle_attempts = 0
    while True:
        try:
            last_prompt_text, _ = assert_text(
                growing, (280, max(50, growth_bounds[-1][1] - 240),
                          growing.width, growth_bounds[-1][1]),
                "Growth message 16.", "growth-last")
            break
        except AssertionError:
            if time.monotonic() >= deadline:
                raise
            settle_attempts += 1
            _, growing = capture(f"-growth-settled-{settle_attempts:02d}")
            assert composer_bounds(growing) == growth_bounds[-1]
    final_followup = "Focus after growth"
    native("type", "--clearmodifiers", "--delay", "15", final_followup)
    _, final = capture("-growth-followup")
    assert composer_bounds(final) == growth_bounds[-1]
    final_text, _ = assert_text(final, growth_bounds[-1], final_followup, "growth-followup")
    evidence = {
        "window": list(initial.size), "fresh_bounds": fresh, "submitted_bounds": submitted,
        "canvas_horizontal_bounds": canvas_bounds_x,
        "first_submission_movement_pixels": submitted[1] - y1,
        "first_prompt_top_pixels": prompt_top,
        "native_typed_text": typed_text, "submitted_transcript_text": sent_text,
        "multiline_draft_text": multiline_text,
        "multiline_line_tops_pixels": [first_line_top, second_line_top],
        "multiline_selection_edit_text": edited_text,
        "multiline_edited_line_tops_pixels": [edited_first_top, edited_second_top],
        "multiline_select_all_restored_text": restored_text,
        "focus_retained_followup_text": followup_text,
        "typing_keeps_position": True, "first_submission_keeps_bounds": True,
        "growth_bounds": growth_bounds, "last_growth_prompt_text": last_prompt_text,
        "growth_settle_attempts": settle_attempts,
        "focus_retained_after_growth_text": final_text,
    }
    output.with_suffix(".acceptance.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print("Fresh-session native acceptance passed: " + json.dumps(evidence))
