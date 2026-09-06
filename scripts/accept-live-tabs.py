#!/usr/bin/env python3
"""Accept clustered folder tabs via native keys/clicks on private Xvfb/Openbox.

Uses the current binary and offline six-panel fixture, never builds or touches
the user's display. Saves navigation JSON, nonblank screenshots, and logs in a
new output directory. Requires the same headless tools as screenshot.py plus
xdotool and Pillow.
"""
import argparse
import json
import math
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import time

from PIL import Image, ImageStat
from screenshot import isolated_env


WIDTH, HEIGHT = 1440, 1000
CANVAS_LEFT, CANVAS_WIDTH, TAB_Y = 276, 1152, 34
SLOTS = set(range(6))


def navigation_state(path):
    """The state file is atomically replaced, and can be absent during startup."""
    try:
        line = next(line for line in path.read_text().splitlines()
                    if line.startswith("navigation="))
        return json.loads(line.partition("=")[2])
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


def panels(navigation):
    return [panel for row in navigation["rows"] for panel in row["panels"]]


def tab_targets(navigation):
    targets = navigation["tab_targets"]
    assert len(targets) == 6 and {slot for slot, _ in targets} == SLOTS, targets
    points = {slot: CANVAS_LEFT + x for slot, x in targets}
    assert all(math.isfinite(x) and CANVAS_LEFT <= x < CANVAS_LEFT + CANVAS_WIDTH
               for x in points.values()), points
    assert len({round(x) for x in points.values()}) == 6, points
    return points


