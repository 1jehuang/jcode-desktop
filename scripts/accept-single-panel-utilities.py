#!/usr/bin/env python3
"""Verify standalone utility child windows on private Xvfb with native clicks.

Uses an already-built linked Desktop binary and offline fixtures. Never builds,
reloads, accesses live desktop sockets, or submits account/model requests.
"""
import argparse
from contextlib import ExitStack
import importlib.util
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time

from PIL import Image
from model_picker_acceptance import parse_words, phrase_bounds
from screenshot import isolated_env

SPEC = importlib.util.spec_from_file_location("standalone", Path(__file__).with_name("accept-single-panel.py"))
STANDALONE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STANDALONE)


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new evidence directory")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    args = parser.parse_args()
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool):
            parser.error("missing executable: " + tool)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe is required")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        parser.error("refusing to overwrite " + str(output))
    output.mkdir(parents=True)
    report = {"passed": False, "binary": str(binary), "checks": [],
              "scope": "native Accounts child and shared model-picker footer clicks on private offline Xvfb"}
    with tempfile.TemporaryDirectory(prefix="spu-", dir=repo / "target") as temporary, ExitStack() as stack:
        root = Path(temporary)
        env = isolated_env(root)
        env.update(VK_DRIVER_FILES=str(drivers[0]), JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT="empty",
                   JCODE_DESKTOP_SCREENSHOT_PANELS="1", JCODE_DESKTOP_SCREENSHOT_MODELS="1",
                   JCODE_DESKTOP_CONFIG=str(root / "desktop.toml"))
        for name in ("home", "runtime", "config", "cache", "data", "logs", "jcode"):
            (root / name).mkdir(mode=0o700)
        (root / "desktop.toml").write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n')
        (root / "openbox.xml").write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<keyboard/><applications><application class="*"><decor>no</decor>
