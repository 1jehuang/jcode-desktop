#!/usr/bin/env python3
"""Render the real app on a private Xvfb display using offline fixture data."""
import argparse
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
    parser.add_argument("--size", default="1440x1000")
    parser.add_argument("--panels", type=int, choices=range(1, 7), default=1,
                        help="show a connected folder group with its middle panel focused")
    parser.add_argument("--focus-panel", type=int,
                        help="click this zero-based panel through X11 before capture")
    args = parser.parse_args()
    if args.focus_panel is not None and not 0 <= args.focus_panel < args.panels:
        parser.error("focus-panel must identify one of the displayed panels")
    if args.focus_panel is not None and not shutil.which("xdotool"):
        parser.error("native focus verification requires xdotool")
    width, height = (int(n) for n in args.size.split("x"))
    if not (640 <= width <= 7680 and 480 <= height <= 4320):
        parser.error("size must be between 640x480 and 7680x4320")
    if args.focus_panel is not None and (width - 264) / args.panels < 320:
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
        env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = str(args.panels)
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
                if args.focus_panel is not None:
                    # The fixture always shows the 264px sidebar. Native X11
                    # input crosses the same platform -> GPUI -> workspace path
                    # as a user click, on this private display only.
                    x = round(264 + (width - 264) * (args.focus_panel + 0.5) / args.panels)
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
