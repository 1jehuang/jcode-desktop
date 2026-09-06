#!/usr/bin/env python3
"""Accept focused folder-panel layers through real pixels on private Xvfb.

Uses screenshot.isolated_env and an existing binary, never builds or reloads.
Runs a 1440x1000 empty offline three-panel fixture for both supported test
palettes, clicking every panel through native X11 input. The output directory
must be new. PNGs, public focus state, logs, and results.json survive failures.

Example: python3 scripts/accept-panel-layers.py target/accept-panel-layers
         --binary target/focus-layers-baseline-binary
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import tempfile
import time

from PIL import Image
from screenshot import isolated_env


# workspace.rs: sidebar 264 + connector 12, right margin 12, STRUT .58,
# GAP 0, STRIP_PADDING_Y 16, FOLDER_CONTENT_INSET 32, CORNER_RADIUS 6.
# These are acceptance coordinates, not inferred from the pixels under test.
WIDTH, HEIGHT = 1440, 1000
LEFT, CANVAS_WIDTH = 276, 1152
PANEL_WIDTH = (CANVAS_WIDTH - 2 * 0.58) / 3
TOP, BOTTOM, RADIUS = 48, 984, 6
PALETTES = {
    "warm-neutral": {"active": (37, 34, 31), "inactive": (48, 43, 39)},
    "neutral-light": {"active": (248, 247, 244), "inactive": (226, 223, 218)},
}


def near(actual, expected):
    return max(abs(a - b) for a, b in zip(actual, expected)) <= 2


def bounds(panel):
    left = LEFT + PANEL_WIDTH * panel
    return left, left + PANEL_WIDTH


def read_state(root):
    try:
        return (root / "state").read_text()
    except FileNotFoundError:
        return ""


def focused_state(text, focus):
    if not re.search(rf"\bfocus={focus}\s", text):
        return False
    if "widths=0.33,0.33,0.33" not in text:
        return False
    lines = [line for line in text.splitlines() if line.startswith("navigation=")]
    try:
        navigation = json.loads(lines[0].split("=", 1)[1]) if lines else {}
    except json.JSONDecodeError:
        return False
    panels = [panel for row in navigation.get("rows", []) for panel in row["panels"]]
    selected = [panel["slot"] for panel in panels if panel["focused"]]
    return selected == [focus] and navigation.get("keyboard_panel") == focus


def pixel_checks(image, focus, palette):
    """Check both sides of every bottom corner, especially internal seams.

    Inactive cutouts must disappear into the backing. Active cutouts must
    expose that same inactive backing, while the adjacent bottom edge and
    panel body retain active fill. Samples avoid tabs, text and antialiasing.
    The old PANEL_BG backing inverts these internal corner relationships.
    """
    checks = []

    def sample(label, point, expected, panel=None):
        actual = image.getpixel(point)
        checks.append(dict(label=label, panel=panel, point=point,
                           expected=expected, actual=actual,
                           passed=near(actual, expected)))

    for panel in range(3):
        left, right = bounds(panel)
        fill = palette["active" if panel == focus else "inactive"]
        for y in (300, 700):
            sample("body fill", (math.ceil(left) + 20, y), fill, panel)
        for side, cut_x, fill_x in (
            ("left", math.ceil(left) + 1, math.ceil(left) + RADIUS + 4),
            ("right", math.floor(right) - 2, math.floor(right) - RADIUS - 4),
        ):
            internal = (side == "left" and panel > 0) or (side == "right" and panel < 2)
            role = "active" if panel == focus else "inactive"
            sample(f"{role} {side} {'internal' if internal else 'outer'} rounded cutout",
                   (cut_x, BOTTOM - 1), palette["inactive"], panel)
            sample(f"{side} bottom fill", (fill_x, BOTTOM - 1), fill, panel)
        sample("below panel backing", (round((left + right) / 2), BOTTOM + 8),
               palette["inactive"], panel)
    # The selected sidebar tab must remain raised, not regress to HEADER_BG.
    # Locate a long solid run rather than tying this test to sidebar row labels.
    run = longest = 0
    for y in range(60, 450):
        run = run + 1 if near(image.getpixel((250, y)), palette["active"]) else 0
        longest = max(longest, run)
    checks.append(dict(label="sidebar selected tab retains active fill", x=250,
                       longest_active_run=longest, expected_minimum=20,
                       passed=longest >= 20))
    return checks


def run_theme(binary, theme, output, scratch, driver, timeout):
    result = dict(theme=theme, cases=[], passed=False)
    artifact = output / theme
    artifact.mkdir()
    with tempfile.TemporaryDirectory(prefix="panel-layers-", dir=scratch) as temporary:
        root = Path(temporary)
        env = isolated_env(root)
        env.update({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "empty",
                    "JCODE_DESKTOP_SCREENSHOT_PANELS": "3",
                    "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
                    "JCODE_NO_TELEMETRY": "1", "VK_DRIVER_FILES": str(driver)})
        for name in ("home", "runtime", "config", "cache", "data", "jcode"):
            (root / name).mkdir(mode=0o700)
        (root / "desktop.toml").write_text(
            f'[appearance]\nlayout_mode = "folder_tabs"\ntheme = "{theme}"\n')
        wm_config = root / "openbox.xml"
        wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc">'
                             '<applications><application class="*"><decor>no</decor>'
                             '<maximized>yes</maximized></application></applications></openbox_config>')
        processes, logs = [], []
        read_fd, write_fd = os.pipe()

        def launch(name, command, **kwargs):
            log = (artifact / f"{name}.log").open("w")
            logs.append(log)
            process = subprocess.Popen(command, cwd=root, env=env, stdout=log,
                                       stderr=log, **kwargs)
            processes.append(process)
            return process

        def alive():
            if any(process.poll() is not None for process in processes):
                raise RuntimeError("Private display, window manager, or app exited. See saved logs.")

        def wait_state(focus):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                alive()
                if focused_state(read_state(root), focus):
                    return
                time.sleep(0.1)
            raise RuntimeError(f"Timed out waiting for native focus={focus}: {read_state(root)}")

        def rendered_frame(path, focus):
            # State can precede presentation, including an entirely black root
            # capture. Poll actual palette pixels, then require two stable
            # bottom bands. Do NOT wait for corner checks to pass: that would
            # turn the precise pre-fix regression into a startup timeout.
            deadline = time.monotonic() + timeout
            previous = None
            polls = 0
            while time.monotonic() < deadline:
                alive()
                subprocess.run(["import", "-window", "root", "png:" + str(path)],
                               cwd=root, env=env, check=True, timeout=15)
                with Image.open(path) as source:
                    image = source.convert("RGB")
                polls += 1
                ready = image.size == (WIDTH, HEIGHT) and focused_state(read_state(root), focus)
                if ready:
                    for panel in range(3):
                        fill = PALETTES[theme]["active" if panel == focus else "inactive"]
                        x = math.ceil(bounds(panel)[0]) + 20
                        ready = ready and all(near(image.getpixel((x + dx, y)), fill)
                                              for dx in range(3) for y in (300, 700))
                band = image.crop((LEFT, BOTTOM - 12, WIDTH - 12, BOTTOM + 8)).tobytes()
                if ready and band == previous:
                    return image, polls
                previous = band if ready else None
                time.sleep(0.2)
            raise RuntimeError(f"No stable rendered palette pixels for focus={focus}; last capture: {path}")

        try:
            launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                            f"{WIDTH}x{HEIGHT}x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
            os.close(write_fd)
            write_fd = None
            if not select.select([read_fd], [], [], 15)[0]:
                raise RuntimeError("Xvfb startup timeout")
            display = os.read(read_fd, 64).decode().strip()
            if not display.isdigit():
                raise RuntimeError("Xvfb did not allocate a private display")
            env["DISPLAY"] = ":" + display
            launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm_config)])
            time.sleep(0.5)
            launch("app", [str(binary)])
            wait_state(1)  # The fixture initially focuses the middle panel.
            rendered_frame(artifact / "initial.png", 1)
            for focus in range(3):
                x = round(sum(bounds(focus)) / 2)
                subprocess.run(["xdotool", "mousemove", str(x), "500", "click", "1"],
                               cwd=root, env=env, check=True, timeout=10)
                wait_state(focus)
                path = artifact / f"focus-{focus}.png"
                image, polls = rendered_frame(path, focus)
                text = read_state(root)
                (artifact / f"focus-{focus}.state").write_text(text)
                checks = pixel_checks(image, focus, PALETTES[theme])
                case = dict(focus=focus, native_click=[x, 500], state=text,
                            state_focus_passed=focused_state(text, focus),
                            screenshot=str(path), capture_polls=polls, checks=checks,
                            passed=all(check["passed"] for check in checks))
                case["passed"] = case["passed"] and case["state_focus_passed"]
                result["cases"].append(case)
                failures = [check["label"] for check in checks if not check["passed"]]
                print(f"{'PASS' if case['passed'] else 'FAIL'} {theme} focus={focus}: "
                      f"{', '.join(failures) if failures else 'focus and all layer pixels'}", flush=True)
            result["passed"] = all(case["passed"] for case in result["cases"])
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
            print(f"ERROR {theme}: {error}", flush=True)
        finally:
            (artifact / "last.state").write_text(read_state(root))
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
            for log in logs:
                log.close()
            diagnostics = root / "logs"
            if diagnostics.exists():
                shutil.copytree(diagnostics, artifact / "diagnostics")
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
    (artifact / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new artifact directory")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--timeout", type=float, default=60, help="state/presentation timeout seconds")
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be finite and positive")
    for tool in ("Xvfb", "openbox", "xdotool", "import"):
        if not shutil.which(tool):
            parser.error(f"missing required tool: {tool}")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe Vulkan driver")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        parser.error(f"refusing to overwrite artifacts: {output}")
    output.mkdir(parents=True, mode=0o700)
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", repo / "target"))
    scratch.mkdir(parents=True, exist_ok=True)
    report = dict(binary=str(binary), size=[WIDTH, HEIGHT], transcript="empty",
                  layout_mode="folder_tabs", panel_count=3,
                  geometry=dict(left=LEFT, canvas_width=CANVAS_WIDTH,
                                panel_width=PANEL_WIDTH, top=TOP, bottom=BOTTOM, radius=RADIUS),
                  themes=[], passed=False)
    # A build may atomically replace the source binary during this run. Freeze
    # one executable for both palettes and record its content hash as evidence.
    with tempfile.TemporaryDirectory(prefix="panel-layers-binary-", dir=scratch) as temporary:
        frozen = Path(temporary) / "jcode-desktop"
        shutil.copy2(binary, frozen)
        with frozen.open("rb") as executable:
            report["binary_sha256"] = hashlib.file_digest(executable, "sha256").hexdigest()
        for theme in PALETTES:
            report["themes"].append(run_theme(frozen, theme, output, scratch, drivers[0], args.timeout))
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    report["passed"] = all(theme["passed"] for theme in report["themes"])
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"{'PASS' if report['passed'] else 'FAIL'}: {output / 'results.json'}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
