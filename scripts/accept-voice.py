#!/usr/bin/env python3
"""Exercise production --toggle-voice IPC on a private offline Xvfb host.

Never uses a compositor, live desktop socket, credentials, or microphone.
Requires an already-built host, Xvfb, Openbox, xdotool, ImageMagick and tesseract.
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
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new evidence directory, never overwritten")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool):
            parser.error(f"missing {tool}")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe is required")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    for name in ("home", "runtime", "config", "cache", "data", "jcode", "logs"):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.update({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "empty",
                "JCODE_DESKTOP_SCREENSHOT_PANELS": "1",
                "VK_DRIVER_FILES": str(drivers[0])})
    config = root / "desktop.toml"
    config.write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n')
    env["JCODE_DESKTOP_CONFIG"] = str(config)
    wm_config = root / "openbox.xml"
    wm_config.write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<applications><application class="*"><decor>no</decor><maximized>yes</maximized>
</application></applications></openbox_config>''')

    def run(*command, check=True):
        return subprocess.run(command, env=env, cwd=root, text=True,
                              capture_output=True, check=check, timeout=20)

    missing = run(str(binary), "--toggle-voice", check=False)
    assert missing.returncode != 0, "forwarding without a host must fail"
    assert not (root / "runtime/jcode-desktop.sock").exists(), "CLI launched a host"
    assert not (root / "state").exists(), "CLI launched a UI"
    (root / "missing-host.txt").write_text(missing.stderr)

    processes = []
    read_fd, write_fd = os.pipe()
    try:
        with (root / "native.log").open("w+") as log:
            xvfb = subprocess.Popen(
                ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"],
                pass_fds=(write_fd,), env=env, cwd=root, stdout=log, stderr=log)
            processes.append(xvfb)
            os.close(write_fd)
            write_fd = None
            if not select.select([read_fd], [], [], 15)[0]:
                raise RuntimeError("Xvfb did not become ready")
            display = os.read(read_fd, 64).decode().strip()
            assert display.isdigit(), "Xvfb failed to allocate a private display"
            env["DISPLAY"] = ":" + display
            wm = subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(wm_config)],
                                  env=env, cwd=root, stdout=log, stderr=log)
            processes.append(wm)
            time.sleep(0.5)
            assert wm.poll() is None, "private Openbox failed"
            app = subprocess.Popen([str(binary), "--no-hot-reload"], env=env, cwd=root,
                                   stdout=log, stderr=log)
            processes.append(app)
            deadline = time.monotonic() + 60
            while not (root / "state").exists():
                if app.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("host did not render; see native.log")
                time.sleep(0.1)
            time.sleep(2)
            run("xdotool", "key", "--clearmodifiers", "Escape")
            time.sleep(0.3)
            run("xdotool", "type", "--clearmodifiers", "Draft preserved for review")
            time.sleep(0.3)

            def image_text(name):
                image = root / f"{name}.png"
                run("import", "-window", "root", str(image))
                text = run("tesseract", str(image), "stdout", "--psm", "11").stdout
                (root / f"{name}.txt").write_text(text)
                return " ".join(text.casefold().split())

            before = image_text("before")
            assert "microphone access is disabled" not in before, before
            assert "draft preserved for review" in before, before
            windows_before = run("xdotool", "search", "--onlyvisible", "--pid", str(app.pid)).stdout.split()
            assert len(windows_before) == 1, windows_before
            socket_path = root / "runtime/jcode-desktop.sock"
            socket_inode = socket_path.stat().st_ino
            run("xdotool", "windowminimize", windows_before[0])
            time.sleep(0.3)
            assert not run("xdotool", "search", "--onlyvisible", "--pid", str(app.pid), check=False).stdout.strip(), "private host did not minimize"
            result = run(str(binary), "--toggle-voice")
            assert result.returncode == 0
            deadline = time.monotonic() + 15
            while True:
                after = image_text("after")
                if "microphone access is disabled in offline previews" in after:
                    break
                if app.poll() is not None or time.monotonic() > deadline:
                    raise AssertionError("queued voice action did not produce visible offline status: " + after)
                time.sleep(0.2)
            assert "draft preserved for review" in after, after
            assert socket_path.stat().st_ino == socket_inode, "request replaced host socket"
            assert app.poll() is None, "original host exited"
            windows_after = run("xdotool", "search", "--onlyvisible", "--pid", str(app.pid)).stdout.split()
            assert windows_before == windows_after, "voice request opened another window"
            (root / "result.json").write_text(json.dumps({
                "passed": True,
                "missing_host_does_not_start": True,
                "production_cli_to_visible_offline_status": True,
                "request_while_minimized": True,
                "draft_preserved": True,
                "same_socket_and_window": True,
                "microphone_opened": False,
                "live_compositor_tested": False,
            }, indent=2) + "\n")
            print(f"PASS: production voice IPC -> visible offline status, preserved draft/window. Evidence: {root}")
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        # These are only the exact child processes launched on our private display.
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


if __name__ == "__main__":
    main()