<maximized>yes</maximized></application></applications></openbox_config>''')
        processes = []
        read_fd, write_fd = os.pipe()

        def spawn(name, command, **kwargs):
            log = stack.enter_context((output / (name + ".log")).open("w"))
            process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
            processes.append(process)
            return process

        def native(*values, check=True):
            return subprocess.run(["xdotool", *map(str, values)], env=env, cwd=root,
                                  text=True, capture_output=True, check=check, timeout=10)

        def windows():
            found = native("search", "--onlyvisible", "--pid", app.pid, check=False)
            return found.stdout.split() if found.returncode == 0 else []

        def wait(predicate, label):
            return STANDALONE.wait_for(predicate, label, processes, timeout=30)

        def focus(window):
            native("windowactivate", "--sync", window)
            assert native("getactivewindow").stdout.strip() == window
            time.sleep(.3)

        def capture(window, label):
            focus(window)
            native("mousemove", "--window", window, 1400, 850)
            time.sleep(.2)
            path = output / (label + ".png")
            subprocess.run(["import", "-window", window, "png:" + str(path)],
                           env=env, cwd=root, check=True, timeout=20)
            return path

        def words(path, footer=False):
            with Image.open(path) as image:
                bounds = (0, image.height - 90, image.width, image.height) if footer else (0, 0, image.width, image.height)
                crop = image.crop(bounds).convert("RGB")
                crop = crop.resize((crop.width * 3, crop.height * 3))
                crop_path = path.with_name(path.stem + ("-footer" if footer else "-ocr") + ".png")
                crop.save(crop_path)
            result = subprocess.run(["tesseract", str(crop_path), "stdout", "--psm", "11", "tsv"],
                                    env=env, capture_output=True, text=True, check=True, timeout=30)
            crop_path.with_suffix(".tsv").write_text(result.stdout)
            return parse_words(result.stdout, bounds)

        def click_phrase(window, label, phrase, footer=False):
            bounds = phrase_bounds(words(capture(window, label), footer), phrase)
            native("mousemove", "--window", window, round((bounds[0] + bounds[2]) / 2),
                   round((bounds[1] + bounds[3]) / 2), "click", 1)
            time.sleep(.5)

        def state():
            return wait(lambda: STANDALONE.navigation(root / "state"), "source navigation")

        def unchanged(label):
            assert app.poll() is None
            current = state()
            STANDALONE.assert_single(current)
            assert STANDALONE.invariant(current) == baseline, "source layout changed: " + label
            (output / (label + "-state.json")).write_text(json.dumps(current, indent=2))
            # Native focus/paint guarantees this is a fresh view of the original,
            # not only an unchanged state dump while another window is active.
            path = capture(source, label)
            phrase_bounds(words(path), "draft survives")
            return path

        try:
            spawn("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
            os.close(write_fd)
            write_fd = None
            assert select.select([read_fd], [], [], 15)[0], "Xvfb startup timed out"
            display = os.read(read_fd, 64).decode().strip()
            assert display.isdigit()
            env["DISPLAY"] = ":" + display
            spawn("openbox", ["openbox", "--sm-disable", "--config-file", str(root / "openbox.xml")])
            time.sleep(.5)
            app = spawn("desktop", [str(binary), "--no-hot-reload", "--single-panel"])
            ids = wait(windows, "standalone window")
            assert len(ids) == 1, ids
            source = ids[0]
            wait(lambda: STANDALONE.navigation(root / "state"), "initial state")
            time.sleep(2)
            focus(source)
            native("key", "--clearmodifiers", "Escape")
            native("type", "--clearmodifiers", "--delay", "30", "utility draft survives intact")
            time.sleep(.4)
            baseline = STANDALONE.invariant(state())
            unchanged("source-before")
            report["source_window"] = source
            # Model footer label is fixture-provided, not an arbitrary coordinate.
            footer_words = words(capture(source, "footer"), footer=True)
            footer_text = " ".join(word["text"] for word in footer_words)
            report["footer_text"] = footer_text
            model_label = next((label for label in ("openai:atlas-01", "Choose model", "claude-opus-4-6")
                                if label.lower() in footer_text.lower()), None)
            if model_label is None:
                raise AssertionError("fixture model footer label not recognized: " + footer_text)
            account_label = "api key" if "api key" in footer_text.lower() else "Accounts"
            for utility, footer_label, child_label in (("accounts", account_label, "Accounts"),):
                print("JCODE_CHECKPOINT " + json.dumps({"message": "Checking native " + utility + " child window"}), flush=True)
                click_phrase(source, utility + "-click", footer_label, footer=True)
                ids = wait(lambda: windows() if len(windows()) == 2 else None, utility + " child")
                child = next(window for window in ids if window != source)
                phrase_bounds(words(capture(child, utility + "-child")), child_label)
                focus(child)  # Accounts deliberately focuses its search field on open.
                native("type", "--clearmodifiers", "Gemini")
                time.sleep(.3)
                filtered = words(capture(child, "accounts-search-filtered"))
                phrase_bounds(filtered, "Google Gemini")
                phrase_bounds(filtered, "Gemini API")
                assert not any("openai" in word["text"].lower() for word in filtered), "account search did not filter unrelated providers"
                native("key", "--clearmodifiers", "ctrl+a", "BackSpace")
                time.sleep(.3)
                report["checks"].append("Accounts search filters native provider rows and clears without changing the source")
                unchanged(utility + "-source-open")
                click_phrase(source, utility + "-repeat", footer_label, footer=True)
                assert set(windows()) == {source, child}, "repeated click created duplicate utility"
                focus(child)
                native("key", "--clearmodifiers", "Escape")
                wait(lambda: windows() == [source], utility + " close")
                unchanged(utility + "-source-closed")
                report["checks"].append(utility + ": footer native click opens separate content, reuses child, Escape closes only child, source layout/draft unchanged")
            print("JCODE_CHECKPOINT " + json.dumps({"message": "Checking shared footer model picker"}), flush=True)
            click_phrase(source, "models-click", model_label, footer=True)
            assert windows() == [source], "model footer must use shared in-window picker"
            model_words = words(capture(source, "models-shared-picker"))
            phrase_bounds(model_words, "openai:atlas-02")
            assert STANDALONE.invariant(state()) == baseline, "model picker changed source layout"
            native("key", "--clearmodifiers", "Escape")
            time.sleep(.4)
            unchanged("models-source-closed")
            report["checks"].append("Model footer uses shared in-window picker, Escape restores existing draft and source layout")
            report["passed"] = True
        except BaseException as error:
            report["error"] = f"{type(error).__name__}: {error}"
            if env.get("DISPLAY"):
                subprocess.run(["import", "-window", "root", "png:" + str(output / "failure.png")],
                               env=env, cwd=root, timeout=20, check=False)
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
            if (root / "state").exists():
                shutil.copy2(root / "state", output / "final-state.txt")
            shutil.copytree(root / "logs", output / "app-logs", dirs_exist_ok=True)
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print("PASS: standalone utility windows. Evidence: " + str(output))


if __name__ == "__main__":
    main()
