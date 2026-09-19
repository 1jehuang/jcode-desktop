"""Exercise the bottom workspace switcher on screenshot.py's private display."""
import json
import subprocess
import time
from default_directory_acceptance import NativeUI
from model_picker_acceptance import normalized


def verify(output, env, root):
    ui = NativeUI(output, env, root)
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

    def select_workspace(row):
        # Four numbered buttons now share a single pinned row.
        native("mousemove", str(39 + row * 62), "978", "click", "1")
        native("mousemove", "270", "400")
        assert nav()["active_row"] == row, nav()

    def visible_group(image, row, label):
        words = ui.words(image, (0, 92, 264, 300), label)
        text = normalized(" ".join(word["text"] for word in words))
        assert f"reviewfolder{row + 1}" in text, text
        assert "activesessions" not in text, text
        assert not any(f"workspace{number}" in text for number in range(1, 5)), text
        for other in (1, 2, 3):
            if other != row:
                assert f"reviewfolder{other + 1}" not in text, text
        return words

    select_workspace(1)
    ui.wait_frame("workspace-2-expanded", lambda image: visible_group(image, 1, "workspace-2-expanded"))
    # The first collapsed marker sits in the upper-left of the first list row.
    native("mousemove", "20", "106", "click", "1")
    native("mousemove", "270", "400")
    assert nav()["active_row"] == 1, "disclosure must not navigate"
    def expanded(image):
        words = ui.words(image, (0, 92, 264, 300), "manual-expansion")
        assert sum(normalized(word["text"]) == "review" for word in words) == 2, words
    ui.wait_frame("manual-expansion", expanded)
    select_workspace(2)
    ui.wait_frame("workspace-3-expanded", lambda image: visible_group(image, 2, "workspace-3-expanded"))

    # Scroll the history far beyond the inline workspace headings.
    native("mousemove", "130", "400", "click", "--repeat", "70", "--delay", "15", "5")
    checks = []
    # The compact footer remains reachable after scrolling the history.
    for row in (0, 2, 1, 3):
        select_workspace(row)
        current = nav()
        assert current["active_row"] == row, current
        assert current["keyboard_panel"] == row, current
        checks.append({"row": row, "keyboard_panel": current["keyboard_panel"]})
    native("mousemove", "270", "400")
    output.with_suffix(".sidebar-workspaces.json").write_text(json.dumps({
        "passed": True, "history_scrolled": True, "native_workspace_clicks": checks,
        "numbered_groups": True, "focus_auto_collapses": True, "manual_expansion_preserves_focus": True,
    }, indent=2) + "\n")
    print("Sidebar workspaces PASS: all four bottom buttons navigate and focus after history scrolling", flush=True)
