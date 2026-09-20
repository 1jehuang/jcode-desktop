#!/usr/bin/env python3
"""Check real activity-orb animation on private Xvfb displays, never the live desktop.

Build the current binary first. Captures normal/reduced-motion/light/idle cases,
checks a sidebar activity crop across time, and retains screenshots and logs.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import time

from screenshot import isolated_env


def capture_case(binary, root, *, theme, reduced=False, transcript="streaming"):
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ("home", "runtime", "config", "cache", "data", "jcode"):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env["VK_DRIVER_FILES"] = str(next(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json")))
    env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = transcript
    env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = "1"
    config = root / "desktop.toml"
    config.write_text(f'[appearance]\nlayout_mode = "folder_tabs"\ntheme = "{theme}"\n'
                      f'reduce_motion = {str(reduced).lower()}\n[workspace]\ncoaching_hints = false\n')
    env["JCODE_DESKTOP_CONFIG"] = str(config)
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                         '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                         '</application></applications></openbox_config>')
    processes, logs = [], []
    read_fd, write_fd = os.pipe()

    def launch(name, command, **kwargs):
        log = (root / f"{name}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process

    def run(command, **kwargs):
        return subprocess.run(command, env=env, cwd=root, check=True, timeout=15, **kwargs)

    try:
        launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24",
                        "-nolisten", "tcp"], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        if not select.select([read_fd], [], [], 15)[0]:
            raise RuntimeError("Xvfb did not start")
        display = os.read(read_fd, 64).decode().strip()
        if not display.isdigit():
            raise RuntimeError("Invalid private display")
        env["DISPLAY"] = ":" + display
        launch("wm", ["openbox", "--sm-disable", "--config-file", str(wm_config)])
        time.sleep(0.5)
        app = launch("app", [str(binary), "--no-hot-reload"])
        deadline = time.monotonic() + 45
        while not (root / "state").exists():
            if app.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError(f"App failed to render, see {root}")
            time.sleep(0.1)
        # State emission precedes actual X11 presentation under build load.
        time.sleep(3)
        run(["xdotool", "key", "--clearmodifiers", "Escape"])
        first = root / "frame-0.png"
        while True:
            if app.poll() is not None:
                raise RuntimeError(f"App exited before presentation, see {root}")
            run(["import", "-window", "root", "png:" + str(first)])
            deviation = float(run(["magick", str(first), "-format", "%[standard-deviation]", "info:"],
                                  capture_output=True, text=True).stdout)
            if deviation > 100:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("Native capture remained blank")
            time.sleep(0.25)
        time.sleep(0.5)
        hashes = []
        def cpu_ticks():
            fields = Path(f"/proc/{app.pid}/stat").read_text().split(") ", 1)[1].split()
            return int(fields[11]) + int(fields[12])
        cpu_before = cpu_ticks()
        sample_started = time.monotonic()
        # Fixed offline fixture region containing the right-aligned sidebar orb,
        # not the blinking composer cursor or the assistant's avatar animation.
        for index in range(8):
            frame = root / f"frame-{index}.png"
            run(["import", "-window", "root", "png:" + str(frame)])
            crop = run(["magick", str(frame), "-crop", "38x38+214+126", "+repage", "rgb:-"],
                       capture_output=True).stdout
            hashes.append(hashlib.sha256(crop).hexdigest())
            time.sleep(0.09)
        sample_seconds = time.monotonic() - sample_started
        cpu_percent = (cpu_ticks() - cpu_before) / os.sysconf("SC_CLK_TCK") / sample_seconds * 100
        distinct = len(set(hashes))
        if reduced or transcript == "empty":
            assert distinct == 1, f"Static activity crop changed: {distinct} frames"
        else:
            assert distinct >= 4, f"Activity animation did not visibly advance: {distinct} frames"
        run(["magick", str(first), "-crop", "38x38+214+126", "+repage", "-filter", "point",
             "-resize", "400%", str(root / "orb-enlarged.png")])
        return {"theme": theme, "reduced_motion": reduced, "transcript": transcript,
                "distinct_activity_frames": distinct, "samples": len(hashes), "hashes": hashes,
                "sample_seconds": sample_seconds, "fixture_cpu_percent": cpu_percent}
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
                    process.wait(timeout=5)
        for log in logs:
            log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/jcode-desktop"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    cases = {"dark": dict(theme="neutral-dark"), "light": dict(theme="neutral-light"),
             "reduced": dict(theme="neutral-dark", reduced=True),
             "idle": dict(theme="neutral-dark", transcript="empty")}
    result = {}
    for name, options in cases.items():
        result[name] = capture_case(binary, root / name, **options)
        print(f"JCODE_CHECKPOINT {json.dumps({'message': name + ' native activity check passed'})}", flush=True)
    (root / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
