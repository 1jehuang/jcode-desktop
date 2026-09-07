"""Native sidebar pointer acceptance on screenshot.py's private Xvfb only."""
import json
import time
from collections import Counter

from default_directory_acceptance import NativeUI, panels, phrase_bounds, normalized


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    observations = []

    def nav():
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            try:
                line = next(line for line in (root / "state").read_text().splitlines()
                            if line.startswith("navigation="))
                return json.loads(line.partition("=")[2])
            except (OSError, StopIteration, json.JSONDecodeError):
                time.sleep(.01)
        raise AssertionError("No navigation state")

    def settled(label):
        time.sleep(.7)
        state = nav()
        observations.append({"stage": label, "navigation": state})
        return state

    def active(state):
        return next(panel["session"] for panel in panels(state)
                    if panel["slot"] == state["focused_slot"])

    def live(state):
        return {panel["session"] for panel in panels(state) if not panel["closing"]}

    def locate(phrase, label):
        image = ui.capture(label)
        words = ui.words(image, (8, 140, 256, image.height - 20), label)
        bounds = phrase_bounds([word for word in words if normalized(word["text"])], phrase)
        return round((bounds[1] + bounds[3]) / 2)

    def drag(y, dx, dy=0, wheel=False):
        ui.native("mousemove", 180, y, "mousedown", 1)
        try:
            if wheel:
                ui.native("click", 5)
            for step in range(1, 9):
                ui.native("mousemove", round(180 + dx * step / 8), round(y + dy * step / 8))
                time.sleep(.015)
        finally:
            ui.native("mouseup", 1)

    initial = settled("initial")
    assert live(initial) == {"screenshot-fixture", "screenshot-fixture-1"}, initial
    # Put the second session on another workspace, then return to the first.
    ui.native("key", "super+shift+j")
    moved = settled("moved-down")
    assert moved["active_row"] == 1, moved
    ui.native("key", "super+k")
    before = settled("returned-upper")
    assert active(before) == "screenshot-fixture", before
    y = locate("Review folder 2", "target-row")
    upper_y = locate("Review markdown", "upper-row")

    ui.native("mousemove", 800, 650)
    hidden = ui.capture("sidebar-close-hidden")
    ui.native("mousemove", 100, y)
    hovered = ui.capture("sidebar-close-hover")
    ui.native("mousemove", 100, upper_y)
    elsewhere = ui.capture("sidebar-close-other-row")

    def ink(image, row_y):
        # The close cell lives after the truncated title and before the gutter.
        crop = image.crop((214, row_y - 9, 236, row_y + 10))
        pixels = list(crop.getdata())
        background = Counter(pixels).most_common(1)[0][0]
        return sum(max(abs(a - b) for a, b in zip(pixel, background)) > 45
                   for pixel in pixels)

    counts = {"hidden": ink(hidden, y), "hovered": ink(hovered, y),
              "other_row": ink(elsewhere, y)}
    assert counts["hovered"] > counts["hidden"] + 5, counts
    assert counts["other_row"] <= counts["hidden"] + 3, counts

    for label, dx, dy, wheel in [("short", -35, 0, False),
                                 ("vertical", -90, 35, False),
                                 ("wheel", -90, 0, True),
                                 ("rightward", 70, 0, False)]:
        drag(y, dx, dy, wheel)
        state = settled(label)
        assert live(state) == live(before), (label, state)
        assert active(state) == "screenshot-fixture", (label, state)

    # A real click still navigates and returns focus to the composer.
    ui.native("mousemove", 100, y, "click", 1)
    clicked = settled("clicked-lower")
    assert active(clicked) == "screenshot-fixture-1", clicked
    assert clicked["keyboard_panel"] == clicked["focused_slot"], clicked
    ui.native("key", "super+k")
    settled("return-before-close")
    y = locate("Review folder 2", "target-after-selection")
    drag(y, -90)
    closed = settled("left-drag-closed")
    assert live(closed) == {"screenshot-fixture"}, closed
    assert active(closed) == "screenshot-fixture", closed
    assert closed["active_row"] == 0, closed
    assert closed["keyboard_panel"] == closed["focused_slot"], closed
    ui.capture("sidebar-drag-closed")
    upper_y = locate("Review markdown", "surviving-row")
    ui.native("mousemove", 225, upper_y, "click", 1)
    button_closed = settled("close-button-clicked")
    assert "screenshot-fixture" not in live(button_closed), button_closed
    ui.capture("sidebar-button-closed")
    report = {"passed": True, "hover_ink_pixels": counts, "observations": observations}
    path = output.with_suffix(".sidebar-gesture.json")
    path.write_text(json.dumps(report, indent=2) + "\n")
    print("Sidebar gesture acceptance: " + str(path))
