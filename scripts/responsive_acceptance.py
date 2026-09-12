"""Native responsive-layout acceptance, only on screenshot.py's private display."""
import json
import subprocess
import time


def verify(output, env, root):
    state = root / "state"

    def nav():
        return json.loads(next(line.split("=", 1)[1] for line in state.read_text().splitlines()
                               if line.startswith("navigation=")))

    def native(*args):
        result = subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root,
                                check=True, capture_output=True, text=True, timeout=10)
        time.sleep(.25)
        return result.stdout.strip()

    def wait_for(predicate):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            current = nav()
            if predicate(current) and not current["camera_motion"] and not current["tab_motion"]:
                return current
            time.sleep(.05)
        raise AssertionError(nav())

    def capture(label):
        frame = output.with_name(f"{output.stem}-{label}.png")
        if frame.exists():
            raise FileExistsError(frame)
        subprocess.run(["import", "-window", window, "png:" + str(frame)],
                       env=env, cwd=root, check=True, timeout=15)
        return frame.name

    window = native("getactivewindow")
    native("windowmove", window, 0, 0)
    native("windowsize", window, 1440, 1000)
    original = wait_for(lambda n: n["viewport"] == [1440, 1000])
    original_ids = [panel["id"] for row in original["rows"] for panel in row["panels"]]
    results = []
    for width, height in [(960, 700), (800, 900), (640, 480), (1440, 1000)]:
        native("windowsize", window, width, height)
        current = wait_for(lambda n: n["viewport"] == [width, height])
        assert current["compact_sidebar"] == (width < 1100), current
        assert current["focused_slot"] == original["focused_slot"], current
        panels = [panel for row in current["rows"] for panel in row["panels"]]
        assert [panel["id"] for panel in panels] == original_ids
        assert all(panel["width"] == .5 for panel in panels), panels
        if width < 1100:
            assert current["canvas_width"] >= width - 72, current
            before = current["canvas_width"]
            native("mousemove", 24, 48, "click", 1)
            opened = wait_for(lambda n: n["sidebar_overlay"])
            assert opened["canvas_width"] == before, opened
            if width == 800:
                results.append({"drawer": capture("drawer")})
            native("key", "Escape")
            closed = wait_for(lambda n: not n["sidebar_overlay"])
            assert closed["keyboard_panel"] == original["focused_slot"], closed
            # Tabs remain a pointer-accessible way to reach every conversation.
            for slot in [0, 1]:
                x = dict(nav()["tab_targets"])[slot] + 60
                native("mousemove", round(x), 45, "click", 1)
                focused = wait_for(lambda n: n["focused_slot"] == slot and n["keyboard_panel"] == slot)
                assert focused["compact_sidebar"]
        results.append({"size": [width, height], "screenshot": capture(f"{width}x{height}"),
                        "compact": current["compact_sidebar"], "canvas_width": current["canvas_width"]})
    report = {"passed": True, "stable_panel_ids": original_ids, "frames": results,
              "native_sidebar_open_close_cycles": 3, "native_tab_selections": 6,
              "native_resizes": 4, "width_presets_preserved": True}
    output.with_suffix(".responsive.json").write_text(json.dumps(report, indent=2) + "\n")
    print("Responsive layout PASS: four native resizes, three drawers, six tab selections, preserved panel widths")
