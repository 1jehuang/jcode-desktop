#!/usr/bin/env python3
"""Exercise the real embedded terminal on a private Xvfb display, without building.

Usage: python3 scripts/terminal_acceptance.py target/terminal-acceptance
       python3 scripts/terminal_acceptance.py target/terminal-baseline --no-kitty

Requires a current prebuilt host (no plugin is loaded), Xvfb, Openbox, xdotool,
ImageMagick, Tesseract, Mesa lavapipe, and Pillow. Artifacts and diagnostics are
retained, including on failure. Never connects to the user's display, daemon,
preview endpoint, or instance socket. All keyboard input targets private Xvfb.
The fixture is a Python child launched by the actual terminal shell on its PTY,
not bytes injected into an emulator. --no-kitty explicitly omits image acceptance
and is useful for characterizing the old terminal before Handterm integration.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import select
import shlex
import shutil
import subprocess
import sys
import termios
import time
import tty

from screenshot import isolated_env


IMAGE_COLORS = ((23, 213, 199), (241, 55, 173))
HISTORY_TEXT_COLOR = (255, 180, 40)


def kitty_packet():
    """Small direct RGB image with an explicit placement, without external files."""
    pixels = bytes(channel for y in range(24) for x in range(24)
                   for channel in IMAGE_COLORS[(x // 12 + y // 12) % 2])
    payload = base64.b64encode(pixels)
    return (b"\x1b[11;4H\x1b_Ga=T,f=24,t=d,s=24,v=24,i=4242,c=12,r=6,q=2;"
            + payload + b"\x1b\\\x1b[19;1H")


def fixture_worker(root):
    """Run inside the app's real PTY. File receipts are independent of OCR."""
    def emit(value):
        sys.stdout.buffer.write(value)
        sys.stdout.buffer.flush()

    proof = {"stdin_tty": os.isatty(0), "stdout_tty": os.isatty(1),
             "tty": os.ttyname(0), "pid": os.getpid(), "keys": []}
    if not proof["stdin_tty"] or not proof["stdout_tty"]:
        raise RuntimeError("fixture must run through the desktop's real PTY")
    original = termios.tcgetattr(0)
    try:
        tty.setraw(0)
        # Query through the real shell PTY, not a mocked engine. Clients use
        # these responses to decide whether to render light or dark text.
        emit(b"\x1b]10;?\x07\x1b]11;?\x1b\\")
        replies = b""
        deadline = time.monotonic() + 3
        colors = {}
        while len(colors) < 2 and time.monotonic() < deadline:
            if not select.select([0], [], [], .1)[0]:
                continue
            replies += os.read(0, 4096)
            for match in re.finditer(rb"\x1b\](10|11);rgb:([0-9a-fA-F]+)/([0-9a-fA-F]+)/([0-9a-fA-F]+)(?:\x07|\x1b\\)", replies):
                colors[match[1].decode()] = [
                    round(int(channel, 16) * 255 / (16 ** len(channel) - 1))
                    for channel in match.groups()[1:]
                ]
        if len(colors) != 2:
            raise RuntimeError(f"Missing terminal color replies: {replies!r}")
        proof["colors"] = colors
        accent = (b"\x1b[38;2;20;90;30m" if sum(colors["11"]) > 384
                  else b"\x1b[38;2;255;210;40m")
        # Absolute cursor addressing, truecolor, erase-in-line, and overwrite.
        emit(b"\x1b[?25l\x1b[2J\x1b[H"
             b"TERMINAL ACCEPTANCE\r\n"
             + accent + b"ANSI COLOR\x1b[0m\r\n"
             b"\x1b[4;1HCURSOR POSITION OK"
             b"\x1b[5;1HERASE WRONG CONTENT\x1b[5;1H\x1b[2KERASE OK"
             b"\x1b[6;1HPTY REAL\x1b[8;1HINPUT READY\x1b[19;1H")
        while True:
            size = os.get_terminal_size(0)
            proof["rows"], proof["cols"] = size.lines, size.columns
            temporary = root / "pty-proof.tmp"
            temporary.write_text(json.dumps(proof))
            temporary.replace(root / "pty-proof.json")
            key = os.read(0, 1)
            if not key or key == b"q":
                break
            proof["keys"].append(key.decode("ascii", errors="replace"))
            if key == b"k":
                emit(b"\x1b[7;1HKEYBOARD OK\x1b[19;1H")
            elif key == b"a":
                emit(b"\x1b[?1049h\x1b[2J\x1b[HALTERNATE SCREEN OK")
            elif key == b"b":
                emit(b"\x1b[?1049l")
            elif key == b"i":
                emit(kitty_packet())
            elif key == b"h":
                # This runs after all layout/debug checks. Never resize once
                # these anchored text and image rows enter scrollback.
                emit(b"\x1b[2J\x1b[H" + kitty_packet()
                     + b"\x1b[11;20H\x1b[38;2;255;180;40mHISTORY ANCHOR\x1b[0m")
            elif key == b"s":
                emit(f"\x1b[{size.lines};1H".encode())
                for line in range(size.lines + 12):
                    emit(f"\r\nHISTORY FILL {line:04d}".encode())
            elif key == b"d":
                emit(b"\x1b_Ga=d,d=I,i=4242,q=2\x1b\\")
    finally:
        emit(b"\x1b[?1049l\x1b[?25h\x1b[0m\r\n")
        termios.tcsetattr(0, termios.TCSANOW, original)


