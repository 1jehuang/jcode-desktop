"""Native rename acceptance on screenshot.py's private offline Xvfb display.

Run only with a current binary:
    python3 scripts/screenshot.py target/rename.png --no-build \
        --transcript empty --rename-interact

Mouse/keyboard input and rendered OCR are the evidence, not debug selectors.
OUTPUT is the final open dialog. Step PNGs, OCR and a .rename.json report persist.
The fixture bridge ignores commands, so Enter is checked for dismissal only,
not SDK delivery, persisted titles, or a renamed tab/history entry.
"""
import json
import shutil

from default_directory_acceptance import NativeUI
from fresh_session_acceptance import composer_bounds
from model_picker_acceptance import normalized, phrase_bounds


DRAFT = "Keep this composer draft safe"
REPLACEMENT = "Native rename acceptance"


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    report = {
        "checks": {},
        "scope": "native offline rename UI and composer preservation",
        "limitations": ["Fixture bridge ignores commands. SDK delivery, persistence, "
                         "and visible renamed titles are not asserted."],
    }
    stage = "initial"

    def record(label):
        report["checks"][label] = True
        print(f"Rename check passed: {label}", flush=True)

    def center_words(image, label):
        return ui.words(image, (image.width // 2 - 240, image.height // 2 - 240,
                                image.width // 2 + 240, image.height // 2 + 240),
                        label, psm=11)

    def dialog(image, label):
        words = center_words(image, label)
        heading = phrase_bounds(words, "Rename session")
        # Restrict controls below the heading to avoid matching shortcut hints.
        controls = [word for word in words if word["y"] > heading[3] + 35]
        cancel = phrase_bounds(controls, "Cancel")
        save = phrase_bounds(controls, "Save")
        assert abs((heading[1] + save[3]) / 2 - image.height / 2) < 100, \
            "Rename dialog is not vertically centered"
        assert image.width / 2 - 225 < heading[0] < image.width / 2, \
            "Rename dialog is not horizontally centered"
        return words, cancel, save

    def opened(label):
        return ui.wait_frame(label, lambda image: dialog(image, label))

    def closed(image):
        words = center_words(image, stage + "-center")
        text = normalized(" ".join(word["text"] for word in words))
        assert "renamesession" not in text, "Rename dialog stayed open"
        # Positive evidence avoids treating failed/empty OCR as dismissal.
        # Reuse the measured composer rectangle. Focus restoration can repaint
        # its border separately from the preserved text.
        draft_words = ui.words(image, bounds, stage + "-draft")
        actual = normalized(" ".join(word["text"] for word in draft_words))
        assert normalized(DRAFT) in actual, "Composer draft was lost"
        assert normalized(REPLACEMENT) not in actual, "Rename text leaked into composer"

    def type_text(text):
        ui.native("type", "--clearmodifiers", "--delay", "25", text)

    def key(value):
        ui.native("key", "--clearmodifiers", value)

    try:
        image = ui.capture("rename-initial")
        bounds = composer_bounds(image)
        ui.click((bounds[0] + 20, bounds[1] + 12, bounds[0] + 80, bounds[1] + 36))
        type_text(DRAFT)
        ui.wait_frame("rename-draft", closed)

        stage = "native-button-opens"
        from tab_actions_acceptance import hover_actions
        button, _ = hover_actions(ui)
        ui.click(button)
        opened("rename-button-dialog")
        record(stage)

        stage = "escape-cancels-preserves-draft"
        key("Escape")
        ui.wait_frame(stage, closed)
        record(stage)

        stage = "f2-opens"
        key("F2")
        opened("rename-f2-dialog")
        record(stage)

        stage = "preselected-title-replaced"
        # Deliberately do not press Ctrl+A. Typing must replace the initial title.
        type_text(REPLACEMENT)

        def replaced(image):
            words, cancel, _ = dialog(image, stage)
            hint = phrase_bounds(words, "Enter to save")
            field = ui.words(image, (image.width // 2 - 180, round(hint[3] + 24),
                                     image.width // 2 + 180, round(cancel[1] - 30)),
                             stage + "-field", psm=7)
            actual = normalized(" ".join(word["text"] for word in field))
            # OCR may read the insertion caret as an extra I/l. The original
            # fixture title must be gone, and the full replacement must render.
            assert normalized(REPLACEMENT) in actual and "reviewmarkdown" not in actual, (
                "Title was not replaced", actual)

        ui.wait_frame(stage, replaced)
        record(stage)

        stage = "enter-dismisses-preserves-draft"
        key("Return")
        ui.wait_frame(stage, closed)
        record(stage)

        stage = "empty-save-validation"
        key("F2")
        opened("rename-before-empty")
        key("ctrl+a")
        key("BackSpace")
        _, _, save = opened("rename-empty")
        ui.click(save)

        def invalid(image):
            words, _, _ = dialog(image, stage)
            phrase_bounds(words, "Enter a session title")

        ui.wait_frame(stage, invalid)
        record(stage)

        stage = "cancel-closes-preserves-draft"
        _, cancel, _ = opened("rename-before-cancel")
        ui.click(cancel)
        ui.wait_frame(stage, closed)
        record(stage)

        stage = "final-dialog-capture"
        key("F2")
        opened("rename-final-dialog")
        shutil.copyfile(ui.artifact("rename-final-dialog.png"), output)
        report["final_image"] = str(output)
        report["passed"] = True
        record(stage)
    except Exception as error:
        report.update(passed=False, failed_stage=stage, error=str(error))
        raise
    finally:
        for source, suffix in ((root / "app.log", "rename-app.log"),
                               (root / "logs/jcode-desktop/jcode-desktop.log", "rename-desktop.log")):
            if source.exists():
                shutil.copyfile(source, ui.artifact(suffix))
        output.with_suffix(".rename.json").write_text(json.dumps(report, indent=2) + "\n")
