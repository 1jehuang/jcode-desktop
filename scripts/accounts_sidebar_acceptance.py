"""Account sidebar acceptance using only screenshot.py's private offline display."""
import json
import shutil

from default_directory_acceptance import NativeUI, phrase_bounds


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    report = {"passed": False}
    try:
        ui.native("mousemove", 132, 30)

        def find_accounts(image):
            words = ui.words(image, (0, 52, 264, 340), "accounts-menu", psm=11)
            return phrase_bounds(words, "accounts")

        ui.click(ui.wait_frame("accounts-menu", find_accounts))
        ui.native("mousemove", 700, 600)

        def check(image):
            words = ui.words(image, (0, 80, 264, image.height), "accounts-statuses", psm=11)
            labels = ["Connected", "Not connected", "Signed in", "Not signed in",
                      "Not configured", "Expired", "Gemini", "OpenRouter", "Copilot"]
            bounds = {label: phrase_bounds(words, label) for label in labels}
            assert bounds["Connected"][1] < bounds["Not connected"][1]
            assert bounds["Not connected"][1] < bounds["Gemini"][1]
            return bounds

        report["labels"] = ui.wait_frame("accounts-sidebar", check)
        shutil.copyfile(ui.artifact("accounts-sidebar.png"), output)

        # The fixture's Claude login has a redeemable session reset. Its pill
        # opens a review only: offline, the check never reaches a provider and
        # no confirm action exists, so nothing can be spent here.
        def find_reset(image):
            words = ui.words(image, (0, 80, 264, image.height), "accounts-reset", psm=11)
            return phrase_bounds(words, "Reset session")

        report["reset_pill"] = ui.wait_frame("accounts-reset", find_reset)
        ui.click(report["reset_pill"])
        ui.native("mousemove", 1200, 700)

        def find_dialog(image):
            # The dialog is centred. Crop to it so the dimmed backdrop is not OCR noise.
            box = (image.width // 4, image.height // 3, image.width * 3 // 4, image.height * 2 // 3)
            words = ui.words(image, box, "usage-reset-dialog", psm=6)
            bounds = {label: phrase_bounds(words, label)
                      for label in ["Reset Claude session limit", "default Claude login",
                                    "Checking which resets this login can use", "Cancel"]}
            try:
                phrase_bounds(words, "Reset now")
            except AssertionError:
                pass
            else:
                raise RuntimeError("offline review must not offer a confirm action")
            return bounds

        report["dialog"] = ui.wait_frame("usage-reset-dialog", find_dialog)
        shutil.copyfile(ui.artifact("usage-reset-dialog.png"), ui.artifact("reset-review.png"))
        ui.click(report["dialog"]["Cancel"])

        def dialog_closed(image):
            box = (image.width // 4, image.height // 3, image.width * 3 // 4, image.height * 2 // 3)
            words = ui.words(image, box, "usage-reset-closed", psm=6)
            try:
                phrase_bounds(words, "Reset Claude session limit")
            except AssertionError:
                return True
            raise AssertionError("reset dialog is still open")

        report["dialog_closed"] = ui.wait_frame("usage-reset-closed", dialog_closed)
        report["passed"] = True
    finally:
        ui.artifact("accounts-sidebar.json").write_text(json.dumps(report, indent=2) + "\n")
