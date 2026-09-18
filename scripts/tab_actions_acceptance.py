"""Native hover-only tab actions and shortcut tooltips on screenshot.py's private display."""
import json
import shutil

from default_directory_acceptance import NativeUI
from model_picker_acceptance import phrase_bounds


def hover_actions(ui):
    state = (ui.root / "state").read_text()
    navigation = json.loads(next(line.removeprefix("navigation=") for line in state.splitlines()
                                 if line.startswith("navigation=")))
    image = ui.capture("tab-actions-initial")
    # The single-tab fixture's native layout target, not a debug selector.
    left = image.width - 12 - navigation["canvas_width"]
    center = left + navigation["tab_targets"][0][1]
    rename = (round(center + 53), 16, round(center + 73), 36)
    close = (round(center + 77), 16, round(center + 97), 36)

    def ink(image, bounds):
        x1, y1, x2, y2 = bounds
        return sum(max(pixel) > 110 for pixel in image.crop(
            (x1 + 3, y1 + 3, x2 - 3, y2 - 3)).getdata())

    ui.native("mousemove", 400, 400)
    hidden = ui.capture("tab-actions-hidden")
    assert ink(hidden, rename) == 0 and ink(hidden, close) == 0, "Actions must be hidden at rest"
    ui.native("mousemove", round(center), 26)
    hovered = ui.capture("tab-actions-hover")
    assert ink(hovered, rename) > 3, "Rename icon did not appear beside title"
    assert ink(hovered, close) > 3, "Close icon did not appear beside title"
    # Hovering must not relayout/truncate the title differently.
    title_crop = (round(center - 74), 17, round(center + 43), 35)
    title_before = phrase_bounds(ui.words(hidden, title_crop, "title-before", psm=7), "Review markdown")
    title_after = phrase_bounds(ui.words(hovered, title_crop, "title-after", psm=7), "Review markdown")
    assert title_before == title_after, "Title shifted on hover"

    def tooltip(bounds, phrase, label):
        ui.native("mousemove", round((bounds[0] + bounds[2]) / 2), 26)
        ui.wait_frame(label, lambda image: phrase_bounds(
            ui.words(image, (500, 34, image.width, 58), label, psm=7), phrase))

    tooltip(rename, "Rename session (F2)", "tab-rename-shortcut")
    tooltip(close, "Close tab (Super+Q)", "tab-close-shortcut")
    tooltip((image.width - 80, 0, image.width - 56, 36), "New session (Super+N)", "tab-new-shortcut")
    ui.native("mousemove", 400, 400)
    hidden_again = ui.capture("tab-actions-hidden-again")
    assert ink(hidden_again, rename) == 0 and ink(hidden_again, close) == 0, "Actions stayed visible after leaving"
    return rename, close


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    report = {"passed": False}
    try:
        rename, close = hover_actions(ui)
        ui.click(rename)
        ui.wait_frame("tab-actions-rename-dialog", lambda image: phrase_bounds(
            ui.words(image, (450, 300, 1000, 700), "tab-actions-dialog", psm=11), "Rename session"))
        ui.native("key", "Escape")
        ui.click(close)
        ui.wait_frame("tab-actions-closed", lambda image: phrase_bounds(
            ui.words(image, (794, 17, 966, 35), "tab-actions-empty", psm=7), "Empty workspace"))
        shutil.copyfile(ui.artifact("tab-actions-hover.png"), output)
        report.update(passed=True, checks=["hidden-at-rest", "icons-beside-title", "no-title-shift",
            "rename-shortcut", "close-shortcut", "new-shortcut", "hidden-on-leave",
            "rename-click-opens", "close-click-closes-tab-not-window"])
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        output.with_suffix(".tab-actions.json").write_text(json.dumps(report, indent=2) + "\n")