class Harness:
    def __init__(self, root, env):
        self.root, self.env = root, env
        self.app = None
        self.report = {"scope": "real shell PTY, offline app, private Xvfb, native input",
                       "checks": {}}

    def wait(self, predicate, description, timeout=20):
        deadline = time.monotonic() + timeout
        while True:
            if self.app and self.app.poll() is not None:
                raise RuntimeError(f"Isolated app exited: {self.app.returncode}")
            result = predicate()
            if result:
                return result
            if time.monotonic() >= deadline:
                raise TimeoutError(description)
            time.sleep(.1)

    def native(self, *args):
        subprocess.run(["xdotool", *map(str, args)], env=self.env, cwd=self.root,
                       check=True, timeout=15)

    def navigation(self):
        path = self.root / "state"
        try:
            for line in path.read_text().splitlines():
                if line.startswith("navigation="):
                    return json.loads(line.split("=", 1)[1])
        except (FileNotFoundError, json.JSONDecodeError):
            # Workspace diagnostics are opt-in but written in place per frame.
            # A partial read is a retry, not a terminal or acceptance failure.
            pass
        return {}

    def proof(self):
        path = self.root / "pty-proof.json"
        return json.loads(path.read_text()) if path.exists() else {}

    def key(self, key):
        count = len(self.proof().get("keys", []))
        self.native("key", "--clearmodifiers", key)
        self.wait(lambda: len(self.proof().get("keys", [])) > count,
                  f"Native {key!r} did not reach the real PTY")

    def capture(self, label):
        time.sleep(.4)
        path = self.root / f"{label}.png"
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=self.env, check=True, timeout=15)
        return path

    def text(self, path):
        from PIL import Image
        ocr_path = path.with_suffix(".ocr.png")
        with Image.open(path) as image:
            image.resize((image.width * 3, image.height * 3)).save(ocr_path)
        result = subprocess.run(["tesseract", str(ocr_path), "stdout", "--psm", "11"],
                                env=self.env, capture_output=True, text=True,
                                check=True, timeout=20)
        path.with_suffix(".txt").write_text(result.stdout)
        # This fixture contains no digit markers. Tesseract occasionally reads
        # the narrow monospace O as 0 even at 3x, especially in the word OK.
        return " ".join(result.stdout.upper().replace("0", "O").split())

    def expect_text(self, label, phrases, absent=()):
        # OCR samples the actual rendered surface, not shell echo or a stub grid.
        path = self.capture(label)
        text = self.text(path)
        for phrase in phrases:
            if phrase not in text:
                raise AssertionError(f"{label}: missing {phrase!r}: {text}")
        for phrase in absent:
            if phrase in text:
                raise AssertionError(f"{label}: unexpectedly painted {phrase!r}: {text}")
        self.report["checks"][label] = {"screenshot": path.name, "phrases": phrases}
        return path


def color_counts(path):
    from PIL import Image
    with Image.open(path) as image:
        histogram = image.convert("RGB").getcolors(image.width * image.height)
    return [sum(count for count, pixel in histogram
                if all(abs(pixel[i] - color[i]) <= 3 for i in range(3)))
            for color in IMAGE_COLORS]


