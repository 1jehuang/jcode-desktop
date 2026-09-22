"""Verify the real startup countdown and dismissal on screenshot.py's private display."""
import json

from default_directory_acceptance import NativeUI
from fresh_session_acceptance import composer_bounds
from model_picker_acceptance import normalized, phrase_bounds


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    report = {"checks": {}, "scope": "native offline startup overlay"}
    draft = "Ready to work after the beta notice"

    def words(image, label):
        return ui.words(image, (0, 0, image.width, image.height), label, psm=11)

    def opened(image):
        text = words(image, "beta-launch")
        phrase_bounds(text, "Jcode Desktop is in beta testing")
        return phrase_bounds(text, "Got it")

    def dismissed(image):
        text = normalized(" ".join(word["text"] for word in words(image, "beta-dismissed")))
        assert "betatesting" not in text, "Startup notice remained visible"
        bounds = composer_bounds(image)
        # The shared helper scans from x=280 for older sidebar widths. Recover
        # the full horizontal border so a narrower sidebar cannot clip the draft.
        border = image.getpixel((bounds[0], bounds[1]))[:3]
        while bounds[0] > 0 and image.getpixel((bounds[0] - 1, bounds[1]))[:3] == border:
            bounds[0] -= 1
        actual = normalized(" ".join(word["text"] for word in ui.words(image, bounds, "beta-draft")))
        assert normalized(draft) in actual, "Dismissal did not restore composer typing"

    def expired(image):
        text = normalized(" ".join(word["text"] for word in words(image, "beta-expired")))
        assert "betatesting" not in text, "Startup countdown has not expired"

    try:
        ui.wait_frame("beta-startup", opened)
        report["checks"]["shown-on-launch"] = True
        # Do not click or press any key. The timer must dismiss the overlay.
        # Exact three-second timing and repaint stability are covered by the
        # deterministic GPUI tests, independent of OCR/capture overhead here.
        ui.wait_frame("beta-auto-dismissed", expired)
        report["checks"]["automatically-dismisses"] = True
        # Do not click the composer. Dismissal must restore its focus itself.
        ui.native("type", "--clearmodifiers", "--delay", "25", draft)
        ui.wait_frame("beta-dismissed", dismissed)
        report["checks"]["countdown-restores-focus"] = True
        # Re-render and change sidebar visibility without spawning another notice.
        ui.native("key", "--clearmodifiers", "super+b")
        ui.native("key", "--clearmodifiers", "super+b")
        ui.wait_frame("beta-stays-dismissed", dismissed)
        report["checks"]["stays-dismissed-during-work"] = True
        report["passed"] = True
    except Exception as error:
        report["passed"] = False
        report["error"] = repr(error)
        raise
    finally:
        output.with_suffix(".beta.json").write_text(json.dumps(report, indent=2) + "\n")
    print("Beta startup overlay acceptance passed", flush=True)
