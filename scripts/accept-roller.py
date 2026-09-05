#!/usr/bin/env python3
"""Accept the folder-mode sidebar roller through native input on private Xvfb.

Uses the existing offline screenshot fixture and current desktop binary. It never
builds or sends input to the user's display. The output directory contains a
JSONL trace, checkpoint screenshots, OCR crops, and process logs.
"""
import argparse
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import subprocess
import time

from PIL import Image, ImageEnhance, ImageOps
from screenshot import isolated_env


WIDTH, HEIGHT = 1440, 1000
CENTER = (132, 35)
HEADER_BLANK = (130, 3)
PREVIOUS = (117, 9)
NEXT = (147, 9)


def sidebar_state(path):
    """Read the existing human-readable sidebar field despite atomic rewrites."""
    try:
        match = re.search(r"^layout=(\w+) sidebar=(\w+)$", path.read_text(), re.MULTILINE)
        return match.groups() if match else None
    except FileNotFoundError:
        return None


def navigation_state(path):
    """Read structured panel identity and focus from the existing state file."""
    try:
        line = next(line for line in path.read_text().splitlines()
                    if line.startswith("navigation="))
        return json.loads(line.partition("=")[2])
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


def panel_signature(navigation):
    """Observable panel catalog plus map/keyboard focus for action-safety checks."""
    assert navigation is not None, "missing structured navigation state"
    panels = [(panel["id"], panel["session"], panel["slot"])
              for row in navigation["rows"] for panel in row["panels"]]
    focused = [panel["session"] for row in navigation["rows"]
               for panel in row["panels"] if panel["focused"]]
    return panels, focused, navigation["focused_slot"], navigation["keyboard_panel"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new directory for screenshots, trace, and logs")
    parser.add_argument("--binary", type=Path, help="current binary; defaults to target/debug/jcode-desktop")
    args = parser.parse_args()

    repo = Path(__file__).resolve().parents[1]
    binary = (args.binary or repo / "target/debug/jcode-desktop").resolve()
    root = args.output.resolve()
    if not binary.is_file():
        parser.error(f"missing current binary: {binary}; build it before running this script")
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool):
            parser.error("missing executable: " + tool)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe Vulkan driver")
    root.mkdir(parents=True, exist_ok=False, mode=0o700)

    env = isolated_env(root)
    env.update({
        "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
        "JCODE_DESKTOP_SCREENSHOT_PANELS": "1",
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
    processes = []
    logs = []
    started = time.monotonic()

    def launch(name, command, **kwargs):
        log = (root / f"{name}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(command, cwd=root, env=env, stdout=log, stderr=log,
                                   start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def healthy(label):
        exited = [(p.pid, p.poll(), p.args) for p in processes if p.poll() is not None]
        assert not exited, f"child exited during {label}: {exited}"

    def wait_until(predicate, label, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            healthy(label)
            if predicate():
                return
            time.sleep(0.05)
        raise AssertionError(f"timed out waiting for {label}\n" +
                             (state_path.read_text() if state_path.exists() else "no state"))

    def visible_windows():
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", ".*"],
                                env=env, text=True, capture_output=True, timeout=10)
        return sorted(line for line in result.stdout.splitlines() if line)

    def capture(label, expected_center=None):
        healthy(label)
        time.sleep(0.45)
        image_path = root / f"{label}.png"
        subprocess.run(["import", "-window", "root", "png:" + str(image_path)], env=env,
                       check=True, timeout=15)
        image = Image.open(image_path).convert("RGB")
        assert image.size == (WIDTH, HEIGHT), image.size
        # Crop only the centered folder, excluding the arrow controls and side folders.
        crop = image.crop((91, 18, 174, 51))
        crop = ImageOps.grayscale(crop)
        crop = ImageEnhance.Contrast(crop).enhance(3.0)
        crop = crop.resize((crop.width * 8, crop.height * 8))
        crop_path = root / f"{label}-center.png"
        crop.save(crop_path)
        ocr = subprocess.check_output(
            ["tesseract", str(crop_path), "stdout", "--psm", "7"], env=env,
            text=True, stderr=subprocess.DEVNULL, timeout=20,
        ).strip().lower()
        layout_sidebar = sidebar_state(state_path)
        navigation = navigation_state(state_path)
        item = {
            "checkpoint": label,
            "elapsed_seconds": round(time.monotonic() - started, 3),
            "center_ocr": ocr,
            "expected_center": expected_center,
            "layout_sidebar": layout_sidebar,
            "navigation": navigation,
            "visible_windows": visible_windows(),
            "screenshot": image_path.name,
        }
        with (root / "roller.jsonl").open("a") as trace:
            trace.write(json.dumps(item) + "\n")
        print(f"{label}: center={ocr!r} state={layout_sidebar}", flush=True)
        return item

    def point_and_click(point, button=1, repeat=1):
        command = ["xdotool", "mousemove", str(point[0]), str(point[1]), "click"]
        if repeat != 1:
            command += ["--repeat", str(repeat), "--delay", "80"]
        command.append(str(button))
        subprocess.run(command, env=env, check=True, timeout=10)
        time.sleep(0.55)

    def assert_page(expected, label):
        wait_until(lambda: sidebar_state(state_path) == ("FolderTabs", expected), label, timeout=5)

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
        launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm_config)])
        time.sleep(0.5)
        launch("desktop", [str(binary)])
        wait_until(lambda: sidebar_state(state_path) == ("FolderTabs", "Sessions"),
                   "initial folder-mode sidebar", timeout=45)
        time.sleep(1.5)

        initial = capture("01-initial-sessions", "chat")
        baseline_windows = initial["visible_windows"]
        baseline_panels = panel_signature(initial["navigation"])

        def assert_browse_safe(item, label):
            assert item["visible_windows"] == baseline_windows, \
                label + ": browsing opened or closed a visible window"
            assert panel_signature(item["navigation"]) == baseline_panels, \
                label + ": browsing created, replaced, or refocused a panel"

        # Both explicit arrows browse. Previous reaches the wrapped action folder;
        # next returns to Sessions without invoking that action.
        point_and_click(PREVIOUS)
        previous = capture("01b-previous-browsed")
        assert sidebar_state(state_path) == ("FolderTabs", "Sessions"), \
            "previous-arrow browsing activated an action"
        assert_browse_safe(previous, "previous arrow")
        assert (root / "01b-previous-browsed-center.png").read_bytes() != \
            (root / "01-initial-sessions-center.png").read_bytes(), \
            "previous arrow did not visibly change the centered folder"
        point_and_click(NEXT)
        returned = capture("01c-next-returned-sessions", "chat")
        assert sidebar_state(state_path) == ("FolderTabs", "Sessions")
        assert_browse_safe(returned, "next arrow")

        def wheel_to_learn(point, label):
            # One discrete X11 wheel event is one native roller step. Browsing must
            # visibly move to Learn without activating it or opening UI.
            point_and_click(point, button=5)
            wheel = capture(label, "learn")
            assert sidebar_state(state_path) == ("FolderTabs", "Sessions"), \
                label + ": wheel browsing changed the active sidebar page"
            assert_browse_safe(wheel, label)
            point_and_click(CENTER)
            assert_page("Learn", label + ": centered folder after wheel was not Learn")

        # Diagnose event delivery through a blank part of the parent and through
        # the centered tab itself. Both are part of the 52px header hit target.
        wheel_to_learn(HEADER_BLANK, "02-wheel-blank-browsed-learn")
        point_and_click(PREVIOUS)
        point_and_click(CENTER)
        assert_page("Sessions", "reset after blank-header wheel")
        wheel_to_learn(CENTER, "03-wheel-tab-browsed-learn")

        # Clicking the wheel-centered Learn folder above activated it. Arrows browse
        # all subsequent pages without activation until their center is clicked.
        point_and_click(CENTER)
        assert_page("Learn", "centered Learn remains active")
        capture("03b-clicked-learn", "learn")

        for number, (center, page) in enumerate(
                (("files", "Files"), ("accounts", "Accounts"), ("theme", "Theme")), 4):
            point_and_click(NEXT)
            browsed = capture(f"{number:02d}-arrow-browsed-{center}", center)
            assert sidebar_state(state_path) == ("FolderTabs", {
                "files": "Learn", "accounts": "Files", "theme": "Accounts"
            }[center]), "arrow browsing activated a page"
            assert_browse_safe(browsed, center + " arrow")
            point_and_click(CENTER)
            assert_page(page, f"centered {page} activation")
            capture(f"{number:02d}b-clicked-{center}", center)

        # Theme is index 4. Seven next clicks traverse action folders without invoking
        # them and wrap index 11 back to Sessions.
        for _ in range(7):
            point_and_click(NEXT)
            assert sidebar_state(state_path) == ("FolderTabs", "Theme"), \
                "arrow browsing across action folders activated one"
        wrapped = capture("07-wrapped-to-sessions", "chat")
        assert_browse_safe(wrapped, "wrapping across action folders")
        point_and_click(CENTER)
        assert_page("Sessions", "wrapped Sessions activation")
        final = capture("08-final-sessions", "chat")
        assert_browse_safe(final, "final Sessions activation")

        print("PASS: wheel and arrows browse without activation; centered Learn, Files, "
              "Accounts, and Theme clicks activate; next wraps to Sessions. "
              f"Evidence: {root}", flush=True)
    finally:
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
