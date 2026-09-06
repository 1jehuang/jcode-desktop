#!/usr/bin/env python3
"""Accept crash-only workspace recovery in the real app on private Xvfb.

Uses offline fixture sessions, never the user's daemon, credentials, or display.
Requires a current desktop binary. --reloads exercises Ctrl+R with a prebuilt
plugin and a no-build Cargo shim, so it tests activation, not compilation.
Artifacts include native screenshots, navigation state, and recovery checkpoints.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import time

from screenshot import isolated_env


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def navigation(path):
    try:
        for line in path.read_text().splitlines():
            if line.startswith("navigation="):
                return json.loads(line.split("=", 1)[1])
    except (OSError, ValueError):
        pass
    return None


def panels(state):
    return [panel for row in state["rows"] for panel in row["panels"]]


def layout(state):
    # Entity IDs are process-local. Compare actual session identities instead.
    return {
        "active_row": state["active_row"],
        "focused_slot": state["focused_slot"],
        "keyboard_panel": state["keyboard_panel"],
        "overview": state["overview"],
        "rows": [[(p["session"], round(p["width"], 4), p["focused"])
                  for p in row["panels"]] for row in state["rows"]],
    }


def draft(checkpoint, session):
    if checkpoint:
        for slot in checkpoint["snapshot"]["slots"]:
            if slot["panel"]["session_id"] == session:
                return slot["panel"]["draft"]["content"]
    return None


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new private artifact directory")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--plugin", type=Path, default=repo / "target/debug/libjcode_desktop_ui.so")
    parser.add_argument("--reloads", type=int, default=0, help="prebuilt-plugin Ctrl+R cycles before crash")
    parser.add_argument("--no-sidebar", action="store_true", help="exercise the independent workspace recovery file")
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--quit-key", default="super+shift+q", help="native Quit action, not close-panel")
    args = parser.parse_args()
    if not 0 <= args.reloads <= 10 or args.timeout <= 0:
        parser.error("reloads must be 0..10 and timeout must be positive")
    for tool in ("Xvfb", "openbox", "xdotool", "import", "identify", "cp"):
        if not shutil.which(tool):
            parser.error("missing executable: " + tool)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe Vulkan driver")
    binary = args.binary.absolute()
    if not binary.is_file():
        parser.error("desktop binary does not exist: " + str(binary))
    plugin = args.plugin.resolve() if args.reloads else None
    if plugin is not None and not plugin.is_file():
        parser.error("prebuilt plugin does not exist: " + str(plugin))
    root = args.output.resolve()
    if len(str(root / "runtime/jcode-desktop-no-sidebar.sock").encode()) >= 104:
        parser.error("output path is too long for private Unix sockets")
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    # Cargo may replace the executable or remove deps/*.so during this run.
    # Reuse one immutable artifact pair across all process restarts and reloads.
    source_binary, source_plugin = binary, plugin
    binary = root / "jcode-desktop"
    subprocess.run(["cp", "--reflink=auto", "--preserve=mode", "--",
                    str(source_binary), str(binary)], check=True, timeout=60)
    if plugin is not None:
        plugin = root / "prebuilt-ui.so"
        subprocess.run(["cp", "--reflink=auto", "--preserve=mode", "--",
                        str(source_plugin), str(plugin)], check=True, timeout=60)
    for name in ("home", "runtime", "config", "cache", "data", "jcode", "tmp"):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.update({
        "TMPDIR": str(root / "tmp"),
        "JCODE_NO_TELEMETRY": "1",
        "JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "empty",
        "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
        "VK_DRIVER_FILES": str(drivers[0]),
    })
    (root / "desktop.toml").write_text(
        '[appearance]\nlayout_mode = "normal"\ntheme = "warm-neutral"\n'
        '[workspace]\ncoaching_hints = false\n')
    if args.reloads:
        shim = root / "cargo-no-build"
        shim.write_text("#!/bin/sh\n# Reload the explicitly supplied prebuilt plugin.\nexit 0\n")
        shim.chmod(0o700)
        env["CARGO"] = str(shim)
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc">'
                         '<applications><application class="*"><decor>no</decor>'
                         '<maximized>yes</maximized></application></applications></openbox_config>')
    state_path = root / "state"
    log_path = root / "logs/jcode-desktop/jcode-desktop.log"
    recovery = log_path.parent / ("crash-recovery-no-sidebar.json" if args.no_sidebar
                                  else "crash-recovery.json")
    processes, logs, evidence = [], [], []
    app = None

    def launch(name, command, **kwargs):
        log = (root / (name + ".log")).open("w")
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log,
                                   stderr=log, start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait(label, predicate, timeout=None):
        deadline = time.monotonic() + (args.timeout if timeout is None else timeout)
        while True:
            if app is not None and app.poll() is not None:
                raise AssertionError(f"Desktop exited during {label}: {app.returncode}")
            result = predicate()
            if result:
                return result
            if time.monotonic() >= deadline:
                detail = state_path.read_text() if state_path.exists() else "no state"
                raise AssertionError(f"Timeout: {label}\n{detail}\nSee {log_path}")
            time.sleep(.1)

    def key(chord):
        subprocess.run(["xdotool", "key", "--clearmodifiers", chord], env=env,
                       check=True, timeout=10)

    def type_text(text):
        subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "15", text],
                       env=env, check=True, timeout=15)

    def count_generations():
        return log_path.read_text().count("activated UI generation") if log_path.exists() else 0

    def start(label, fixture_count, expected_count):
        nonlocal app
        state_path.unlink(missing_ok=True)
        env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = str(fixture_count)
        before = count_generations()
        command = [str(binary)] + (["--hot-reload", str(plugin)] if args.reloads else ["--no-hot-reload"])
        if args.no_sidebar:
            command.append("--no-sidebar")
        app = launch(label, command)
        wait(label + " render", lambda: (s := navigation(state_path)) is not None
             and len(panels(s)) == expected_count and s)
        if args.reloads:
            wait(label + " startup activation", lambda: count_generations() > before)
        # Observe actual render state again after the startup root replacement.
        wait(label + " settled render", lambda: (s := navigation(state_path)) is not None
             and len(panels(s)) == expected_count and not s["camera_motion"] and s)
        subprocess.run(["xdotool", "search", "--sync", "--onlyvisible", "--class",
                        "^jcode-desktop$", "windowactivate", "--sync"], env=env,
                       check=True, timeout=15)
        return app

    def checkpoint(label, predicate=lambda c: True):
        result = wait(label, lambda: (c := read_json(recovery)) is not None
                      and c["pid"] == app.pid and predicate(c) and c)
        assert recovery.stat().st_mode & 0o777 == 0o600, "checkpoint must be private"
        return result

    def capture(label):
        state = wait(label + " state", lambda: navigation(state_path))
        (root / (label + ".json")).write_text(json.dumps(state, indent=2) + "\n")
        image = root / (label + ".png")

        def presented():
            subprocess.run(["import", "-window", "root", "png:" + str(image)],
                           env=env, check=True, timeout=15)
            colors, deviation = subprocess.check_output(
                ["identify", "-format", "%k %[fx:standard_deviation]", str(image)],
                env=env, text=True, timeout=15).split()
            # Navigation can be emitted before lavapipe presents its first frame.
            # Do not accept an all-black Xvfb root as visual evidence.
            return int(colors) > 64 and float(deviation) > .01 and int(colors)

        colors = wait(label + " nonblank presented pixels", presented)
        evidence.append({"checkpoint": label, "layout": layout(state), "image_colors": colors})
        print(f"PASS checkpoint: {label}", flush=True)
        return state

    def crash():
        nonlocal app
        app.kill()  # Only the process launched by this isolated harness.
        assert app.wait(timeout=10) == -signal.SIGKILL
        app = None

    def quit_normally():
        nonlocal app
        key(args.quit_key)
        assert app.wait(timeout=15) == 0, "native Quit must exit successfully"
        app = None
        wait("normal quit removes recovery checkpoint", lambda: not recovery.exists(), 5)

    read_fd, write_fd = os.pipe()
    try:
        xvfb = launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                               "1800x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        if not select.select([read_fd], [], [], 15)[0]:
            raise AssertionError("Xvfb startup timeout")
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit(), "Xvfb did not allocate a private display"
        env["DISPLAY"] = ":" + display
        wm = launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm_config)])
        time.sleep(.5)
        assert xvfb.poll() is None and wm.poll() is None

        start("initial", 3, 3)
        key("super+u")
        wait("first panel focus", lambda: (s := navigation(state_path)) is not None
             and s["focused_slot"] == 0 and s["keyboard_panel"] == 0)
        text = "Crash recovery keeps this unsent draft"
        type_text(text)
        key("super+r")
        original = checkpoint("three panels and typed draft persisted", lambda c:
                              len(c["snapshot"]["slots"]) == 3
                              and draft(c, "screenshot-fixture") == text
                              and c["snapshot"]["focus"] == {"Panel": 0}
                              and abs(c["snapshot"]["slots"][0]["width_fraction"] - 1 / 3) > .01)
        before = capture("before-crash")
        assert any(abs(p["width"] - 1 / 3) > .01 for p in panels(before)), "width shortcut had no effect"
        baseline = layout(before)
        for index in range(args.reloads):
            generation = count_generations()
            previous_owner = checkpoint("pre-reload writer")["owner"]
            key("ctrl+r")
            wait("Ctrl+R activation", lambda: count_generations() > generation)
            wait("layout retained across hot reload", lambda: (s := navigation(state_path)) is not None
                 and layout(s) == baseline)
            checkpoint("new writer retains draft across hot reload", lambda c:
                       c["owner"] != previous_owner and draft(c, "screenshot-fixture") == text)
            capture(f"reload-{index + 1}")
        original = checkpoint("final crash checkpoint", lambda c: draft(c, "screenshot-fixture") == text)
        (root / "before-crash-recovery.json").write_text(json.dumps(original, indent=2) + "\n")
        crash()
        start("restored", 1, 3)
        wait("exact restored layout and keyboard focus", lambda: (s := navigation(state_path)) is not None
             and layout(s) == baseline)
        checkpoint("restored draft", lambda c: draft(c, "screenshot-fixture") == text)
        type_text(" plus native typing")
        checkpoint("typing appends to restored draft", lambda c:
                   draft(c, "screenshot-fixture") == text + " plus native typing")
        capture("restored-after-sigkill")

        crash()
        expired = read_json(recovery)
        assert expired is not None
        expired["updated_at_ms"] = int(time.time() * 1000) - 120_000
        recovery.write_text(json.dumps(expired))
        start("expired", 1, 1)
        checkpoint("expired checkpoint replaced by fresh workspace", lambda c:
                   len(c["snapshot"]["slots"]) == 1 and draft(c, "screenshot-fixture") == "")
        capture("expired-is-fresh")
        quit_normally()

        # Quit from three panels, not one, so a stale checkpoint cannot make the
        # one-panel fresh-start assertion pass accidentally.
        start("before-normal-quit", 3, 3)
        checkpoint("three panels before normal quit", lambda c: len(c["snapshot"]["slots"]) == 3)
        quit_normally()
        start("after-normal-quit", 1, 1)
        checkpoint("normal quit starts fresh", lambda c:
                   len(c["snapshot"]["slots"]) == 1 and draft(c, "screenshot-fixture") == "")
        capture("normal-quit-is-fresh")
        quit_normally()
        (root / "results.json").write_text(json.dumps({
            "result": "passed", "runtime": "offline fixture, private Xvfb/Openbox",
            "hot_reload_cycles": args.reloads, "hot_reload_build": "prebuilt plugin, no compilation",
            "source_binary": str(source_binary),
            "source_plugin": str(source_plugin) if source_plugin else None,
            "no_sidebar": args.no_sidebar, "checks": evidence,
        }, indent=2) + "\n")
        print(f"PASS: SIGKILL recovery, draft/focus/layout, expiration, normal quit, {args.reloads} UI reloads. Artifacts: {root}")
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
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


if __name__ == "__main__":
    main()