def pixel_region(path, colors=IMAGE_COLORS):
    """Return exact-color counts and a half-open bounding box of their union."""
    from PIL import Image, ImageChops
    counts = []
    with Image.open(path) as image:
        channels = image.convert("RGB").split()
        union = Image.new("L", image.size)
        for color in colors:
            masks = [channel.point([255 if abs(value - target) <= 3 else 0
                                    for value in range(256)])
                     for channel, target in zip(channels, color)]
            mask = ImageChops.multiply(ImageChops.multiply(masks[0], masks[1]), masks[2])
            counts.append(mask.histogram()[255])
            union = ImageChops.lighter(union, mask)
        bbox = union.getbbox()
    return {"counts": counts, "bbox": list(bbox) if bbox else None}


def verify_image_history(h):
    from PIL import Image

    def sample(label):
        path = h.capture(label)
        result = {"screenshot": path.name, **pixel_region(path),
                  "anchor": pixel_region(path, (HISTORY_TEXT_COLOR,))}
        # Write evidence before assertions, including failing screenshots.
        h.report["checks"][label] = result
        return path, result

    def wheel(button):
        h.native("mousemove", "--sync", mouse_x, mouse_y)
        h.native("click", button)

    def dimensions(box):
        return box[2] - box[0], box[3] - box[1]

    h.key("h")
    original_path, original = sample("history-original")
    box = original["bbox"]
    anchor = original["anchor"]["bbox"]
    if not box or not anchor or min(original["counts"]) < 100:
        raise AssertionError(f"History fixture image/anchor missing: {original}")
    if "HISTORY ANCHOR" not in h.text(original_path):
        raise AssertionError("History fixture text marker not readable")
    mouse_x, mouse_y = (box[0] + box[2]) // 2, (box[1] + box[3]) // 2
    h.key("s")
    _, live = sample("history-live-hidden")
    if sum(live["counts"]) or live["anchor"]["bbox"]:
        raise AssertionError(f"Image and anchor did not wholly enter history: {live}")
    up_steps = 0
    revealed = None
    # One native wheel notch per frame avoids assuming engine wheel line size.
    for step in range(h.proof()["rows"] * 2):
        wheel(4)
        up_steps += 1
        path, state = sample(f"history-up-{step:03d}")
        if state["bbox"] and dimensions(state["bbox"]) == dimensions(box):
            revealed = state
            delta = state["bbox"][1] - box[1]
            expected_anchor = [anchor[0], anchor[1] + delta, anchor[2], anchor[3] + delta]
            if state["anchor"]["bbox"] != expected_anchor:
                raise AssertionError(f"History image detached from text: {state}, expected {expected_anchor}")
            if "HISTORY ANCHOR" not in h.text(path):
                raise AssertionError("Revealed image lacks correct nearby history text")
            if state["counts"] != original["counts"]:
                raise AssertionError(f"Revealed image pixels changed: {state}")
            break
        # The text marker is at the image top. If it becomes visible without
        # an image, the old engine has discarded that image on scroll-out.
        if state["anchor"]["bbox"] and not state["bbox"]:
            h.text(path)
            raise AssertionError("Persistent image scrollback missing: native wheel revealed anchored text but no image pixels")
    if revealed is None:
        raise AssertionError("Native wheel never revealed the complete history image")

    clipped = False
    for step in range(up_steps + 1):
        wheel(5)
        path, state = sample(f"history-down-{step:03d}")
        current = state["bbox"]
        if current and 0 < dimensions(current)[1] < dimensions(box)[1]:
            clipped = True
            if dimensions(current)[0] != dimensions(box)[0] or min(state["counts"]) < 20:
                raise AssertionError(f"Clipped image lost width/colors: {state}")
            # The top is clipped as we scroll toward live output. Compare the
            # actual remaining checkerboard with the original bottom slice,
            # not a rescaled image which can preserve overall color counts.
            height = dimensions(current)[1]
            with Image.open(original_path) as before, Image.open(path) as after:
                expected = before.convert("RGB").crop((box[0], box[3] - height, box[2], box[3]))
                actual = after.convert("RGB").crop(tuple(current))
                if actual.tobytes() != expected.tobytes():
                    raise AssertionError("Partially clipped history image stretched or changed pixels")
        if not current:
            break
    else:
        raise AssertionError("Wheel down did not hide history image")
    if not clipped:
        raise AssertionError("Native wheel did not exercise partial image clipping")
    # Return to live bottom without any geometry changes, delete while the
    # placement is offscreen, and revisit the exact same text history.
    for _ in range(up_steps + 2):
        wheel(5)
    h.key("d")
    _, deleted = sample("history-deleted-live")
    if sum(deleted["counts"]):
        raise AssertionError("Deleted offscreen image remains live")
    for _ in range(up_steps):
        wheel(4)
    path, deleted = sample("history-deleted-revisited")
    if sum(deleted["counts"]):
        raise AssertionError("Deleted image resurrected from scrollback")
    if deleted["anchor"]["bbox"] != revealed["anchor"]["bbox"] or "HISTORY ANCHOR" not in h.text(path):
        raise AssertionError("Deletion check did not revisit the same anchored text history")
    h.report["checks"]["image_history"] = {"native_wheel_up_steps": up_steps,
                                             "partial_clip_verified": clipped,
                                             "delete_no_resurrection": True}


