#!/usr/bin/env python3
"""Render the real app on a private Xvfb display using offline fixture data."""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time


def isolated_env(root):
    """Allowlist, never inherit desktop sockets, credentials, or app settings."""
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": str(root / "home"),
        "XDG_RUNTIME_DIR": str(root / "runtime"),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "data"),
        "XDG_STATE_HOME": str(root / "logs"),
        "JCODE_HOME": str(root / "jcode"),
        "JCODE_DESKTOP_SCREENSHOT": "1",
        "JCODE_DESKTOP_STATE": str(root / "state"),
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(root / "no-dbus"),
        "LIBGL_ALWAYS_SOFTWARE": "1",
        "LANG": "C.UTF-8",
    }


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--fresh-interact", action="store_true",
                        help="measure fresh composer pixels and verify native typing and submission")
    parser.add_argument("--html-interact", action="store_true",
                        help="exercise native input and controls on the HTML fixture")
    parser.add_argument("--history-interact", action="store_true",
                        help="verify native history clicks open and focus the intended composer")
    parser.add_argument("--transcript", choices=("all", "empty", "reasoning", "streaming", "html"), default="all",
                        help="choose the isolated transcript fixture")
    parser.add_argument("--size", default="1440x1000")
    parser.add_argument("--learn-stage", type=int, choices=(1, 2, 3),
                        help="show the staged tutorial in the top-left Learn tab")
    parser.add_argument("--panels", type=int, choices=range(1, 7), default=1,
                        help="show a connected folder group with its middle panel focused")
    parser.add_argument("--focus-panel", type=int,
                        help="click this zero-based panel through X11 before capture")
    parser.add_argument("--layout-mode", choices=("normal", "folder_tabs"), default="folder_tabs")
    parser.add_argument("--theme", default="warm-neutral", choices=(
        "warm-neutral", "warm-studio", "neutral-dark", "neutral-light",
        "midnight", "ocean", "forest", "plum", "rose-dawn", "parchment",
    ), help="render a built-in palette with isolated settings")
    args = parser.parse_args()
    if args.fresh_interact:
        incompatible = (
            args.transcript != "empty" or args.panels != 1
            or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
            or args.learn_stage is not None or args.focus_panel is not None
            or args.html_interact or getattr(args, "image_interact", False)
            or args.history_interact
        )
        if incompatible:
            parser.error("fresh-interact requires an empty transcript, one panel, warm-neutral folder tabs, and no other interaction mode")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("fresh-interact requires xdotool and tesseract")
    if args.history_interact and (args.panels != 1 or args.size != "1440x1000" or args.learn_stage is not None or args.focus_panel is not None or args.html_interact):
        parser.error("history-interact requires default size, one panel, and no other interaction mode")
    if args.history_interact and not shutil.which("xdotool"):
        parser.error("history-interact requires xdotool")
    if args.html_interact and (args.transcript != "html" or args.size != "1440x1000" or args.theme != "warm-neutral" or args.panels != 1):
        parser.error("html-interact requires the html transcript, default size/theme, and one panel")
    if args.html_interact and not shutil.which("xdotool"):
        parser.error("html-interact requires xdotool")
    if args.focus_panel is not None and not 0 <= args.focus_panel < args.panels:
        parser.error("focus-panel must identify one of the displayed panels")
    if args.focus_panel is not None and not shutil.which("xdotool"):
        parser.error("native focus verification requires xdotool")
    width, height = (int(n) for n in args.size.split("x"))
    if not (640 <= width <= 7680 and 480 <= height <= 4320):
        parser.error("size must be between 640x480 and 7680x4320")
    canvas_left = 276 if args.layout_mode == "folder_tabs" else 264
    canvas_insets = 288 if args.layout_mode == "folder_tabs" else 264
    if args.focus_panel is not None and (width - canvas_insets) / args.panels < 320:
        parser.error("native focus verification needs at least 320px per panel beside the sidebar")
    for tool in ("Xvfb", "import", "openbox"):
        if not shutil.which(tool):
            parser.error(f"missing {tool}: install Xvfb, ImageMagick, and Openbox")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe: install vulkan-swrast (Arch) or mesa-vulkan-drivers (Debian/Ubuntu)")
    if not args.no_build:
        subprocess.run(["cargo", "build", "-p", "jcode-desktop"], cwd=repo, check=True)
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        parser.error(f"refusing to overwrite {output}")
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", repo / "target"))
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="screenshot-", dir=scratch) as temporary:
        root = Path(temporary)
        env = isolated_env(root)
        config = root / "desktop.toml"
        config.write_text(f'[appearance]\nlayout_mode = "{args.layout_mode}"\ntheme = "{args.theme}"\n')
        env["JCODE_DESKTOP_CONFIG"] = str(config)
        env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = args.transcript
        if args.learn_stage is not None:
            env["JCODE_DESKTOP_SCREENSHOT_LEARN_STAGE"] = str(args.learn_stage)
        env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = str(args.panels)
        if args.history_interact:
            env["JCODE_DESKTOP_SCREENSHOT_HISTORY"] = "1"
        env["VK_DRIVER_FILES"] = str(drivers[0])
        wm_config = root / "openbox.xml"
        wm_config.write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<applications><application class="*"><decor>no</decor>
