"""Native tab navigation checks on screenshot.py's private Xvfb display."""
import json
import subprocess
import time


def verify(output, env, root):
    state_file = root / "state"

    def state():
        try:
            return json.loads(next(line.partition("=")[2]
                                   for line in state_file.read_text().splitlines()
                                   if line.startswith("navigation=")))
        except (FileNotFoundError, StopIteration, json.JSONDecodeError):
            return None

    def native(*args):
        subprocess.run(["xdotool", *args], env=env, cwd=root, check=True, timeout=10)

    def settled(slot=None):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            current = state()
            if (current and not current["camera_motion"] and not current["tab_motion"]
                    and (slot is None or current["focused_slot"] == slot)):
                return current
            time.sleep(.01)
        raise AssertionError(("navigation did not settle", slot, state()))

    def click(slot):
        current = settled()
        x = dict(current["tab_targets"])[slot] + 276
        native("mousemove", str(round(x)), "34", "mousedown", "1")
        native("mouseup", "1")
        return settled(slot)

    # Make a real 2D map with an overflowing lower row, through native keys.
    for slot, row in [(0, 0), (1, 1), (2, 2), (3, 2)]:
        click(slot)
        for _ in range(row):
            native("key", "super+shift+j")
            time.sleep(.2)
        native("key", "super+f")
        time.sleep(.2)
    click(0)
    results = []
    for slot in [3, 1, 0, 2, 3, 0]:
        before = settled()
        x = dict(before["tab_targets"])[slot] + 276
        native("mousemove", str(round(x)), "34", "mousedown", "1", "mouseup", "1")
        samples = []
        frame = None
        deadline = time.monotonic() + .35
        while time.monotonic() < deadline:
            current = state()
            if current and current["focused_slot"] == slot:
                if not samples or current["map_camera"] != samples[-1]["map_camera"]:
                    samples.append(current)
                if (current["row_motion"] and frame is None
                        and abs(current["map_camera"][1] - before["map_camera"][1]) > .4
                        and abs(current["map_camera"][1] - current["active_row"]) > .05):
                    path = output.with_name(f"{output.stem}-trip-{len(results)}.png")
                    frame = subprocess.Popen(["import", "-window", "root", "png:" + str(path)],
                                             env=env, cwd=root)
            time.sleep(.003)
        after = settled(slot)
        if frame is not None:
            assert frame.wait(timeout=10) == 0
        start, end = before["map_camera"], after["map_camera"]
        moving = [sample for sample in samples if sample["camera_motion"]]
        assert moving, ("tab click teleported without camera motion", slot, start, end)
        for axis in range(2):
            delta = end[axis] - start[axis]
            values = [start[axis]] + [s["map_camera"][axis] for s in samples] + [end[axis]]
            tolerance = .02 if axis else 1.0
            if abs(delta) <= tolerance:
                assert max(values) - min(values) <= tolerance, ("unexpected perpendicular motion", axis, values)
            else:
                direction = 1 if delta > 0 else -1
                assert all((b - a) * direction >= -tolerance for a, b in zip(values, values[1:])), (
                    "camera reversed or jumped away from its destination", axis, values)
        if abs(end[0] - start[0]) > 50 and abs(end[1] - start[1]) > .5:
            for sample in moving:
                point = sample["map_camera"]
                px = (point[0] - start[0]) / (end[0] - start[0])
                py = (point[1] - start[1]) / (end[1] - start[1])
                assert abs(px - py) < .03, ("axes did not travel together across the map", px, py)
        assert after["keyboard_panel"] == slot, "arrival lost composer focus"
        results.append({"slot": slot, "from": start, "to": end,
                        "samples": [{"camera": s["map_camera"], "moving": s["camera_motion"]}
                                    for s in samples]})
    output.with_suffix(".map-motion.json").write_text(json.dumps({"passed": True, "trips": results}, indent=2) + "\n")
    print(f"Map motion PASS: {len(results)} native tab trips, continuous aligned axes and correct focus", flush=True)
