"""Measure rendered pixels and native keyboard behavior on a private X11 display."""
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
        text = subprocess.check_output(
            ["tesseract", str(path), "stdout", "--psm", "6"],
            env=env, cwd=root, timeout=15, stderr=subprocess.DEVNULL,
        ).decode()
        assert expected.lower() in " ".join(text.lower().split()), (expected, text)
        return text.strip()

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
    typed_text = assert_text(typed, fresh, prompt, "typed")

    native("key", "Return")
    _, sent = capture("-submitted")
    compact = composer_bounds(sent)
    assert compact[1] > initial.height * .85, compact
    assert y2 - y1 >= (compact[3] - compact[1]) * 2.5, (fresh, compact)
    assert compact[1] - y1 > initial.height * .25, (fresh, compact)
    sent_text = assert_text(sent, (280, 50, sent.width, compact[1]), prompt, "submitted")

    # No second click: keyboard focus must follow the editor to the bottom.
    followup = "Followup input retained"
    native("type", "--clearmodifiers", "--delay", "30", followup)
    _, following = capture("-followup")
    assert composer_bounds(following) == compact
    followup_text = assert_text(following, compact, followup, "followup")
    evidence = {
        "window": list(initial.size), "fresh_bounds": fresh, "compact_bounds": compact,
        "height_ratio": (y2 - y1) / (compact[3] - compact[1]),
        "moved_up_pixels": compact[1] - y1,
        "native_typed_text": typed_text, "submitted_transcript_text": sent_text,
        "focus_retained_followup_text": followup_text,
        "typing_keeps_position": True,
    }
    output.with_suffix(".acceptance.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print("Fresh-session native acceptance passed: " + json.dumps(evidence))
