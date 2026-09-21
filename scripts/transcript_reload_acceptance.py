#!/usr/bin/env python3
"""Verify transcript preservation across real linked-UI/cdylib reloads.

Build a matching host and UI plugin before running. This script never builds:
the isolated child's Cargo command is a gated no-op, so actual activation uses
the prebuilt plugin. All windows, native input, settings, and harness fixtures
are private to Xvfb. No live desktop socket or daemon is contacted.

Both fixtures receive unique transcript content through native input before
the first activation. Merely recreating the fixture therefore cannot pass.
Screenshots, OCR, navigation state, and logs are retained for every generation.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import time

from reload_lifecycle_probe import wait_for
from screenshot import isolated_env


def normalized(text):
    return " ".join(text.lower().split())


def check_evidence(text, panel, expected_count, marker, fixture, prompt=None):
    """Reject activity-only, reseeded, missing-row, and duplicate-marker frames."""
    text = normalized(text)
    assert panel["history_items"] == expected_count, (
        "Transcript item count changed across reload", expected_count, panel)
    assert text.count(marker) == 1, ("Unique transcript marker missing or duplicated", marker, text)
    if prompt:
        assert text.count(normalized(prompt)) == 1, ("User prompt missing or duplicated", prompt, text)
    phrases = {
        "streaming": ("small spinner", "checking the implementation", "responding"),
        "tool-streaming": ("agentgrep", "check the build"),
    }[fixture]
    for phrase in phrases:
        assert phrase in text, ("Expected transcript content did not paint", phrase, text)


def read_panel(state):
    try:
        lines = state.read_text().splitlines()
        navigation = json.loads(next(line.split("=", 1)[1] for line in lines
                                     if line.startswith("navigation=")))
        return next(panel for row in navigation["rows"] for panel in row["panels"]
                    if panel["session"] == "screenshot-fixture")
    except (OSError, ValueError, StopIteration, KeyError):
        return None


def run_case(root, fixture, binary, plugin, reloads):
    root.mkdir(parents=True, exist_ok=False)
    env = isolated_env(root)
    for name in ("home", "runtime", "config", "cache", "data", "jcode"):
        (root / name).mkdir(mode=0o700)
    env["VK_DRIVER_FILES"] = str(next(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json")))
    env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = fixture
    config = root / "desktop.toml"
    config.write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n')
    env["JCODE_DESKTOP_CONFIG"] = str(config)
    gate = root / "linked-ui-verified"
    cargo = root / "prebuilt-cargo"
    cargo.write_text('#!/bin/sh\nwhile [ ! -f "$TRANSCRIPT_PROBE_GATE" ]; do sleep 0.1; done\nexit 0\n')
    cargo.chmod(0o700)
    env["TRANSCRIPT_PROBE_GATE"] = str(gate)
    env["CARGO"] = str(cargo)
    wm_config = root / "openbox.xml"
    wm_config.write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<applications><application class="*"><decor>no</decor>
<maximized>yes</maximized></application></applications></openbox_config>''')
    diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
    state = root / "state"
    # Distinct from all fixtures. Avoid random hexadecimal suffixes because
    # OCR legitimately confuses 0/O and 1/l, obscuring the reload assertion.
    marker = "reloadproof"
    prompt = "Unique reload prompt preserved" if fixture == "tool-streaming" else None
    results = []
    processes = []

    def native(*args):
        subprocess.run(["xdotool", *args], env=env, cwd=root, check=True, timeout=10)

    def submit(text):
        native("mousemove", "900", "935", "click", "1")
        native("key", "--clearmodifiers", "ctrl+a")
        native("type", "--clearmodifiers", "--delay", "2", text)
        native("key", "--clearmodifiers", "Return")

    def capture(generation, expected_count, attempt=0):
        stem = f"generation-{generation}" + (f"-attempt-{attempt}" if attempt else "")
        # Capture even a bad frame, so failures leave useful visual evidence.
        time.sleep(.6)
        image = root / f"{stem}.png"
        subprocess.run(["import", "-window", "root", str(image)],
                       env=env, check=True, timeout=15)
        # Preserve original pixels for prose and exactly-once prompt checks.
        # Thresholding the entire transcript can erase small counters/descenders
        # in otherwise legible text. Only tool labels need separate enlargement.
        text = subprocess.check_output(["tesseract", str(image), "stdout"],
                                       env=env, stderr=subprocess.DEVNULL, timeout=30).decode()
        if fixture == "tool-streaming":
            # Full-screen OCR can miss small dim badge glyphs. Independently
            # OCR the original two-tool region at 4x, without changing expected text.
            # This crop contains no marker or user prompt, so exactly-once
            # checks remain based on the full transcript OCR above.
            label_image = root / f"{stem}-label.png"
            subprocess.run(["convert", str(image), "-crop", "600x60+245+120", "+repage",
                            "-resize", "400%", str(label_image)],
                           env=env, check=True, timeout=15, stderr=subprocess.DEVNULL)
            label_text = subprocess.check_output(
                ["tesseract", str(label_image), "stdout", "--psm", "6"],
                env=env, stderr=subprocess.DEVNULL, timeout=30).decode()
            (root / f"{stem}-label.txt").write_text(label_text)
            # Supplement only the tool labels, never reconstruct transcript
            # content or relax the exact unique-marker/prompt assertions.
            if any(phrase not in normalized(text) and phrase in normalized(label_text)
                   for phrase in ("agentgrep", "check the build")):
                text += "\n" + label_text
        (root / f"{stem}.txt").write_text(text)
        shutil.copyfile(state, root / f"{stem}.state")
        panel = read_panel(state)
        assert panel is not None, "Fixture panel vanished from navigation state"
        try:
            check_evidence(text, panel, expected_count, marker, fixture, prompt)
        except AssertionError:
            # At most three independently captured animation frames. Never
            # retry lost rows/markers/prompts, only ambiguous tool-label OCR.
            exact_state = (panel["history_items"] == expected_count
                           and normalized(text).count(marker) == 1
                           and (not prompt or normalized(text).count(normalized(prompt)) == 1))
            if fixture == "tool-streaming" and exact_state and attempt < 2:
                return capture(generation, expected_count, attempt + 1)
            raise
        result = {"generation": generation, "history_items": panel["history_items"],
                  "unique_marker_count": normalized(text).count(marker),
                  "unique_prompt_count": normalized(text).count(normalized(prompt)) if prompt else None,
                  "screenshot": image.name}
        results.append(result)
        (root / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps({"fixture": fixture, **result}), flush=True)

    try:
        with (root / "xvfb.log").open("w") as xlog, (root / "app.log").open("w") as log:
            read_fd, write_fd = os.pipe()
            try:
                xvfb = subprocess.Popen(
                    ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"],
                    pass_fds=(write_fd,), env=env, cwd=root, stdout=xlog, stderr=xlog)
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise TimeoutError("Private Xvfb failed to start")
                display = os.read(read_fd, 64).decode().strip()
                if not display.isdigit():
                    raise RuntimeError("Invalid private Xvfb display")
                env["DISPLAY"] = ":" + display
            finally:
                os.close(read_fd)
                if write_fd is not None:
                    os.close(write_fd)
            wm = subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(wm_config)],
                                  env=env, cwd=root, stdout=xlog, stderr=xlog)
            processes.append(wm)
            time.sleep(.5)
            if wm.poll() is not None:
                raise RuntimeError("Private window manager failed to start")
            app = subprocess.Popen([str(binary), "--hot-reload", str(plugin)],
                                   env=env, cwd=root, stdout=log, stderr=log)
            processes.append(app)
            wait_for(lambda: read_panel(state) is not None, app, timeout=45)
            time.sleep(1)
            native("key", "--clearmodifiers", "Escape")
            before = read_panel(state)["history_items"]
            # Unknown slash commands append a local Error item even while an
            # answer streams. They do not send requests or stop the live text.
            submit("/" + marker)
            wait_for(lambda: (read_panel(state) or {}).get("history_items") == before + 1, app)
            if prompt:
                # The tool fixture is idle, so this is an actual locally echoed
                # User row, not a queue entry. The offline harness cannot send it.
                submit(prompt)
                wait_for(lambda: (read_panel(state) or {}).get("history_items") == before + 2, app)
            expected_count = before + 1 + int(prompt is not None)
            capture(0, expected_count)
            assert not diagnostics.exists() or "activated UI generation 1 " not in diagnostics.read_text(), (
                "Initial activation was not held until unique content painted")
            gate.touch()
            for generation in range(1, reloads + 2):
                if generation > 1:
                    native("key", "--clearmodifiers", "ctrl+r")
                wait_for(lambda: diagnostics.exists() and
                         f"activated UI generation {generation} " in diagnostics.read_text(), app, timeout=45)
                capture(generation, expected_count)
            if app.poll() is not None:
                raise RuntimeError("Isolated app exited during reload acceptance")
    finally:
        gate.touch()
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    return results


def self_test():
    """Negative controls for the verifier, without starting any windows."""
    marker = "reloadproofabc123"
    good = marker + " small spinner checking the implementation Responding"
    check_evidence(good, {"history_items": 3}, 3, marker, "streaming")
    bad_cases = [("Responding", 3), (good + " " + marker, 3), (good, 2),
                 (good.replace("checking the implementation", ""), 3)]
    for text, count in bad_cases:
        try:
            check_evidence(text, {"history_items": count}, 3, marker, "streaming")
        except AssertionError:
            continue
        raise AssertionError(("Negative control incorrectly passed", text, count))
    prompt = "Unique reload prompt preserved"
    tools = marker + " agentgrep Check the build " + prompt
    check_evidence(tools, {"history_items": 6}, 6, marker, "tool-streaming", prompt)
    for text in (tools.replace("agentgrep", ""), tools + " " + prompt):
        try:
            check_evidence(text, {"history_items": 6}, 6, marker, "tool-streaming", prompt)
        except AssertionError:
            continue
        raise AssertionError(("Tool/prompt negative control incorrectly passed", text))
    print("Transcript verifier: two positive and six negative controls passed")


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, nargs="?", help="new artifact directory")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--plugin", type=Path, default=repo / "target/debug/libjcode_desktop_ui.so")
    parser.add_argument("--reloads", type=int, choices=range(1, 6), default=2,
                        help="Ctrl+R activations after the first cdylib activation")
    parser.add_argument("--fixture", choices=("streaming", "tool-streaming", "both"), default="both")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.output is None:
        parser.error("output is required unless --self-test is used")
    for tool in ("Xvfb", "openbox", "import", "convert", "xdotool", "tesseract"):
        if not shutil.which(tool):
            parser.error(f"missing required tool: {tool}")
    if not list(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json")):
        parser.error("Mesa lavapipe Vulkan driver is required")
    binary, plugin = args.binary.resolve(strict=True), args.plugin.resolve(strict=True)
    root = args.output.resolve()
    # The host creates an AF_UNIX instance socket beneath XDG_RUNTIME_DIR.
    if len(os.fsencode(root / "tool-streaming/runtime/jcode-desktop.sock")) >= 108:
        parser.error("artifact path is too long for Unix sockets, use a short target/ path")
    root.mkdir(parents=True, exist_ok=False)
    fixtures = ("streaming", "tool-streaming") if args.fixture == "both" else (args.fixture,)
    results = {fixture: run_case(root / fixture, fixture, binary, plugin, args.reloads)
               for fixture in fixtures}
    (root / "results.json").write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
