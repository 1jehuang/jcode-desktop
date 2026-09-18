"""Exercise the bottom workspace switcher on screenshot.py's private display."""
import json
import subprocess
import time


def verify(output, env, root):
    def nav():
        return json.loads(next(line.partition("=")[2] for line in (root / "state").read_text().splitlines()
                               if line.startswith("navigation=")))

    def native(*args):
        subprocess.run(["xdotool", *args], env=env, cwd=root, check=True, timeout=10)
        time.sleep(.4)

    # Distribute the four real fixture panels across the four workspaces.
    for slot in (1, 2, 3):
        x = dict(nav()["tab_targets"])[slot] + 276
        native("mousemove", str(round(x)), "34", "click", "1")
        assert nav()["focused_slot"] == slot, nav()
        for _ in range(slot):
            native("key", "super+shift+j")
        assert nav()["active_row"] == slot, nav()

    # Scroll the history far beyond the inline workspace headings.
    native("mousemove", "130", "400", "click", "--repeat", "70", "--delay", "15", "5")
    checks = []
    # Four 30px rows, 4px gaps and an 8px bottom inset, anchored to the window.
    for row in (0, 2, 1, 3):
        y = 1000 - 8 - 15 - (3 - row) * 34
        native("mousemove", "100", str(y), "click", "1")
        current = nav()
        assert current["active_row"] == row, current
        assert current["keyboard_panel"] == row, current
        checks.append({"row": row, "keyboard_panel": current["keyboard_panel"]})
    native("mousemove", "270", "400")
    output.with_suffix(".sidebar-workspaces.json").write_text(json.dumps({
        "passed": True, "history_scrolled": True, "native_workspace_clicks": checks,
    }, indent=2) + "\n")
    print("Sidebar workspaces PASS: all four bottom buttons navigate and focus after history scrolling", flush=True)
