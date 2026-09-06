"""Real rendered-pixel and native-input checks for workspace identity.

Called only from screenshot.py's private Xvfb fixture. Saves each selected
workspace frame and a measured JSON report next to the requested screenshot.
"""
from collections import Counter
import itertools
import json
import math
import subprocess
import time

from PIL import Image


def distance(a, b):
    return math.sqrt(sum((x - y) ** 2 for x, y in zip(a, b)))


def blend(background, foreground, alpha):
    return tuple(round(b * (1 - alpha) + f * alpha) for b, f in zip(background, foreground))


def measure(image, targets, theme):
    """Recognize workspace rails and selection from pixels, not navigation state."""
    accents = ([(138, 180, 248), (128, 203, 196), (232, 184, 109), (196, 161, 237)]
               if theme == "warm-neutral" else
               [(36, 91, 156), (25, 104, 94), (128, 82, 16), (117, 64, 162)])
    # Background is sampled from the empty canvas, away from panels and labels.
    background = image.getpixel((800, 200))
    expected = [(accent, blend(blend(background, accent, .05), accent, .65))
                for accent in accents]
    rails, identified = [], []
    for slot in range(4):
        x = round(targets[slot] + 276)
        counts = Counter(image.crop((x - 25, 16, x + 25, 25)).getdata())
        rail = max((color for color, count in counts.items() if count >= 40),
                   key=lambda color: distance(color, background))
        errors = [min(distance(rail, candidate) for candidate in variants) for variants in expected]
        assert min(errors) <= 4, ("rail has no recognizable workspace identity", slot, rail, errors)
        identified.append(errors.index(min(errors)))
        rails.append(rail)
    assert identified == list(range(4)), ("workspaces are not independently identifiable", identified)
    separation = min(distance(a, b) for a, b in itertools.combinations(rails, 2))
    assert separation >= 30, ("workspace accents are too similar", separation)
    # A filled badge identifies selection even with the text removed. Inactive
    # numbers have only a few accent-colored glyph pixels, not a solid fill.
    fills = []
    for row, accent in enumerate(accents):
        y = round(23 + row * 22.25)
        pixels = list(image.crop((1288, y - 7, 1306, y + 7)).getdata())
        fills.append(sum(distance(pixel, accent) <= 4 for pixel in pixels) / len(pixels))
    selected = [row for row, fill in enumerate(fills) if fill >= .65]
    assert len(selected) == 1, ("selected workspace is visually ambiguous", fills)
    assert all(fill < .15 for row, fill in enumerate(fills) if row != selected[0]), fills
    return {"identified_workspaces": identified, "rail_rgb": rails,
            "minimum_pairwise_rgb_distance": separation,
            "badge_accent_fill_fraction": fills, "visually_selected_workspace": selected[0]}


def verify(output, env, root, *, theme):
    state = root / "state"
    evidence = [output.with_name(f"{output.stem}-selected-{row + 1}.png") for row in range(4)]
    evidence.append(output.with_suffix(".workspace-identity.json"))
    if any(path.exists() for path in evidence):
        raise FileExistsError("workspace evidence already exists; choose a new output name")

    def nav():
        return json.loads(next(line.partition("=")[2] for line in state.read_text().splitlines()
                               if line.startswith("navigation=")))

    def native(*args):
        subprocess.run(["xdotool", *args], env=env, cwd=root, check=True, timeout=10)
        time.sleep(.35)

    for slot in [1, 2, 3]:
        x = dict(nav()["tab_targets"])[slot] + 276
        native("mousemove", str(round(x)), "34", "click", "1")
        assert nav()["focused_slot"] == slot
        for _ in range(slot):
            native("key", "super+shift+j")
        assert nav()["active_row"] == slot
    assert all(len(row["panels"]) == 1 for row in nav()["rows"])
    for _ in range(5):
        native("mousemove", "146", "10", "click", "1")
    native("mousemove", "132", "35", "click", "1")
    assert "sidebar=Settings" in state.read_text()
    native("mousemove", "100", "180", "click", "1")
    assert nav()["minimap_visible"]
    for _ in range(5):
        native("mousemove", "116", "10", "click", "1")
    native("mousemove", "132", "35", "click", "1")
    assert "sidebar=Sessions" in state.read_text()
    results = []
    for row in [0, 1, 2, 3]:
        native("mousemove", "1302", str(round(23 + row * 22.25)), "click", "1")
        native("mousemove", "270", "400")
        current = nav()
        assert current["active_row"] == row and current["keyboard_panel"] == row, current
        frame = output.with_name(f"{output.stem}-selected-{row + 1}.png")
        subprocess.run(["import", "-window", "root", "png:" + str(frame)],
                       env=env, cwd=root, check=True, timeout=15)
        with Image.open(frame) as image:
            metrics = measure(image.convert("RGB"), dict(current["tab_targets"]), theme)
        assert metrics["visually_selected_workspace"] == row, metrics
        metrics.update(requested_workspace=row, native_active_row=current["active_row"],
                       keyboard_panel=current["keyboard_panel"], screenshot=frame.name)
        results.append(metrics)
    report = {"passed": True, "theme": theme, "frames": results,
              "recognized_identities": sum(len(frame["identified_workspaces"]) for frame in results),
              "correct_native_and_visual_selections": len(results)}
    output.with_suffix(".workspace-identity.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Workspace identity PASS: {report['recognized_identities']}/16 rendered identities, "
          f"{len(results)}/4 native/visual selections, minimum RGB separation "
          f"{min(frame['minimum_pairwise_rgb_distance'] for frame in results):.2f}", flush=True)