def offscreen_slots(navigation):
    """Fixed fixture geometry from Workspace::width_for_fraction/render_strip.

    Folder canvas is 1152px with zero panel gap and 0.58px struts. Use only
    settled geometry, and classify fully clipped bodies, not just narrow edges.
    """
    row = next(row for row in navigation["rows"]
               if row["row"] == navigation["active_row"])
    left = -row["camera"]
    result = []
    for panel in row["panels"]:
        width = max(320, (CANVAS_WIDTH - 2 * 0.58) * panel["width"])
        if left + width <= 0 or left >= CANVAS_WIDTH:
            result.append(panel["slot"])
        left += width
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new evidence directory, never overwritten")
    parser.add_argument("--binary", type=Path, help="current binary (default: target/debug/jcode-desktop)")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    binary = (args.binary or repo / "target/debug/jcode-desktop").resolve()
    root = args.output.resolve()
    if not binary.is_file():
        parser.error(f"missing current binary: {binary}; build it before running")
    if root.exists():
        parser.error(f"refusing to overwrite {root}")
    for tool in ("Xvfb", "openbox", "xdotool", "import"):
        if not shutil.which(tool):
            parser.error("missing executable: " + tool)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe Vulkan driver")
    root.mkdir(parents=True, mode=0o700)
    env = isolated_env(root)
    env.update({
        "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
        "JCODE_DESKTOP_SCREENSHOT_PANELS": "6",
        "JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all",
        "VK_DRIVER_FILES": str(drivers[0]),
    })
    (root / "desktop.toml").write_text(
        '[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n'
        '[workspace]\ncoaching_hints = false\n'
    )
    wm_config = root / "openbox.xml"
    wm_config.write_text(
        '<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
        '<application class="*"><decor>no</decor><maximized>yes</maximized>'
        '</application></applications></openbox_config>'
    )
    for name in ("home", "runtime", "config", "cache", "data", "jcode", "logs"):
        (root / name).mkdir(mode=0o700, exist_ok=True)
    state_path = root / "state"
    processes, logs, checkpoints, clicks = [], [], [], []
    started = time.monotonic()
    summary = {"passed": False, "binary": str(binary),
               "binary_mtime_ns": binary.stat().st_mtime_ns, "checkpoints": checkpoints,
               "clicks": clicks}

    def launch(name, command, **kwargs):
        log = (root / f"{name}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(command, cwd=root, env=env, stdout=log, stderr=log,
                                   start_new_session=True, **kwargs)
        processes.append(process)

    def healthy():
        exited = [(p.args, p.poll()) for p in processes if p.poll() is not None]
        assert not exited, f"child exited: {exited}"

    def wait_until(predicate, label, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            healthy()
            nav = navigation_state(state_path)
            if nav is not None and predicate(nav):
                return nav
            time.sleep(0.05)
        raise AssertionError(f"timed out: {label}; navigation={navigation_state(state_path)}")

    def settled(slot=None):
        return wait_until(lambda nav: not nav["tab_motion"] and not nav["camera_motion"]
                          and (slot is None or (nav["focused_slot"] == slot
                                               and nav["keyboard_panel"] == slot)),
                          f"settled focus and keyboard panel {slot}")

    def native(*arguments):
        subprocess.run(["xdotool", *arguments], env=env, cwd=root, check=True, timeout=10)

    def key(name, slot):
        native("key", "--clearmodifiers", "Super_L+" + name)
        return settled(slot)

    def capture(label, slot, repeat=1):
        for frame in range(repeat):
            settled(slot)
            # Also allow width animation/font rasterization and presentation to finish.
            time.sleep(0.45)
            healthy()
            image_path = root / f"{label}-{frame + 1}.png"
            subprocess.run(["import", "-window", "root", "png:" + str(image_path)],
                           env=env, cwd=root, check=True, timeout=15)
            nav = settled(slot)
            points = tab_targets(nav)
            joined_pair = None
            if label == "middle-right":
                separation = points[3] - points[2]
                assert 0 < separation < 220, f"middle tabs are not a joined pair: {points}"
                assert all(points[i] < points[i + 1] for i in range(5)), points
                # Hidden neighbors belong beside the pair, not at the canvas
                # edges. Centers allow for their smaller labeled silhouettes.
                for neighbor, anchor in ((1, 2), (4, 3)):
                    distance = abs(points[neighbor] - points[anchor])
                    assert 100 < distance < 175, (neighbor, anchor, points)
                joined_pair = {"slots": [2, 3], "targets": [points[2], points[3]],
                               "separation_px": separation}
            assert len(panels(nav)) == 6 and not any(p["closing"] for p in panels(nav)), nav
            with Image.open(image_path) as source:
                image = source.convert("RGB")
            assert image.size == (WIDTH, HEIGHT), image.size
            # Validate both the tabs and transcript independently. A painted sidebar
            # must not let a blank/missing panel canvas pass acceptance.
            metrics = {}
            for name, box in (("tabs", (CANVAS_LEFT, 15, 1428, 52)),
                              ("body", (CANVAS_LEFT, 70, 1428, 900))):
                crop = image.crop(box)
                stats = ImageStat.Stat(crop)
                colors = crop.getcolors(crop.width * crop.height)
                metrics[name] = {"colors": len(colors),
                                 "stddev": max(stats.stddev),
                                 "nonmodal_fraction": 1 - max(n for n, _ in colors) /
                                 (crop.width * crop.height)}
                assert len(colors) >= 16 and max(stats.stddev) > 3, (label, name, metrics)
                assert metrics[name]["nonmodal_fraction"] > 0.01, (label, name, metrics)
            item = {"checkpoint": label, "frame": frame + 1,
                    "elapsed_seconds": round(time.monotonic() - started, 3),
                    "navigation": nav, "screen_tab_targets": points,
                    "joined_pair": joined_pair,
                    "offscreen_slots": offscreen_slots(nav), "pixels": metrics,
                    "screenshot": image_path.name}
            checkpoints.append(item)
            with (root / "live-tabs.jsonl").open("a") as trace:
                trace.write(json.dumps(item) + "\n")
            print(f"{label}/{frame + 1}: focus={slot}, six targets onscreen, pixels OK", flush=True)

    read_fd, write_fd = os.pipe()
    try:
        launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                        f"{WIDTH}x{HEIGHT}x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0], "Xvfb startup timeout"
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit(), display
        env["DISPLAY"] = ":" + display
        summary["private_display"] = env["DISPLAY"]
        launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm_config)])
        time.sleep(0.5)
        launch("desktop", [str(binary)])
        wait_until(lambda nav: len(panels(nav)) == 6, "six-panel fixture", timeout=45)
        assert "layout=FolderTabs sidebar=Sessions" in state_path.read_text()
        time.sleep(1.5)
        initial = settled()
        identities = [(p["slot"], p["id"], p["session"]) for p in panels(initial)]
        key("u", 0)
        for slot in range(6):
            if slot:
                key("l", slot)
            key("2", slot)
            wait_until(lambda nav: all(abs(p["width"] - 0.5) < 0.0001
                                      for p in panels(nav) if p["slot"] <= slot),
                       f"half-width panels through {slot}")
            capture(f"width-half-{slot}", slot)

        key("u", 0)
        key("l", 1)
        key("l", 2)
        capture("middle-left", 2, repeat=3)
        key("l", 3)
        capture("middle-right", 3, repeat=3)
        assert {0, 5}.issubset(offscreen_slots(settled(3))), "middle pair did not clip end panels"

        # Alternate ends to exercise collapsed tabs for clipped bodies. The final
        # 3 -> 4 click checks a barely visible panel at the right canvas edge.
        for step, slot in enumerate((0, 5, 1, 4, 2, 3, 4), 1):
            before = settled()
            x = round(tab_targets(before)[slot])
            item = {"slot": slot, "point": [x, TAB_Y], "before": before,
                    "was_offscreen": slot in offscreen_slots(before)}
            clicks.append(item)
            native("mousemove", str(x), str(TAB_Y), "click", "1")
            after = settled(slot)
            assert [(p["slot"], p["id"], p["session"]) for p in panels(after)] == identities
            assert all(abs(p["width"] - 0.5) < 0.0001 for p in panels(after))
            assert [p["slot"] for p in panels(after) if p["focused"]] == [slot]
            item["after"] = after
            capture(f"click-{step}-tab-{slot}", slot, repeat=2)
        assert {0, 5}.issubset({item["slot"] for item in clicks if item["was_offscreen"]})
        summary["passed"] = True
        print(f"PASS: six half-width tabs clickable, including offscreen panels. Evidence: {root}")
    except Exception as error:
        summary["error"] = str(error)
        summary["last_navigation"] = navigation_state(state_path)
        if "DISPLAY" in env:
            try:
                subprocess.run(["import", "-window", "root", "png:" + str(root / "failure.png")],
                               env=env, cwd=root, check=True, timeout=15)
                summary["failure_screenshot"] = "failure.png"
            except (OSError, subprocess.SubprocessError) as capture_error:
                summary["capture_error"] = str(capture_error)
        raise
    finally:
        (root / "result.json").write_text(json.dumps(summary, indent=2) + "\n")
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        for log in logs:
            log.close()


if __name__ == "__main__":
    main()
