"""Native input and rendered-header acceptance on screenshot.py's private display."""
import json
import subprocess
import time

from PIL import Image


def header_pixels(image, height, minimap):
    """FPS occupies the left slot of the tab row, not a separate top strip."""
    assert height == 0, ("the separate header must be removed", height)
    # This acceptance fixture uses the default folder layout and full sidebar.
    slot = image.crop((276, 10, 364, 42))
    background = slot.getpixel((0, 0))
    changed = [(x, y) for y in range(slot.height) for x in range(slot.width)
               if slot.getpixel((x, y)) != background]
    assert changed, "FPS text is missing"
    assert all(0 < x < slot.width - 1 and 0 < y < slot.height - 1 for x, y in changed), "FPS content is clipped"
    # The new-session plus is always visible to the right of the tabs, before the minimap.
    right = image.width - 12 - (154 if minimap else 0)
    plus = image.crop((right - 32, 10, right, 42))
    assert any(pixel != plus.getpixel((0, 0)) for pixel in plus.getdata()), "tab plus is missing"
    return {"header_height": height, "text_pixels": len(changed), "background": background}


def verify(output, env, root):
    state = root / "state"
    evidence = [output.with_name(f"{output.stem}-workspace-{row + 1}.png") for row in range(4)]
    report = output.with_suffix(".fps-header.json")
    if any(path.exists() for path in [*evidence, report]):
        raise FileExistsError("FPS evidence exists; choose a fresh output name")

    def nav():
        return json.loads(next(line.partition("=")[2] for line in state.read_text().splitlines()
                               if line.startswith("navigation=")))

    def native(*args):
        subprocess.run(["xdotool", *args], env=env, cwd=root, check=True, timeout=10)
        time.sleep(.35)

    height = round(nav()["header_height"])
    for slot in [1, 2, 3]:
        x = dict(nav()["tab_targets"])[slot] + 276
        native("mousemove", str(round(x)), str(height + 34), "click", "1")
        assert nav()["focused_slot"] == slot, nav()
        for _ in range(slot):
            native("key", "super+shift+j")
        assert nav()["active_row"] == slot, nav()
    assert all(len(row["panels"]) == 1 for row in nav()["rows"])
    for _ in range(3):
        native("key", "super+k")
    frames = []
    for row, path in enumerate(evidence):
        if row:
            native("key", "super+j")
        current = nav()
        assert current["active_row"] == row and current["keyboard_panel"] == row, current
        native("mousemove", "270", "400")
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=env, cwd=root, check=True, timeout=15)
        with Image.open(path) as image:
            pixels = header_pixels(image.convert("RGB"), height, current["minimap_visible"])
        frames.append(dict(workspace=row, keyboard_panel=current["keyboard_panel"],
                           screenshot=path.name, **pixels))
    width = nav()["viewport"][0]
    native("mousemove", str(round(width - 24)), "400", "click", "1")
    assert sum(len(row["panels"]) for row in nav()["rows"]) == 4, "old edge target still creates sessions"
    native("mousemove", str(round(width - 12 - (154 if nav()["minimap_visible"] else 0) - 16)), "26", "click", "1")
    current = nav()
    assert sum(len(row["panels"]) for row in current["rows"]) == 5, "tab plus did not create exactly one session"
    assert current["active_row"] == 3 and len(current["rows"][3]["panels"]) == 2, current
    assert current["keyboard_panel"] == current["focused_slot"], "new composer did not receive focus"
    report.write_text(json.dumps(dict(passed=True, frames=frames, plus_created_session=True,
                                     edge_did_not_create_session=True), indent=2) + "\n")
    print("FPS tab row PASS: left FPS, right plus, native tab navigation across 4 workspaces, new-session focus, and removed edge target", flush=True)
