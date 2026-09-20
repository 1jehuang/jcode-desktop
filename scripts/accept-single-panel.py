#!/usr/bin/env python3
"""Accept independent single-panel windows on private Xvfb using an existing binary.

Never builds, reloads, connects to a daemon, or sends input to the live desktop.
The new output directory retains screenshots, state snapshots, logs and results.
"""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import select
import shutil
import socket
import stat
import subprocess
import tempfile
import time

from screenshot import isolated_env


WIDTH, HEIGHT = 1440, 1000
# Native aliases cover creation, sidebar, overview, strip/focus/move and widths.
# Ctrl+R is deliberately absent: the acceptance run must never build/reload.
WORKSPACE_KEYS = (
    "ctrl+b", "super+b", "ctrl+shift+e", "super+o", "super+shift+Tab",
    "super+n", "super+Return", "ctrl+alt+Return", "ctrl+shift+n", "ctrl+t",
    "super+semicolon", "super+apostrophe", "super+space", "super+t",
    "super+h", "super+l", "super+j", "super+k",
    "super+Left", "super+Right", "super+Up", "super+Down",
    "super+Home", "super+End", "super+u", "super+p", "super+Tab",
    "ctrl+Tab", "ctrl+shift+Tab", "ctrl+Page_Up", "ctrl+Page_Down",
    "ctrl+alt+Left", "ctrl+alt+Right", "ctrl+alt+Up", "ctrl+alt+Down",
    "super+shift+h", "super+shift+l", "super+shift+j", "super+shift+k",
    "super+shift+Home", "super+shift+End", "super+r", "super+f",
    "super+1", "super+2", "super+3", "super+4",
)


def navigation(path):
    """Retryable reader: the renderer can be between truncate and write."""
    try:
        line = next(line for line in path.read_text().splitlines()
                    if line.startswith("navigation="))
        return json.loads(line.partition("=")[2])
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


def invariant(state):
    """Stable workspace identity, excluding animation time and keyboard focus."""
    fields = ("single_panel", "visible_panels", "sidebar_visible", "active_row",
              "focused_slot", "overview", "minimap_visible", "sidebar_overlay",
              "viewport", "canvas_width", "tab_targets")
    value = {key: state.get(key) for key in fields}
    value["rows"] = [
        {"row": row["row"], "remembered": row["remembered"], "panels": [
            {key: panel.get(key) for key in ("slot", "id", "session", "width", "closing")}
            for panel in row["panels"]]}
        for row in state["rows"]]
    return value


def assert_single(state):
    assert state["single_panel"] is True, state
    assert state["visible_panels"] == 1, state
    assert state["sidebar_visible"] is False, state
    assert state["overview"] is False, state
    assert state["minimap_visible"] is False, state
    assert state["sidebar_overlay"] is False, state
    assert not state["tab_targets"], state
    assert state["viewport"] == [WIDTH, HEIGHT], state
    assert abs(state["canvas_width"] - WIDTH) <= 1, state
    panels = [panel for row in state["rows"] for panel in row["panels"]]
    assert len(panels) == 1, state
    assert panels[0]["width"] == 1, state
    assert panels[0]["closing"] is False, state
    assert state["focused_slot"] == panels[0]["slot"], state


def wait_for(predicate, label, processes=(), timeout=30):
    deadline = time.monotonic() + timeout
    while True:
        dead = [(process.pid, process.returncode) for process in processes
                if process.poll() is not None]
        if dead:
            raise AssertionError(f"Process exited while waiting for {label}: {dead}")
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise AssertionError("Timed out waiting for " + label)
        time.sleep(.1)


def socket_ready(path):
    try:
        if not stat.S_ISSOCK(path.stat().st_mode):
            return False
        # Connect then close without sending a command. Do not Show/Reload a host.
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(.5)
            client.connect(str(path))
        return True
    except OSError:
        return False


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new directory for retained evidence")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop",
                        help="already-built Desktop host with current linked UI")
    args = parser.parse_args()
    for tool in ("Xvfb", "openbox", "xdotool", "import"):
        if not shutil.which(tool):
            parser.error("missing executable: " + tool)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe is required")
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must be an existing executable. This script never builds.")
    output = args.output.resolve()
    if output.exists():
        parser.error("refusing to overwrite " + str(output))
    output.mkdir(parents=True)
    scratch = repo / "target"
    scratch.mkdir(exist_ok=True)
    report = {"scope": "offline linked UI on private Xvfb + Openbox", "binary": str(binary),
              "passed": False, "checks": [], "instances": {}}
    with tempfile.TemporaryDirectory(prefix="sp-", dir=scratch) as temporary, ExitStack() as stack:
        root = Path(temporary)
        env = isolated_env(root)
        env.update(VK_DRIVER_FILES=str(drivers[0]), JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT="empty",
                   JCODE_DESKTOP_SCREENSHOT_PANELS="1",
                   JCODE_DESKTOP_CONFIG=str(root / "desktop.toml"))
        for name in ("home", "runtime", "config", "cache", "data", "logs", "jcode"):
            (root / name).mkdir(mode=0o700)
        if len(str(root / "runtime/jcode-desktop-single-panel-99999999.sock").encode()) >= 104:
            parser.error("checkout path is too long for private Unix sockets")
        (root / "desktop.toml").write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n')
        (root / "openbox.xml").write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<keyboard/><applications><application class="*"><decor>no</decor>
