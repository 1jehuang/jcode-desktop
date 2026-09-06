"""Measure rendered pixels and native keyboard behavior on a private X11 display."""
import csv
import io
import json
import subprocess
import time

from PIL import Image


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
    expected_center_x = (276 + initial.width - 12) / 2
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
        "first_submission_movement_pixels": submitted[1] - y1,
        "first_prompt_top_pixels": prompt_top,
        "native_typed_text": typed_text, "submitted_transcript_text": sent_text,
        "focus_retained_followup_text": followup_text,
        "typing_keeps_position": True, "first_submission_keeps_bounds": True,
        "growth_bounds": growth_bounds, "last_growth_prompt_text": last_prompt_text,
        "growth_settle_attempts": settle_attempts,
        "focus_retained_after_growth_text": final_text,
    }
    output.with_suffix(".acceptance.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print("Fresh-session native acceptance passed: " + json.dumps(evidence))
