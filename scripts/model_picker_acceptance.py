"""Native /model acceptance on screenshot.py's private, offline X11 display.

Only rendered pixels/OCR are used as evidence. The harness state file is not a
substitute for visible search, list accessibility, focus, or model selection.
Artifacts include the open picker at OUTPUT, per-step PNGs, OCR crops, and a
JSON report (including the failing step if acceptance fails).
"""
import csv
import io
import json
import re
import shutil
import subprocess
import time
from collections import Counter

from PIL import Image

from fresh_session_acceptance import composer_bounds


# These routes are supplied by the explicitly enabled screenshot model fixture.
# Three provider groups include twelve OpenAI models, initially capped at three.
TENTH_ROUTE = "openai:atlas-10"
LAST_ROUTE = "openai:atlas-12"
FILTER_ROUTE = "anthropic:sonnet-review"
FILTER = "sonnet-review"
NO_MATCH = "zzzz-no-model-937"
FILTER_SPEC = "claude-api:sonnet-review"
LAST_SPEC = "openai-api:atlas-12"


def normalized(text):
    return re.sub(r"[^a-z0-9]", "", text.lower())


def dialog_bounds(image):
    """Find suggestions immediately above the still-visible focused composer."""
    edges = []
    for y in range(100, image.height - 20):
        xs = [x for x in range(280, image.width - 12)
              if image.getpixel((x, y))[:3] == (135, 121, 107)]
        if len(xs) >= 250:
            edges.append((y, min(xs), max(xs)))
    assert len(edges) == 4, f"Expected suggestion and composer borders, found {edges}"
    top, bottom, composer_top, composer_bottom = edges
    assert 0 < composer_top[0] - bottom[0] <= 8, ("Suggestions must be above input", edges)
    assert abs(top[1] - composer_top[1]) <= 2, ("Suggestions must align with input", edges)
    assert abs(top[2] - composer_top[2]) <= 2, ("Suggestions must match input width", edges)
    assert abs(top[1] - bottom[1]) <= 2 and abs(top[2] - bottom[2]) <= 2, edges
    return (top[1], top[0], top[2] + 1, bottom[0] + 1)


def parse_words(tsv, bounds, scale=3):
    words = []
    for row in csv.DictReader(io.StringIO(tsv), delimiter="\t"):
        if not row.get("text", "").strip():
            continue
        words.append({
            "text": row["text"],
            "x": bounds[0] + int(row["left"]) / scale,
            "y": bounds[1] + int(row["top"]) / scale,
            "width": int(row["width"]) / scale,
            "height": int(row["height"]) / scale,
        })
    return words


def phrase_bounds(words, phrase):
    """Locate OCR words allowing punctuation/spacing variation, never typos."""
    expected = normalized(phrase)
    for start in range(len(words)):
        text = ""
        for end in range(start, len(words)):
            text += normalized(words[end]["text"])
            if text == expected:
                matched = words[start:end + 1]
                return (min(word["x"] for word in matched),
                        min(word["y"] for word in matched),
                        max(word["x"] + word["width"] for word in matched),
                        max(word["y"] + word["height"] for word in matched))
            if not expected.startswith(text):
                break
    raise AssertionError(f"Visible text {phrase!r} missing from: "
                         + " ".join(word["text"] for word in words))


def selected_row(image, bounds, words, route):
    row = phrase_bounds(words, route)
    y = round((row[1] + row[3]) / 2)
    selected_pixels = sum(image.getpixel((x, y))[:3] == (228, 221, 211)
                          for x in range(bounds[0] + 1, bounds[2] - 1))
    assert selected_pixels >= 200, ("Keyboard-selected route is not visibly highlighted", route)


def normalize_menu_ocr(crop):
    """Give inverse selected rows the same polarity as other menu rows.

    Keep original captures as pixel evidence. Tesseract otherwise drops the
    light selected row while recognizing the surrounding dark menu.
    """
    crop = crop.convert("RGB").copy()
    for y in range(crop.height):
        colors = [crop.getpixel((x, y)) for x in range(crop.width)]
        background = Counter(colors).most_common(1)[0][0]
        if sum(background) > 128 * 3:
            for x, color in enumerate(colors):
                crop.putpixel((x, y), tuple(255 - channel for channel in color))
    return crop


