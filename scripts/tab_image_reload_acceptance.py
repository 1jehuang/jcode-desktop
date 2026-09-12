#!/usr/bin/env python3
"""Check animated texture identities across real linked-UI/cdylib reloads.

Build matching host and plugin first. Runs only on private Xvfb with offline
fixtures. Cargo is replaced in the child by a gated no-op: this tests actual
library activation and painting, not compilation. The gate lets the linked UI
paint before the first activation, exercising the original counter collision.
"""
import argparse
import json
import os
from pathlib import Path
import re
import select
import subprocess
import time

from screenshot import isolated_env
from reload_lifecycle_probe import wait_for


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    env = isolated_env(root)
    for name in ("home", "runtime", "config", "cache", "data", "jcode"):
        (root / name).mkdir(mode=0o700)
    env["VK_DRIVER_FILES"] = str(next(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json")))
    env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = "streaming"
    env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = "3"
    cargo = root / "prebuilt-cargo"
    env["IMAGE_PROBE_GATE"] = str(root / "linked-ui-painted")
    cargo.write_text('#!/bin/sh\nwhile [ ! -f "$IMAGE_PROBE_GATE" ]; do sleep 0.1; done\nexit 0\n')
    cargo.chmod(0o700)
    env["CARGO"] = str(cargo)
    diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
    wm_config = root / "openbox.xml"
    wm_config.write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<applications><application class="*"><decor>no</decor>
<maximized>yes</maximized></application></applications></openbox_config>''')
    processes = []
    try:
        with (root / "xvfb.log").open("w") as xlog, (root / "app.log").open("w") as log:
            read_fd, write_fd = os.pipe()
            try:
                xvfb = subprocess.Popen(
                    ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"],
                    pass_fds=(write_fd,), env=env, stdout=xlog, stderr=xlog)
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise TimeoutError("Private Xvfb failed to start")
                display = os.read(read_fd, 64).decode().strip()
                assert display.isdigit(), display
                env["DISPLAY"] = ":" + display
            finally:
                os.close(read_fd)
                if write_fd is not None:
                    os.close(write_fd)
            wm = subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(wm_config)],
                                  env=env, stdout=xlog, stderr=xlog)
            processes.append(wm)
            time.sleep(.5)
            assert wm.poll() is None, "Private window manager failed to start"
            binary = repo / "target/debug/jcode-desktop"
            plugin = repo / "target/debug/libjcode_desktop_ui.so"
            app = subprocess.Popen([str(binary), "--hot-reload", str(plugin)],
                                   env=env, stdout=log, stderr=log)
            processes.append(app)
            wait_for(lambda: diagnostics.exists() and
                     "desktop-image tab-emoji-frame pose=1" in diagnostics.read_text(), app)
            time.sleep(1)
            subprocess.run(["import", "-window", "root", str(root / "generation-0.png")],
                           env=env, check=True, timeout=10)
            Path(env["IMAGE_PROBE_GATE"]).touch()
            for generation in range(1, 4):
                if generation > 1:
                    subprocess.run(["xdotool", "key", "ctrl+r"], env=env, check=True, timeout=10)
                wait_for(lambda: diagnostics.exists() and
                         f"activated UI generation {generation} " in diagnostics.read_text(), app)
                wait_for(lambda: "desktop-image tab-emoji-frame pose=1" in
                         diagnostics.read_text().split(f"activated UI generation {generation} ")[-1], app)
                time.sleep(1)
                subprocess.run(["import", "-window", "root", str(root / f"generation-{generation}.png")],
                               env=env, check=True, timeout=10)
                print(f"Painted real UI generation {generation}", flush=True)
            text = diagnostics.read_text()
            generations = re.split(r"activated UI generation \d+ [^\n]*", text)
            allocations = []
            for index, segment in enumerate(generations):
                frames = re.findall(r"desktop-image tab-emoji-frame pose=(\d+) texture=(\d+)", segment)
                assert frames, f"No native animated frames in generation {index}"
                assert {pose for pose, _ in frames} == {"0", "1"}, frames
                allocations.extend(map(int, re.findall(
                    r"desktop-image (?:tab-emoji-frame|decode-start) [^\n]*texture=(\d+)", segment)))
            assert len(allocations) == len(set(allocations)), "Texture ID reused across library generations"
            assert allocations == sorted(allocations), "Allocator restarted across reload"
            result = {"painted_generations_including_linked_ui": len(generations),
                      "unique_texture_ids": allocations, "both_poses_each_generation": True}
            (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result), flush=True)
    finally:
        Path(env["IMAGE_PROBE_GATE"]).touch()
        for process in reversed(processes):
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    main()
