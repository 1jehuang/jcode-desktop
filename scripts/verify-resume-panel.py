#!/usr/bin/env python3
"""Verify the dedicated /resume surface and standalone resume on private Xvfb.

Uses an already-built real Desktop app with offline resume fixtures. No daemon,
provider, live display, builds or reloads. Retains PNGs, OCR, navigation and logs.
Example: python3 scripts/verify-resume-panel.py target/resume-panel.png
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import tempfile
import time

from screenshot import isolated_env


SESSION = "resume-fixture-alpha"
TITLE = "Resume acceptance alpha"


def navigation(path):
    """The renderer may be between truncating and writing its state file."""
    try:
        line = next(line for line in path.read_text().splitlines()
                    if line.startswith("navigation="))
        return json.loads(line.partition("=")[2])
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


def panels(state):
    return [panel for row in state.get("rows", []) for panel in row["panels"]]


def assert_single(state, size, chat=False):
    assert state["single_panel"] is True, state
    assert state["visible_panels"] == 1, state
    for field in ("sidebar_visible", "sidebar_overlay", "overview", "minimap_visible"):
        assert state[field] is False, (field, state)
    assert not state["tab_targets"], state
    assert state["viewport"] == list(size), state
    if chat:
        actual = [panel for panel in panels(state) if panel["focused"]]
        assert len(actual) == 1, actual
        assert actual[0]["width"] == 1 and not actual[0]["closing"], actual
        assert state["focused_slot"] == 0, "Resumed chat must be the standalone anchor"
        assert abs(state["canvas_width"] - size[0]) <= 1, state


def run_mode(binary, output, evidence, driver, single, report):
    name = "single-panel" if single else "workspace"
    size = (800, 950) if single else (1440, 1000)
    mode = {"mode": name, "checks": [], "passed": False}
    report["runs"].append(mode)
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", output.parent))
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="resume-", dir=scratch) as temporary:
        root = Path(temporary)
        env = isolated_env(root)
        env.update(VK_DRIVER_FILES=str(driver),
                   JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT="empty",
                   JCODE_DESKTOP_SCREENSHOT_PANELS="1",
                   JCODE_DESKTOP_SCREENSHOT_RESUME_FIXTURE="1",
                   JCODE_DESKTOP_CONFIG=str(root / "desktop.toml"))
        for directory in ("home", "runtime", "config", "cache", "data", "logs", "jcode"):
            (root / directory).mkdir(mode=0o700)
        assert len(str(root / "runtime/jcode-desktop-single-panel-99999999.sock").encode()) < 104, "Private socket path too long"
        (root / "desktop.toml").write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n')
        wm = root / "openbox.xml"
        wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><keyboard/>'
                      '<applications><application class="*"><decor>no</decor>'
                      '<maximized>yes</maximized></application></applications></openbox_config>')
        processes, logs = [], []
        read_fd, write_fd = os.pipe()
        state_path = root / "state"

        def launch(label, command, **kwargs):
            log = (evidence / f"{name}-{label}.log").open("w")
            logs.append(log)
            process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log,
                                       start_new_session=True, **kwargs)
            processes.append(process)
            return process

        def wait(predicate, label, timeout=35):
            deadline = time.monotonic() + timeout
            while True:
                dead = [(p.args, p.poll()) for p in processes if p.poll() is not None]
                assert not dead, f"Child exited during {label}: {dead}"
                result = predicate()
                if result:
                    return result
                assert time.monotonic() < deadline, f"Timed out: {label}. State: {navigation(state_path)}"
                time.sleep(.1)

        def native(*args):
            return subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root,
                                  text=True, capture_output=True, check=True, timeout=15)

        def key(value):
            native("key", "--clearmodifiers", value)

        def type_text(value):
            native("type", "--clearmodifiers", "--delay", "30", value)

        def state_check(label, check):
            last_error = [None]
            def ready():
                state = navigation(state_path)
                if state is None:
                    return None
                try:
                    check(state)
                    return state
                except (AssertionError, KeyError) as error:
                    last_error[0] = str(error)
                    return None
            try:
                state = wait(ready, label)
            except AssertionError as error:
                raise AssertionError(f"{error}; last check: {last_error[0]}") from error
            (evidence / f"{name}-{label}.json").write_text(json.dumps(state, indent=2) + "\n")
            mode["checks"].append(label)
            print("JCODE_CHECKPOINT " + json.dumps({"message": name + ": " + label}), flush=True)
            return state

        def capture(path, present=(), absent=()):
            def rendered():
                subprocess.run(["import", "-window", "root", "png:" + str(path)],
                               env=env, check=True, timeout=15)
                text = subprocess.check_output(["tesseract", str(path), "stdout", "--psm", "11"],
                                               env=env, stderr=subprocess.DEVNULL, timeout=30).decode()
                path.with_suffix(".txt").write_text(text)
                normalized = " ".join(text.casefold().split())
                return (all(phrase.casefold() in normalized for phrase in present)
                        and all(phrase.casefold() not in normalized for phrase in absent))
            wait(rendered, "rendered pixels " + path.name)
            mode["checks"].append({"png": str(path), "visible": list(present), "absent": list(absent)})

        def picker(state):
            assert state["resume_picker"] is True, state
            assert state["single_panel"] is single, state
            if single:
                assert_single(state, size)

        def resumed(state):
            assert state["resume_picker"] is False, state
            focused = [p for p in panels(state) if p["focused"]]
            assert len(focused) == 1 and focused[0]["session"] == SESSION, state
            assert state["keyboard_panel"] == focused[0]["slot"], state
            assert not state["camera_motion"] and not state["tab_motion"], state
            assert state["single_panel"] is single, state
            if single:
                assert_single(state, size, chat=True)

        try:
            launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                            f"{size[0]}x{size[1]}x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
            os.close(write_fd)
            write_fd = None
            assert select.select([read_fd], [], [], 15)[0], "Xvfb startup timeout"
            display = os.read(read_fd, 64).decode().strip()
            assert display.isdigit(), "Xvfb did not allocate a private display"
            env["DISPLAY"] = ":" + display
            launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm)])
            time.sleep(.5)
            app = launch("app", [str(binary), "--no-hot-reload", *(["--resume"] if single else [])])
            wait(lambda: navigation(state_path), "initial fixture")
            window = native("search", "--sync", "--onlyvisible", "--pid", app.pid).stdout.split()
            assert len(window) == 1, window
            native("windowactivate", "--sync", window[0])
            time.sleep(1)
            if single:
                state_check("resume-launch-opens-standalone-picker", picker)
                capture(evidence / "single-panel-startup.png", ("Resume session", TITLE, "Resume acceptance beta"))
            key("Escape")
            time.sleep(.3)
            # The shortcut must open the same picker without consuming a draft.
            source = wait(lambda: navigation(state_path), "source chat state")
            source_sessions = [p["session"] for p in panels(source)]
            type_text("Draft survives resume picker")
            key("alt+r")
            state_check("shortcut-opens-picker", picker)
            key("Escape")

            def source_restored(state):
                assert state["resume_picker"] is False, state
                assert [p["session"] for p in panels(state)] == source_sessions, state
                assert state["focused_slot"] == source["focused_slot"], state
                assert state["keyboard_panel"] == state["focused_slot"], state
                if single:
                    assert_single(state, size, chat=True)

            state_check("shortcut-cancel-restores-source", source_restored)
            capture(evidence / f"{name}-preserved-draft.png", ("Draft survives resume picker",))
            key("ctrl+a")
            # Slash submission crosses native input and normal local routing.
            type_text("/resume")
            key("Return")
            state_check("dedicated-picker", picker)
            picker_png = output.with_name(output.stem + "-single-picker.png") if single else output
            capture(picker_png, ("Resume session", TITLE, "Resume acceptance beta"))
            type_text("Resume acceptance")
            capture(evidence / f"{name}-preview-alpha.png", ("Status: idle", "Alpha conversation preview"))
            key("Down")
            capture(evidence / f"{name}-preview-beta.png", ("Status: running", "Beta conversation preview"), ("Status: idle", "Alpha conversation preview"))
            key("Up")
            capture(evidence / f"{name}-preview-return.png", ("Status: idle", "Alpha conversation preview"), ("Status: running", "Beta conversation preview"))
            key("ctrl+3")
            capture(evidence / f"{name}-saved.png", (TITLE,), ("Resume acceptance beta",))
            key("ctrl+1")
            capture(evidence / f"{name}-all.png", (TITLE, "Resume acceptance beta"))
            key("ctrl+a")
            type_text(TITLE)
            # Assert filtering is visible, not just a hidden retained query.
            capture(evidence / f"{name}-filtered.png", (TITLE,), ("Resume acceptance beta",))
            key("Return")
            state_check("resumed-chat", resumed)
            resumed_png = output.with_name(output.stem + ("-single-panel.png" if single else "-resumed.png"))
            type_text("Resumed composer focus verified")
            capture(resumed_png, ("Resumed composer focus verified",))
            # Reopen and dismiss on the resumed chat, preserving identity and
            # standalone presentation without selecting another session.
            key("ctrl+a")
            type_text("/resume")
            key("Return")
            state_check("reopened-picker", picker)
            key("Escape")
            state_check("dismiss-restores-chat", resumed)
            mode["passed"] = True
            print("PASS: " + name + " dedicated picker, filtering, resume and keyboard focus", flush=True)
        except BaseException:
            if env.get("DISPLAY"):
                try:
                    subprocess.run(["import", "-window", "root", "png:" + str(evidence / f"{name}-failure.png")],
                                   env=env, check=True, timeout=15)
                except (OSError, subprocess.SubprocessError):
                    pass
            raise
        finally:
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
            for process in reversed(processes):
                if process.poll() is None:
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
            for process in reversed(processes):
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
            mode["cleanup"] = [{"pid": p.pid, "exit_code": p.returncode} for p in processes]
            if state_path.exists():
                shutil.copy2(state_path, evidence / f"{name}-final.state")
            shutil.copytree(root / "logs", evidence / f"{name}-logs", dirs_exist_ok=True)
            for log in logs:
                log.close()


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, nargs="?", default=repo / "target/resume-panel.png")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    args = parser.parse_args()
    for executable in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(executable, path="/usr/bin:/bin"):
            parser.error("missing executable: " + executable)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe Vulkan driver required")
    binary, output = args.binary.resolve(), args.output.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must be an already-built executable")
    evidence = output.with_name(output.stem + "-evidence")
    artifacts = [output, evidence, *[output.with_name(output.stem + suffix + ".png")
                                   for suffix in ("-single-picker", "-single-panel", "-resumed")]]
    if any(path.exists() for path in artifacts):
        parser.error("refusing to overwrite existing evidence, choose a new output filename")
    evidence.mkdir(parents=True)
    report = {"passed": False, "binary": str(binary), "binary_mtime_ns": binary.stat().st_mtime_ns, "runs": [],
              "scope": "real linked Desktop UI, offline fixtures, native input and rendered pixels on private Xvfb",
              "not_tested": ["live Desktop", "hot reload", "daemon saved-session I/O", "provider calls"]}
    try:
        for single in (False, True):
            run_mode(binary, output, evidence, drivers[0], single, report)
        report["passed"] = True
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        (evidence / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": True, "evidence": str(evidence)}))


if __name__ == "__main__":
    main()
