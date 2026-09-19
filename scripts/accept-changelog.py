#!/usr/bin/env python3
"""Accept the read-only Desktop changelog using native input on private Xvfb.

Requires current prebuilt artifacts. Never invokes a real build or user's daemon.
--hot-reload exercises actual Ctrl+R activation with a no-build Cargo shim and
an immutable copy of --plugin. The explicit screenshot flag opens the changelog
through the real acknowledgement policy. Native restarts check first launch,
same-build suppression, and an explicitly seeded older-build marker.

Example: python3 scripts/accept-changelog.py target/changelog-accept --hot-reload
Artifacts include screenshots, OCR, live navigation, recovery snapshots and JSON.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import select
import shutil
import signal
import subprocess
import time

from screenshot import isolated_env
from default_directory_acceptance import NativeUI
from model_picker_acceptance import normalized, phrase_bounds

CHANGELOG = "desktop://changelog"
SESSION = "screenshot-fixture"
DRAFT = "Changelog reload preserves this unsent draft"


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
    return [p for row in state["rows"] for p in row["panels"] if not p.get("closing")]


def runtime_files(root):
    """Detect runtime-session writes, including creation of unexpected files."""
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob("*") if p.is_file()}


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new private artifact directory")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--plugin", type=Path, default=repo / "target/debug/libjcode_desktop_ui.so")
    parser.add_argument("--hot-reload", action="store_true", help="verify Ctrl+R using the prebuilt plugin, without compilation")
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract", "cp"):
        if not shutil.which(tool):
            parser.error("missing executable: " + tool)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe Vulkan driver")
    source_binary = args.binary.resolve()
    source_plugin = args.plugin.resolve()
    if not source_binary.is_file():
        parser.error("desktop binary does not exist: " + str(source_binary))
    if args.hot_reload and not source_plugin.is_file():
        parser.error("prebuilt plugin does not exist: " + str(source_plugin))
    build_version = subprocess.check_output(
        [str(source_binary), "--version"], text=True, timeout=15).strip()
    version_match = re.fullmatch(r"Jcode Desktop (v\d+\.\d+\.\d+-dev) \([^)]+\)", build_version)
    if not version_match:
        parser.error("changelog development acceptance requires a numbered dev binary: " + build_version)
    root = args.output.resolve()
    if len(str(root / "runtime/jcode-desktop.sock").encode()) >= 104:
        parser.error("output path is too long for private Unix sockets")
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ("home", "runtime", "config", "cache", "data", "jcode", "tmp"):
        (root / name).mkdir(mode=0o700)
    binary, plugin = root / "jcode-desktop", root / "prebuilt-ui.so"
    for source, dest in [(source_binary, binary)] + ([(source_plugin, plugin)] if args.hot_reload else []):
        subprocess.run(["cp", "--reflink=auto", "--preserve=mode", "--", str(source), str(dest)], check=True, timeout=60)
    env = isolated_env(root)
    env.update({"TMPDIR": str(root / "tmp"), "JCODE_NO_TELEMETRY": "1",
                "JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "empty",
                "JCODE_DESKTOP_SCREENSHOT_PANELS": "1",
                "JCODE_DESKTOP_SCREENSHOT_CHANGELOG": "1",
                "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
                "VK_DRIVER_FILES": str(drivers[0])})
    (root / "desktop.toml").write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n[workspace]\ncoaching_hints = false\n')
    if args.hot_reload:
        shim = root / "cargo-no-build"
        shim.write_text("#!/bin/sh\n# Activate only the supplied prebuilt plugin.\nexit 0\n")
        shim.chmod(0o700)
        env["CARGO"] = str(shim)
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
    state_path = root / "state"
    log_path = root / "logs/jcode-desktop/jcode-desktop.log"
    recovery = log_path.parent / "crash-recovery.json"
    processes, logs = [], []
    app = None
    report = {"checks": [], "hot_reload": args.hot_reload,
              "build_version": build_version,
              "scope": "offline native UI and real identity policy with a seeded older marker, not backend acknowledgement"}
    runtime_before = runtime_files(root / "jcode")

    def launch(name, command, **kwargs):
        log = (root / (name + ".log")).open("w")
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log,
                                   start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait(label, predicate):
        deadline = time.monotonic() + args.timeout
        while True:
            if app is not None and app.poll() is not None:
                raise AssertionError(f"Desktop exited during {label}: {app.returncode}")
            result = predicate()
            if result:
                return result
            if time.monotonic() >= deadline:
                raise AssertionError(f"Timeout: {label}. See {state_path} and {log_path}")
            time.sleep(.1)

    def state_with(count):
        state = navigation(state_path)
        if state and not state["camera_motion"] and not state["tab_motion"]:
            matches = [p for p in panels(state) if p["session"] == CHANGELOG]
            if len(matches) == count and len(panels(state)) == 1 + count:
                if not matches or (matches[0]["focused"] and state["keyboard_panel"] == matches[0]["slot"]):
                    return state
        return None

    def key(chord):
        ui.native("key", "--clearmodifiers", chord)

    def type_text(text):
        ui.native("type", "--clearmodifiers", "--delay", "15", text)

    def capture(label, changelog=True, expected=()):
        def inspect(image):
            words = ui.words(image, (0, 0, image.width, image.height), label)
            if changelog:
                # Dense release history can make OCR omit the tiny tab row.
                # Verify the actual focused panel identity via navigation instead.
                assert state_with(1), "expected one focused changelog panel"
                phrase_bounds(words, "What's new in Jcode Desktop")
                phrase_bounds(words, "Close")
            for phrase in expected:
                phrase_bounds(words, phrase)
            return words
        words = ui.wait_frame(label, inspect)
        report["checks"].append(label)
        print("PASS: " + label, flush=True)
        return words

    def checkpoint(label, expected_draft):
        def matching():
            data = read_json(recovery)
            if data and data["pid"] == app.pid:
                slots = data["snapshot"]["slots"]
                assert all(s["panel"]["session_id"] != CHANGELOG for s in slots), "read-only changelog leaked into recoverable runtime sessions"
                matches = [s for s in slots if s["panel"]["session_id"] == SESSION]
                if len(matches) == 1 and matches[0]["panel"]["draft"]["content"] == expected_draft:
                    return data
            return None
        data = wait(label, matching)
        (root / (label + "-recovery.json")).write_text(json.dumps(data, indent=2) + "\n")
        return data

    def focus_composer():
        key("super+u")
        wait("fixture composer focused", lambda: (s := navigation(state_path)) and any(
            p["session"] == SESSION and p["focused"] and s["keyboard_panel"] == p["slot"] for p in panels(s)))

    def slash_open():
        focus_composer()
        key("ctrl+a")
        type_text("/changelog")
        key("Return")
        return wait("slash command opens one focused changelog", lambda: state_with(1))

    def start(label, expected_count):
        nonlocal app
        state_path.unlink(missing_ok=True)
        app = launch(label, [str(binary)] + (["--hot-reload", str(plugin)] if args.hot_reload else ["--no-hot-reload"]))
        wait(label + " initial frame", lambda: navigation(state_path))
        ui.native("search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync")
        # Every fresh process starts with the beta notice owning keyboard focus.
        # Dismiss it through native input before asserting changelog focus.
        key("Escape")
        wait(label + " activation", lambda: state_with(expected_count))

    def quit_app():
        nonlocal app
        key("super+shift+q")
        assert app.wait(timeout=15) == 0, "Native Quit must exit successfully"
        app = None
        wait("normal quit clears crash recovery", lambda: not recovery.exists())

    read_fd, write_fd = os.pipe()
    try:
        xvfb = launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
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
        ui = NativeUI(root / "changelog.png", env, root)
        start("first-launch", 1)
        words = capture("initial", expected=("RUNNING VERSION", version_match.group(1)))
        phrase_bounds(words, "RUNNING VERSION")
        phrase_bounds(words, version_match.group(1))
        phrase_bounds(words, "Latest updates")
        initial_text = normalized(" ".join(w["text"] for w in words))
        assert normalized("Uncommitted files") not in initial_text, "development details leaked into the summary"
        ui.click(phrase_bounds(words, "Release history"))
        words = capture("versioned-history", expected=("UTC", "Unreleased"))
        phrase_bounds(words, "UTC")
        phrase_bounds(words, "Unreleased")
        # Plain arrow keys switch views without leaving the read-only panel.
        key("Right")
        words = capture("build-details", expected=("Development build snapshot",))
        phrase_bounds(words, "Development build snapshot")
        key("Right")
        words = capture("summary-keyboard-return", expected=("RUNNING VERSION", "View history"))
        phrase_bounds(words, "Latest updates")
        marker = root / "changelog-last-seen-build"
        identity = wait("first launch acknowledges build", lambda: marker.read_text() if marker.exists() else None)
        quit_app()
        # A development launch activates the linked UI and then performs an
        # actual startup plugin reload. The existing policy intentionally
        # opens notes on every reload, even with the same build identity.
        expected = 1 if args.hot_reload else 0
        start("same-build", expected)
        # Observe settled startup for longer than the startup/plugin activation.
        time.sleep(1.5)
        assert state_with(expected), "same-build startup violated the launch/reload policy"
        capture("same-build-reload-opens" if args.hot_reload else "same-build-stays-closed", args.hot_reload)
        assert marker.read_text() == identity
        if args.hot_reload:
            key("Escape")
            wait("startup reload summary closes", lambda: state_with(0))
        slash_open()
        words = capture("same-build-manual-summary", expected=("No new changes in this build",))
        key("Escape")
        wait("manual summary closes", lambda: state_with(0))
        quit_app()
        marker.write_text("0.0.0+acceptance-older-build")
        start("changed-build", 1)
        capture("changed-build-opens")
        assert marker.read_text() == identity, "changed build was not acknowledged"
        key("Escape")
        wait("Escape closes changelog", lambda: state_with(0))
        capture("escape-closed", False)
        slash_open()
        first = next(p["id"] for p in panels(navigation(state_path)) if p["session"] == CHANGELOG)
        capture("slash-reopened")
        slash_open()
        second = next(p["id"] for p in panels(navigation(state_path)) if p["session"] == CHANGELOG)
        assert first == second, "repeated /changelog replaced the existing panel"
        words = capture("slash-single-instance")
        ui.click(phrase_bounds(words, "Close"))
        wait("Close button dismisses changelog", lambda: state_with(0))
        capture("button-closed", False)
        focus_composer()
        type_text(DRAFT)
        checkpoint("draft-before", DRAFT)
        # Opening through the command would consume the draft. Reload opens the
        # panel independently and must preserve the composer snapshot instead.
        if args.hot_reload:
            generation = log_path.read_text().count("activated UI generation")
            owner = checkpoint("pre-reload", DRAFT)["owner"]
            key("ctrl+r")
            wait("actual Ctrl+R UI generation", lambda: log_path.read_text().count("activated UI generation") > generation)
            wait("reload activates changelog", lambda: state_with(1))
            checkpoint("draft-after-reload", DRAFT)
            wait("new generation owns checkpoint", lambda: (c := read_json(recovery)) and c["owner"] != owner)
            capture("reload-opened")
            type_text("MUST NOT EDIT OR SUBMIT")
            key("Return")
            time.sleep(1.2)
            checkpoint("read-only-draft-unchanged", DRAFT)
            wait("typing keeps one changelog", lambda: state_with(1))
            words = capture("read-only")
            assert normalized("MUST NOT EDIT OR SUBMIT") not in normalized(" ".join(w["text"] for w in words)), "read-only panel rendered typed input"
            key("Escape")
            wait("reload panel closes", lambda: state_with(0))
            words = capture("preserved-draft", False)
            # OCR may join the blinking caret to the final word ("draftl").
            # The recovery checkpoint above already verifies exact draft bytes.
            assert normalized(DRAFT) in normalized(" ".join(w["text"] for w in words))
        else:
            slash_open()
            type_text("MUST NOT EDIT OR SUBMIT")
            key("Return")
            time.sleep(1.2)
            checkpoint("read-only-empty-draft", "")
            wait("typing keeps one changelog", lambda: state_with(1))
            words = capture("read-only")
            assert normalized("MUST NOT EDIT OR SUBMIT") not in normalized(" ".join(w["text"] for w in words)), "read-only panel rendered typed input"
        assert runtime_files(root / "jcode") == runtime_before, "changelog workflow wrote runtime files in JCODE_HOME"
        report["checks"].append("runtime-files-unchanged")
        report["private_display"] = env["DISPLAY"]
        report["result"] = "passed"
    except Exception as error:
        report["result"] = "failed"
        report["error"] = str(error)
        raise
    finally:
        (root / "evidence.json").write_text(json.dumps(report, indent=2) + "\n")
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=10)
        for log in logs:
            log.close()
    print(f"PASS: native changelog acceptance. Evidence: {root}")


if __name__ == "__main__":
    main()
