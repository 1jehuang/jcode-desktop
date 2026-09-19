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
        report["passed"] = True
    finally:
        ui.artifact("accounts-sidebar.json").write_text(json.dumps(report, indent=2) + "\n")