<maximized>yes</maximized></application></applications></openbox_config>''')
        for name in ("home", "runtime", "config", "cache", "data", "jcode"):
            (root / name).mkdir(mode=0o700)
        processes = []
        read_fd, write_fd = os.pipe()
        try:
            with (root / "xvfb.log").open("w+") as xvfb_log, (root / "app.log").open("w+") as app_log:
                xvfb = subprocess.Popen(
                    ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", f"{width}x{height}x24", "-nolisten", "tcp"],
                    pass_fds=(write_fd,), env=env, cwd=root, stdout=xvfb_log, stderr=xvfb_log,
                )
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise RuntimeError("Xvfb did not become ready")
                display = os.read(read_fd, 64).decode().strip()
                if not display.isdigit():
                    raise RuntimeError("Xvfb failed to allocate a display")
                env["DISPLAY"] = ":" + display
                wm = subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(wm_config)], env=env, cwd=root,
                                      stdout=xvfb_log, stderr=xvfb_log)
                processes.append(wm)
                time.sleep(0.5)
                if wm.poll() is not None:
                    raise RuntimeError("Private Openbox failed to start")
                app = subprocess.Popen([str(binary)], env=env, cwd=root, stdout=app_log, stderr=app_log)
                processes.append(app)
                deadline = time.monotonic() + 45
                state = root / "state"
                expected_widths = "widths=" + ",".join([f"{1 / args.panels:.2f}"] * args.panels)
                while not state.exists() or expected_widths not in state.read_text():
                    if app.poll() is not None or time.monotonic() > deadline:
                        app_log.seek(0)
                        diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
                        detail = diagnostics.read_text() if diagnostics.exists() else ""
                        raise RuntimeError("App failed to render:\n" + app_log.read() + detail)
                    time.sleep(0.1)
                # Allow opening animation and font rasterization to settle.
                time.sleep(2)
                if args.history_interact:
                    # These are visible title coordinates in the fixed-size
                    # offline fixture. Never send input to the user's display.
                    for y, session_id in [(488, "screenshot-history-06"), (105, "screenshot-fixture"), (153, "screenshot-history-06")]:
                        subprocess.run(["xdotool", "mousemove", "100", str(y), "click", "1"],
                                       env=env, cwd=root, check=True, timeout=10)
                        deadline = time.monotonic() + 10
                        while True:
                            text = state.read_text()
                            lines = [line for line in text.splitlines() if line.startswith("navigation=")]
                            navigation = json.loads(lines[0].split("=", 1)[1]) if lines else {}
                            focused = [panel for row in navigation.get("rows", []) for panel in row["panels"] if panel["focused"]]
                            if focused and focused[0]["session"] == session_id and navigation.get("keyboard_panel") == focused[0]["slot"]:
                                break
                            if app.poll() is not None or time.monotonic() > deadline:
                                raise RuntimeError("History click lost session or keyboard focus: " + text)
                            time.sleep(.05)
                    subprocess.run(["xdotool", "type", "history click typing works"],
                                   env=env, cwd=root, check=True, timeout=10)
                    time.sleep(.5)
                if args.focus_panel is not None:
                    # The fixture shows a 264px sidebar and 12px page connector.
                    # Native X11
                    # input crosses the same platform -> GPUI -> workspace path
                    # as a user click, on this private display only.
                    x = round(canvas_left + (width - canvas_insets) * (args.focus_panel + 0.5) / args.panels)
                    subprocess.run(["xdotool", "mousemove", str(x), str(height // 2), "click", "1"],
                                   env=env, cwd=root, check=True, timeout=10)
                    deadline = time.monotonic() + 10
                    while f"focus={args.focus_panel} " not in state.read_text():
                        if app.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("Native panel click did not update public focus state: " + state.read_text())
                        time.sleep(0.05)
                    time.sleep(0.5)
                if app.poll() is not None:
                    raise RuntimeError("App exited before capture")
                subprocess.run(["import", "-window", "root", "png:" + str(output)], env=env, cwd=root, check=True, timeout=15)
                print(f"Screenshot: {output}\nFixture state: {state.read_text().strip()}")
                if args.fresh_interact:
                    from fresh_session_acceptance import verify
                    verify(output, env, root)
                if args.html_interact:
                    from html_preview_acceptance import verify
                    try:
                        verify(output, env, root)
                    except Exception:
                        diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
                        if diagnostics.exists():
                            print(diagnostics.read_text())
                        raise
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
                        process.wait()


if __name__ == "__main__":
    main()
