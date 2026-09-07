#!/usr/bin/env python3
"""Exercise real /update in an isolated managed Linux install on private Xvfb.

The release channel is live HTTPS. Only the temporary HOME may be modified.
Pass the channel's current version to verify its already-current path.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import time

from screenshot import isolated_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=Path("target/release/jcode-desktop"))
    parser.add_argument("--version", default="0.1.0-beta.24")
    parser.add_argument("--expect-version", help="verify a real download/stage to this newer version instead of the no-op path")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    if not args.version or any(c not in "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-" for c in args.version):
        parser.error("invalid version directory")
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool):
            parser.error(f"missing {tool}")
    env = isolated_env(root)
    for name in ("home", "runtime", "config", "cache", "data", "jcode"):
        (root / name).mkdir(mode=0o700)
    env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = "empty"
    env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = "1"
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe is required")
    env["VK_DRIVER_FILES"] = str(drivers[0])
    install = root / "home/.local/opt/jcode-desktop" / args.version
    install.mkdir(parents=True)
    binary = install / "jcode-desktop"
    shutil.copy2(args.binary.resolve(strict=True), binary)
    launcher = root / "home/.local/bin/jcode-desktop"
    launcher.parent.mkdir(parents=True)
    launcher.symlink_to(binary)
    before = os.readlink(launcher)
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
    processes = []
    logs = []
    read_fd, write_fd = os.pipe()

    def launch(name, command, **kwargs):
        log = (root / f"{name}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process

    def run(command):
        return subprocess.check_output(command, env=env, text=True, stderr=subprocess.DEVNULL, timeout=15)

    try:
        launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = -1
        if not select.select([read_fd], [], [], 15)[0]:
            raise RuntimeError("Xvfb did not choose a private display")
        env["DISPLAY"] = ":" + os.read(read_fd, 64).decode().strip()
        launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm_config)])
        time.sleep(.5)
        app = launch("desktop", [str(launcher), "--no-hot-reload"])
        deadline = time.monotonic() + 45
        while not (root / "state").exists():
            if app.poll() is not None or time.monotonic() >= deadline:
                raise AssertionError(f"Desktop did not render. See {root / 'logs/jcode-desktop/jcode-desktop.log'}")
            time.sleep(.1)
        run(["xdotool", "search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync"])
        time.sleep(1)
        run(["xdotool", "key", "--clearmodifiers", "ctrl+a"])
        run(["xdotool", "type", "--clearmodifiers", "--delay", "30", "/update"])
        run(["xdotool", "key", "--clearmodifiers", "Return"])
        deadline = time.monotonic() + 360
        image = root / "result.png"
        while time.monotonic() < deadline:
            if app.poll() is not None:
                raise AssertionError("Desktop exited during the update")
            run(["import", "-window", "root", "png:" + str(image)])
            text = run(["tesseract", str(image), "stdout"])
            (root / "result.txt").write_text(text)
            complete = (
                "installed" in text.lower() and "quit and reopen" in text.lower()
                if args.expect_version else "already up to date" in text.lower()
            )
            if complete:
                if args.expect_version:
                    expected = install.parent / args.expect_version / "jcode-desktop"
                    assert launcher.resolve() == expected, "update did not select expected release"
                    with expected.open("rb") as executable:
                        assert executable.read(4) == b"\x7fELF", "staged desktop is not an ELF executable"
                    assert binary.is_file(), "previous desktop was not retained"
                    for name in ("jcode", "jcode-harness-api-bridge", "jcode.desktop", "jcode.png"):
                        assert (expected.parent / name).is_file(), f"missing bundle file {name}"
                else:
                    assert os.readlink(launcher) == before, "no-update check changed the launcher"
                (root / "evidence.json").write_text(json.dumps({
                    "result": "new version installed" if args.expect_version else "already current",
                    "installed_version": args.expect_version,
                    "version": args.version,
                    "native_slash_command": True,
                    "public_https_metadata": True,
                    "launcher_unchanged": os.readlink(launcher) == before,
                    "previous_version_retained": binary.is_file(),
                    "desktop_still_running": True,
                    "private_display": env["DISPLAY"],
                }, indent=2) + "\n")
                print(f"PASS: native /update completed against public releases and kept the desktop process alive. Evidence: {root}")
                return
            if "update failed:" in text.lower():
                raise AssertionError(f"Desktop reported update error: {text}")
            time.sleep(.5)
        raise AssertionError(f"No up-to-date result. See {root / 'result.txt'}")
    finally:
        os.close(read_fd)
        if write_fd >= 0:
            os.close(write_fd)
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
        for log in logs:
            log.close()


if __name__ == "__main__":
    main()