def verify(harness, kitty, require_debug_state=False):
    h = harness
    h.wait(lambda: h.navigation().get("rows"), "Fixture did not render", timeout=45)
    time.sleep(1)
    h.native("key", "--clearmodifiers", "Escape")
    h.native("key", "--clearmodifiers", "super+t")

    def focused_terminal():
        nav = h.navigation()
        return next((panel for row in nav.get("rows", []) for panel in row["panels"]
                     if panel.get("session") == "terminal" and panel.get("focused")), None)

    terminal = h.wait(focused_terminal, "Super+T did not create and focus terminal")
    h.report["checks"]["terminal_open"] = terminal
    h.native("key", "--clearmodifiers", "super+f")
    time.sleep(.5)
    # Copy only this script and screenshot's import helper into the sandbox. The
    # shell never reads credentials, user rc files, or files from the checkout.
    command = "python3 " + shlex.quote(str(h.root / "terminal_acceptance.py"))
    command += " --fixture-worker " + shlex.quote(str(h.root))
    h.native("type", "--clearmodifiers", "--delay", "1", "--", command)
    h.native("key", "--clearmodifiers", "Return")
    proof = h.wait(lambda: h.proof(), "Shell failed to launch the PTY fixture")
    if not proof["stdin_tty"] or not proof["stdout_tty"] or proof["rows"] < 20 or proof["cols"] < 40:
        raise AssertionError(f"Not a usable real terminal PTY: {proof}")
    base = h.capture("background-query")
    from PIL import Image
    background = tuple(proof["colors"]["11"])
    with Image.open(base) as image:
        histogram = image.convert("RGB").getcolors(image.width * image.height)
    matching = sum(count for count, pixel in histogram if pixel == background)
    if matching < 10000:
        raise AssertionError(f"OSC 11 reports {background}, but only {matching} pixels match")
    h.report["checks"]["background_query"] = {"rgb": background, "painted_pixels": matching}
    base = h.expect_text("ansi", ["TERMINAL ACCEPTANCE", "ANSI COLOR", "CURSOR POSITION OK",
                                "ERASE OK", "PTY REAL", "INPUT READY"], ["WRONG CONTENT"])
    h.key("k")
    h.expect_text("keyboard", ["KEYBOARD OK", "TERMINAL ACCEPTANCE"])
    h.key("a")
    h.expect_text("alternate", ["ALTERNATE SCREEN OK"], ["TERMINAL ACCEPTANCE"])
    h.key("b")
    h.expect_text("restored", ["TERMINAL ACCEPTANCE", "KEYBOARD OK"], ["ALTERNATE SCREEN OK"])
    if kitty:
        before = color_counts(base)
        h.key("i")
        image = h.capture("kitty-image")
        after = color_counts(image)
        if any(new - old < 100 for old, new in zip(before, after)):
            raise AssertionError(f"Kitty image not painted: baseline={before}, image={after}")
        h.report["checks"]["kitty_image"] = {"before": before, "after": after,
                                               "screenshot": image.name}
        h.key("d")
        deleted = h.capture("kitty-deleted")
        remaining = color_counts(deleted)
        if any(new > old + 20 for old, new in zip(before, remaining)):
            raise AssertionError(f"Deleted Kitty image remains painted: {remaining}")
        h.report["checks"]["kitty_delete"] = {"remaining": remaining, "screenshot": deleted.name}
    else:
        h.report["checks"]["kitty_image"] = {"skipped": "explicit --no-kitty baseline"}
    if require_debug_state:
        # Layout actions invalidate workspace diagnostics even if only the
        # terminal's cached subtree was repainting during PTY output.
        h.native("key", "--clearmodifiers", "super+f")
        time.sleep(.3)
        h.native("key", "--clearmodifiers", "super+f")

        def ready_snapshot():
            panel = focused_terminal()
            snapshot = panel.get("terminal") if panel else None
            if snapshot and "KEYBOARD OK" in snapshot.get("contents", ""):
                return snapshot
            return None

        snapshot = h.wait(ready_snapshot, "Missing current terminal debug_snapshot in navigation")
        if (not snapshot.get("resource_id") or snapshot.get("rows", 0) < 20
                or snapshot.get("cols", 0) < 40 or snapshot.get("image_count") != 0):
            raise AssertionError(f"Unexpected terminal state after image deletion: {snapshot}")
        h.report["checks"]["debug_state"] = snapshot
        h.expect_text("resized-restored", ["TERMINAL ACCEPTANCE", "KEYBOARD OK"])
    if kitty:
        verify_image_history(h)
    h.report["pty"] = h.proof()
    h.report["navigation"] = h.navigation()
    h.native("key", "--clearmodifiers", "q")


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--fixture-worker":
        fixture_worker(Path(sys.argv[2]))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new directory for persistent artifacts")
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/jcode-desktop")
    parser.add_argument("--no-kitty", action="store_true", help="explicitly skip image acceptance for the old engine")
    parser.add_argument("--theme", choices=("warm-neutral", "neutral-light"), default="warm-neutral")
    parser.add_argument("--require-debug-state", action="store_true",
                        help="require the Handterm terminal snapshot in opt-in navigation diagnostics")
    args = parser.parse_args()
    for command in ("Xvfb", "openbox", "xdotool", "import", "tesseract", "python3"):
        if not shutil.which(command):
            parser.error("missing prerequisite: " + command)
    try:
        import PIL.Image  # noqa: F401
    except ImportError:
        parser.error("Pillow is required for actual screenshot pixel assertions")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe is required")
    binary = args.binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    env = isolated_env(root)
    for directory in ("home", "runtime", "config", "cache", "data", "jcode"):
        (root / directory).mkdir(mode=0o700)
    env.update({"VK_DRIVER_FILES": str(drivers[0]), "SHELL": "/bin/bash",
                "JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "empty"})
    config = root / "desktop.toml"
    config.write_text(f'[appearance]\ntheme = "{args.theme}"\nlayout_mode = "folder_tabs"\n')
    env["JCODE_DESKTOP_CONFIG"] = str(config)
    shutil.copyfile(__file__, root / "terminal_acceptance.py")
    shutil.copyfile(Path(__file__).with_name("screenshot.py"), root / "screenshot.py")
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                         '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                         '</application></applications></openbox_config>')
    harness = Harness(root, env)
    processes = []
    try:
        with (root / "xvfb.log").open("w") as xlog, (root / "app.log").open("w") as log:
            read_fd, write_fd = os.pipe()
            try:
                xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                                         "1440x1000x24", "-nolisten", "tcp"],
                                        pass_fds=(write_fd,), env=env, stdout=xlog, stderr=xlog)
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
                raise RuntimeError("Private Openbox failed to start")
            harness.app = subprocess.Popen([str(binary)], env=env, cwd=root, stdout=log, stderr=log)
            processes.append(harness.app)
            verify(harness, kitty=not args.no_kitty, require_debug_state=args.require_debug_state)
        harness.report["ok"] = True
    except Exception as error:
        harness.report.update({"ok": False, "error": str(error)})
        if "DISPLAY" in env and harness.app and harness.app.poll() is None:
            try:
                harness.capture("failure")
            except Exception:
                pass
        raise
    finally:
        (root / "report.json").write_text(json.dumps(harness.report, indent=2) + "\n")
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        print(f"Terminal acceptance artifacts: {root}", flush=True)
    print(json.dumps(harness.report, indent=2))


if __name__ == "__main__":
    main()