<maximized>yes</maximized></application></applications></openbox_config>''')
        processes, apps = [], {}
        read_fd, write_fd = os.pipe()

        def spawn(name, command, child_env):
            log = stack.enter_context((output / (name + ".log")).open("w"))
            process = subprocess.Popen(command, env=child_env, cwd=root, stdout=log, stderr=log)
            processes.append(process)
            return process

        def native(*args, check=True):
            return subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root,
                                  text=True, capture_output=True, check=check, timeout=10)

        def windows(pid):
            result = native("search", "--onlyvisible", "--pid", pid, check=False)
            return result.stdout.split() if result.returncode == 0 else []

        def get_state(name):
            return wait_for(lambda: navigation(root / (name + ".state")), name + " state",
                            [app["process"] for app in apps.values()])

        def focus(name):
            native("windowactivate", "--sync", apps[name]["window"])
            assert native("getactivewindow").stdout.strip() == apps[name]["window"]

        def unchanged(baselines):
            for name, baseline in baselines.items():
                assert apps[name]["process"].poll() is None, name + " died"
                assert windows(apps[name]["process"].pid) == [apps[name]["window"]], name
                assert invariant(get_state(name)) == baseline, name + " workspace changed"
                path = apps[name]["socket"]
                assert path.stat().st_ino == apps[name]["inode"], name + " socket replaced"
                assert socket_ready(path), name + " socket unavailable"

        try:
            log = stack.enter_context((output / "xvfb.log").open("w"))
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                                     f"{WIDTH}x{HEIGHT}x24", "-nolisten", "tcp"],
                                    pass_fds=(write_fd,), env=env, cwd=root, stdout=log, stderr=log)
            processes.append(xvfb)
            os.close(write_fd)
            write_fd = None
            assert select.select([read_fd], [], [], 15)[0], "Xvfb did not become ready"
            display = os.read(read_fd, 64).decode().strip()
            assert display.isdigit(), "Xvfb failed to allocate private display"
            env["DISPLAY"] = ":" + display
            wm = spawn("openbox", ["openbox", "--sm-disable", "--config-file", str(root / "openbox.xml")], env)
            time.sleep(.5)
            assert wm.poll() is None, "Private Openbox exited"
            protected = {}
            for name, mode in (("normal", []), ("no-sidebar", ["--no-sidebar"]),
                               ("single-a", ["--single-panel"]), ("single-b", ["--single-panel"])):
                child_env = dict(env, JCODE_DESKTOP_STATE=str(root / (name + ".state")))
                process = spawn(name, [str(binary), "--no-hot-reload", *mode], child_env)
                suffix = f"-single-panel-{process.pid}" if name.startswith("single") else (
                    "-no-sidebar" if name == "no-sidebar" else "")
                path = root / "runtime" / ("jcode-desktop" + suffix + ".sock")
                wait_for(lambda: socket_ready(path), name + " socket", processes)
                ids = wait_for(lambda: windows(process.pid), name + " native window", processes)
                assert len(ids) == 1, (name, ids)
                apps[name] = {"process": process, "window": ids[0], "socket": path,
                              "inode": path.stat().st_ino}
                get_state(name)
                time.sleep(2)
                focus(name)
                native("key", "--clearmodifiers", "Escape")
                time.sleep(.4)
                state = get_state(name)
                if name.startswith("single"):
                    assert_single(state)
                else:
                    assert state["single_panel"] is False, state
                    assert state["sidebar_visible"] is (name == "normal"), state
                unchanged(protected)
                protected[name] = invariant(state)
                report["instances"][name] = {"pid": process.pid, "window": ids[0],
                                             "socket": path.name, "state": state}
            assert len({app["process"].pid for app in apps.values()}) == 4
            assert len({app["window"] for app in apps.values()}) == 4
            assert len({app["inode"] for app in apps.values()}) == 4
            report["checks"].append("four independent PIDs, native windows and listening sockets")
            for name in ("single-a", "single-b"):
                focus(name)
                for key in WORKSPACE_KEYS:
                    native("key", "--clearmodifiers", key)
                    time.sleep(.15)
                    state = get_state(name)
                    assert_single(state)
                    assert invariant(state) == protected[name], (name, key, state)
                unchanged(protected)
                subprocess.run(["import", "-window", apps[name]["window"],
                                "png:" + str(output / (name + ".png"))], env=env, check=True, timeout=20)
                report["checks"].append({"window": name, "disabled_workspace_keys": WORKSPACE_KEYS})
            focus("single-a")
            native("key", "--clearmodifiers", "super+q")
            closing = apps["single-a"]
            wait_for(lambda: closing["process"].poll() is not None, "Super+Q process exit",
                     [app["process"] for name, app in apps.items() if name != "single-a"])
            assert closing["process"].returncode == 0, closing["process"].returncode
            assert not windows(closing["process"].pid), "closed window is still visible"
            del apps["single-a"]
            del protected["single-a"]
            unchanged(protected)
            focus("single-b")
            native("key", "--clearmodifiers", "ctrl+b")
            time.sleep(.2)
            assert_single(get_state("single-b"))
            unchanged(protected)
            report["checks"].append("Super+Q closes only its standalone process/window; all other instances preserved")
            assert not closing["socket"].exists(), "closed standalone socket was not removed"
            report["checks"].append("closed standalone socket removed")
            report["passed"] = True
        except BaseException as error:
            report["error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            if write_fd is not None:
                os.close(write_fd)
            os.close(read_fd)
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
            for process in reversed(processes):
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            for path in root.glob("*.state"):
                shutil.copy2(path, output / path.name)
            shutil.copytree(root / "logs", output / "app-logs", dirs_exist_ok=True)
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print("PASS: independent standalone windows. Evidence: " + str(output))


if __name__ == "__main__":
    main()
