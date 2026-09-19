"""Native pending-startup Enter regression on the private offline Xvfb display.

Run: python3 scripts/screenshot.py target/pending.png --transcript empty --pending-interact
Only real keyboard/mouse input drives submission, retry and local fallback.
Rendered pixels/OCR prove queue preservation and editor clearing. The inert
fixture bridge cannot deliver a prompt or connect to a provider or remote host.
"""
import json

from default_directory_acceptance import NativeUI
from fresh_session_acceptance import composer_bounds
from model_picker_acceptance import normalized, phrase_bounds


PROMPT = "Keep this pending prompt safe"
DRAFT = "Preserve my next editor draft"
FOLLOWUP = " and keep typing locally"


def verify(output, env, root):
    assert env.get("JCODE_DESKTOP_SCREENSHOT_PENDING") == "1"
    assert env.get("JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT") == "empty"
    ui = NativeUI(output, env, root)
    report = {
        "checks": {},
        "scope": "native offline pending startup, Enter queue, Retry, explicit local fallback",
        "limitations": ["The inert bridge does not create a session or deliver queued prompts."],
    }
    stage = "initial-failure"

    def words(image, label):
        return ui.words(image, (280, 50, image.width - 12, image.height - 16), label, psm=11)

    def text(rows):
        return normalized(" ".join(word["text"] for word in rows))

    def require(rows, expected):
        assert normalized(expected) in text(rows), (expected, text(rows))

    def record(label):
        report["checks"][label] = True
        print("Pending submission check passed: " + label, flush=True)

    def type_text(value):
        ui.native("type", "--clearmodifiers", "--delay", "25", value)

    def editor(image, label):
        bounds = composer_bounds(image)
        return bounds, ui.words(image, bounds, label + "-editor")

    def queued(image, label, draft=None):
        bounds, editor_words = editor(image, label)
        queue_words = ui.words(image, (280, bounds[3], image.width - 12, image.height - 16),
                               label + "-queue", psm=11)
        # The small monospace u can OCR as v. Restrict this tolerance to
        # the caption, while requiring the complete prompt and action exactly.
        assert any(caption in text(queue_words) for caption in ("queued", "queved")), text(queue_words)
        require(queue_words, "sends when connected")
        require(queue_words, PROMPT)
        require(queue_words, "Remove")
        assert normalized(PROMPT) not in text(editor_words), "Enter did not clear the editor"
        if draft:
            require(editor_words, draft)
        else:
            # The caret overlaps the T and can make OCR read H. Require
            # only this known placeholder variant, never arbitrary empty OCR.
            assert text(editor_words) in ("typesomething", "hypesomething"), text(editor_words)
        return words(image, label + "-panel")

    try:
        def initial(image):
            rows = words(image, stage)
            require(rows, "offline-remote: connection failed (offline fixture)")
            require(rows, "Not sent")
            phrase_bounds(rows, "Retry connection")
            phrase_bounds(rows, "Use this computer")
            return composer_bounds(image)

        bounds = ui.wait_frame(stage, initial)
        record(stage)
        ui.click((bounds[0] + 20, bounds[1] + 12, bounds[0] + 100, bounds[1] + 36))
        type_text(PROMPT)
        stage = "native-draft"
        ui.wait_frame(stage, lambda image: require(editor(image, stage)[1], PROMPT))
        record(stage)

        ui.native("key", "--clearmodifiers", "Return")
        stage = "enter-queues-clears-editor"
        def entered(image):
            rows = queued(image, stage)
            require(rows, "connection failed")
            require(rows, "Not sent")
        ui.wait_frame(stage, entered)
        record(stage)

        # No click after Enter: typing proves the real editor retains focus.
        type_text(DRAFT)
        stage = "focus-retained-after-enter"
        ui.wait_frame(stage, lambda image: queued(image, stage, DRAFT))
        record(stage)

        stage = "retry-connecting-preserves-content"
        retry = ui.wait_frame("retry-button", lambda image:
                              phrase_bounds(words(image, "retry-button"), "Retry connection"))
        ui.click(retry)
        def retried(image):
            rows = queued(image, stage, DRAFT)
            require(rows, "Retrying connection")
            assert "connectionfailed" not in text(rows), "Stale remote error after Retry"
            assert "notsent" not in text(rows), "Failure banner did not clear after Retry"
            return phrase_bounds(rows, "Use this computer")
        local = ui.wait_frame(stage, retried)
        record(stage)

        ui.click(local)
        stage = "explicit-local-preserves-queue-and-draft"
        def switched(image, draft=DRAFT):
            rows = queued(image, stage, draft)
            require(rows, "Connecting to this computer")
            assert "connectionfailed" not in text(rows), "Remote error remained after local switch"
            assert "notsent" not in text(rows), "Failure banner remained after local switch"
            assert "retryconnection" not in text(rows), "Stale Retry action remained"
            assert "usethiscomputer" not in text(rows), "Remote fallback action remained"
        ui.wait_frame(stage, switched)
        record(stage)

        # The switch must restore focus without replacing the editor or queue.
        type_text(FOLLOWUP)
        stage = "native-typing-after-local-switch"
        ui.wait_frame(stage, lambda image: switched(image, DRAFT + FOLLOWUP))
        record(stage)
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, failed_stage=stage, error=str(error))
        ui.capture("pending-failure")
        raise
    finally:
        ui.artifact("pending.acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
    print("Pending submission native acceptance passed")
