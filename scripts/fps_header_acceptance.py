"""Native input and rendered-header acceptance on screenshot.py's private display."""
import json
import subprocess
import time

from PIL import Image


def header_pixels(image, height):
    """The backing strip is flat, with only a small centered text readout."""
    assert height == 20, ("unexpected header height", height)
    width = image.width
    background = image.getpixel((0, 0))
    changed = [(x, y) for y in range(height) for x in range(width)
               if image.getpixel((x, y)) != background]
    assert changed, "FPS text is missing"
    assert all(abs(x - width / 2) < 65 for x, _ in changed), "header text is not centered"
    assert all(0 < y < height - 1 for _, y in changed), "header content is clipped"
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
            pixels = header_pixels(image.convert("RGB"), height)
        frames.append(dict(workspace=row, keyboard_panel=current["keyboard_panel"],
                           screenshot=path.name, **pixels))
    report.write_text(json.dumps(dict(passed=True, frames=frames), indent=2) + "\n")
    print("FPS header PASS: native tab selection, panel moves, keyboard focus, and centered backing-strip text across 4 workspaces", flush=True)