def verify(output, env, root):
    # Refuse direct use against a user's live desktop, even if invoked manually.
    assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1", "Offline fixture required"
    assert env.get("JCODE_DESKTOP_SCREENSHOT_MODELS") == "1", "Model fixture required"
    assert env.get("XDG_RUNTIME_DIR") == str(root / "runtime"), "Private runtime required"
    assert env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"), "Private X11 required"
    report = {"checks": {}, "scope": "native UI and local selection request, not backend acknowledgement"}
    stage = "initial"

    def native(*args):
        subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root,
                       check=True, timeout=15)

    def type_text(text):
        native("type", "--clearmodifiers", "--delay", "35", text)

    def capture(label):
        path = output.with_name(output.stem + "-" + label + ".png")
        time.sleep(.35)
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=env, cwd=root, check=True, timeout=15)
        return path, Image.open(path).convert("RGB")

    def ocr(image, bounds, label):
        path = output.with_name(output.stem + "-" + label + "-ocr.png")
        crop = normalize_menu_ocr(image.crop(bounds))
        crop.resize((crop.width * 3, crop.height * 3)).save(path)
        tsv = subprocess.check_output(
            ["tesseract", str(path), "stdout", "--psm", "6", "tsv"],
            env=env, cwd=root, timeout=20, stderr=subprocess.DEVNULL,
        ).decode()
        return parse_words(tsv, bounds)

    def wait_frame(label, check):
        # Retry only painting, never re-send input to manufacture missing focus.
        deadline = time.monotonic() + 10
        while True:
            path, image = capture(label)
            try:
                result = check(image)
                return path, image, result
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise

    def picker(image, label, expected=None):
        bounds = dialog_bounds(image)
        words = ocr(image, bounds, label)
        if expected:
            phrase_bounds(words, expected)
        return bounds, words

    def open_picker(command, label):
        type_text(command)
        # /model opens as it is typed. Do not turn an OCR/paint delay into an
        # Enter press that would accidentally submit the highlighted model.
        # Only the explicit /models alias needs command submission.
        if command == "/models":
            native("key", "Return")
            trigger = "Return"
        else:
            trigger = "typing"
        report.setdefault("open_actions", {})[label] = trigger
        return wait_frame(label, lambda image: picker(image, label, FILTER_ROUTE))

    def closed(image):
        # A restored composer border proves the backdrop disappeared. The
        # current dialog border must never be mistaken for a fresh composer.
        bounds = composer_bounds(image)
        words = ocr(image, (280, 100, image.width - 12, image.height), "closed")
        assert "chooseamodel" not in normalized(" ".join(word["text"] for word in words)), "Picker stayed open"
        return bounds

    try:
        initial = Image.open(output).convert("RGB")
        bounds = composer_bounds(initial)
        native("mousemove", bounds[0] + 32, bounds[1] + 24, "click", "1")
        shutil.copyfile(output, output.with_name(output.stem + "-initial.png"))

        stage = "native-open-and-visible-search"
        path, opened, (dialog, words) = open_picker("/model", "model-open")
        shutil.copyfile(path, output)
        report["dialog_bounds"] = dialog
        report["checks"][stage] = True
        stage = "visible-model-usage-metadata"
        phrase_bounds(words, "42 tracked turns")
        phrase_bounds(words, "Last used 2h ago")
        phrase_bounds(words, "7 tracked turns")
        phrase_bounds(words, "Last used 3d ago")
        phrase_bounds(words, "Current")
        report["checks"][stage] = True

        stage = "pointer-selection-and-keyboard-handoff"
        google = phrase_bounds(words, "google:gemini-review")
        native("mousemove", round((google[0] + google[2]) / 2), round((google[1] + google[3]) / 2))
        def pointer_selected(image):
            bounds, current_words = picker(image, stage, "google:gemini-review")
            selected_row(image, bounds, current_words, "google:gemini-review")
        wait_frame("model-pointer-selected", pointer_selected)
        native("mousemove", bounds[0] + 32, bounds[1] + 24)
        native("key", "Up")
        def keyboard_restored(image):
            menu_bounds, current_words = picker(image, stage, FILTER_ROUTE)
            selected_row(image, menu_bounds, current_words, FILTER_ROUTE)
        wait_frame("model-keyboard-restored", keyboard_restored)
        report["checks"][stage] = True

        stage = "provider-groups-and-three-model-preview"
        phrase_bounds(words, "Show 9 more models")
        for route in ["openai:atlas-01", "openai:atlas-02", "openai:atlas-03"]:
            phrase_bounds(words, route)
        assert normalized("openai:atlas-04") not in normalized(" ".join(word["text"] for word in words)), "Collapsed group shows a fourth model"
        report["checks"][stage] = True

        stage = "mouse-expand-provider-without-submitting"
        x1, y1, x2, y2 = phrase_bounds(words, "Show 9 more models")
        native("mousemove", round((x1 + x2) / 2), round((y1 + y2) / 2), "click", "1")
        # Expansion retains the composer and exposes the provider's full list.
        native("mousemove", dialog[2] - 45, dialog[3] - 70,
               "click", "--repeat", "12", "--delay", "60", "5")
        _, _, (_, expanded_words) = wait_frame("model-provider-expanded", lambda image: picker(image, stage, LAST_ROUTE))
        phrase_bounds(expanded_words, "Show fewer models")
        report["checks"][stage] = True

        stage = "keyboard-collapse-provider-without-submitting"
        x1, y1, x2, y2 = phrase_bounds(expanded_words, "Show fewer models")
        native("mousemove", round((x1 + x2) / 2), round((y1 + y2) / 2))
        native("key", "Return")
        _, _, (dialog, words) = wait_frame("model-provider-collapsed", lambda image: picker(image, stage, "Show 9 more models"))
        assert normalized(LAST_ROUTE) not in normalized(" ".join(word["text"] for word in words)), "Collapse kept hidden models visible"
        report["checks"][stage] = True

        stage = "keyboard-expand-provider-without-submitting"
        native("key", "Return")
        native("mousemove", dialog[2] - 45, dialog[1] + 30,
               "click", "--repeat", "18", "--delay", "60", "4")
        _, _, (dialog, words) = wait_frame("model-provider-reexpanded", lambda image: picker(image, stage, "openai:atlas-01"))
        report["checks"][stage] = True

        stage = "keyboard-beyond-eight"
        native("key", "--clearmodifiers", "--delay", "40", *(["Up"] * 20))
        native("key", "--clearmodifiers", "--delay", "80", *(["Down"] * 9))
        def keyboard_scrolled(image):
            bounds, words = picker(image, stage, TENTH_ROUTE)
            selected_row(image, bounds, words, TENTH_ROUTE)
            return bounds, words
        _, _, (_, words) = wait_frame("model-keyboard-scroll", keyboard_scrolled)
        report["checks"][stage] = " ".join(word["text"] for word in words)

        stage = "mouse-scroll-to-last-route"
        native("mousemove", dialog[2] - 45, dialog[3] - 70,
               "click", "--repeat", "12", "--delay", "60", "5")
        wait_frame("model-wheel-scroll", lambda image: picker(image, stage, LAST_ROUTE))
        report["checks"][stage] = True

        stage = "focused-filter-and-scroll-reset"
        type_text(" " + FILTER + " ")
        def filtered(image):
            bounds, words = picker(image, stage, FILTER_ROUTE)
            # The query remains in the real composer below the suggestions.
            query_words = ocr(image, (bounds[0], bounds[3] + 1, bounds[2], min(image.height, bounds[3] + 160)), stage + "-query")
            phrase_bounds(query_words, FILTER)
            assert normalized(LAST_ROUTE) not in normalized(" ".join(word["text"] for word in words)), "Filtering kept unrelated routes"
            return bounds, words
        wait_frame("model-filtered", filtered)
        report["checks"][stage] = True

        stage = "no-matches-and-enter-does-not-submit"
        native("key", "ctrl+a")
        type_text("/model " + NO_MATCH)
        wait_frame("model-no-matches", lambda image: picker(image, stage, "No models match"))
        native("key", "Return")
        wait_frame("model-no-matches-enter", lambda image: picker(image, stage, "No models match"))
        report["checks"][stage] = True

        stage = "escape-restores-native-composer-focus"
        native("key", "Escape")
        _, _, bounds = wait_frame("model-escaped", closed)
        type_text("Composer focus restored ")
        wait_frame("model-focus-restored", lambda image: phrase_bounds(
            ocr(image, bounds, stage), "Composer focus restored"))
        native("key", "ctrl+a", "BackSpace")
        report["checks"][stage] = True

        stage = "models-alias"
        open_picker("/models", "models-alias")
        report["checks"][stage] = True

        stage = "keyboard-selection"
        type_text(" " + FILTER + " ")
        wait_frame("model-keyboard-filter", filtered)
        native("key", "Return")
        def selected(image, route):
            closed(image)
            words = ocr(image, (280, 50, image.width - 12, image.height - 30), stage)
            phrase_bounds(words, "Switching model to")
            phrase_bounds(words, route)
            return True
        wait_frame("model-keyboard-selected", lambda image: selected(image, FILTER_SPEC))
        report["checks"][stage] = FILTER_SPEC

        stage = "mouse-selection"
        open_picker("/model", "model-mouse-open")
        type_text(" " + LAST_ROUTE.split(":")[-1])
        _, _, (_, words) = wait_frame("model-mouse-filter", lambda image: picker(image, stage, LAST_ROUTE))
        x1, y1, x2, y2 = phrase_bounds(words, LAST_ROUTE)
        native("mousemove", round((x1 + x2) / 2), round((y1 + y2) / 2), "click", "1")
        wait_frame("model-mouse-selected", lambda image: selected(image, LAST_SPEC))
        report["checks"][stage] = LAST_SPEC

        stage = "search-finds-collapsed-model"
        _, _, (_, words) = open_picker("/model", "model-search-collapsed-open")
        # Search must reveal a model outside the initial three choices.
        type_text(" " + LAST_ROUTE.split(":")[-1])
        wait_frame("model-search-collapsed-result", lambda image: picker(image, stage, LAST_ROUTE))
        native("key", "Escape")
        wait_frame("model-search-collapsed-closed", closed)
        report["checks"][stage] = True
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, failed_step=stage, error=str(error))
        raise
    finally:
        output.with_suffix(".acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Model-picker native acceptance: " + json.dumps(report))
