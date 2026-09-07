#!/usr/bin/env python3
"""Probe real cdylib reload task retention on private Xvfb with offline fixtures.

Build matching host and plugin first. This deliberately substitutes /usr/bin/true
for Cargo inside the isolated child so reloads reuse that exact prebuilt plugin.
It tests allocation/task lifecycle, not rebuild correctness. It never connects
to the user's display, daemon, or instance socket. Artifacts are retained.
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import time

from screenshot import isolated_env


def wait_for(predicate, app, timeout=30):
    deadline = time.monotonic() + timeout
    while not predicate():
        if app.poll() is not None:
            raise RuntimeError(f"Isolated app exited with {app.returncode}")
        if time.monotonic() >= deadline:
            raise TimeoutError("Expected isolated app checkpoint did not arrive")
        time.sleep(.1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history", action="store_true", help="populate the sidebar virtual list")
    parser.add_argument("output", type=Path, help="new artifact directory")
    parser.add_argument("--seconds", type=int, default=6, choices=range(3, 31))
    parser.add_argument("--reloads", type=int, default=2, choices=range(1, 6))
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    binary = (repo / "target/debug/jcode-desktop").resolve(strict=True)
    plugin = (repo / "target/debug/libjcode_desktop_ui.so").resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    env = isolated_env(root)
    for name in ("home", "runtime", "config", "cache", "data", "jcode"):
        (root / name).mkdir(mode=0o700)
    env["VK_DRIVER_FILES"] = str(next(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json")))
    env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = "empty"
    if args.history:
        env["JCODE_DESKTOP_SCREENSHOT_HISTORY"] = "1"
    env["CARGO"] = "/usr/bin/true"
    diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
    processes = []
    results = []
    try:
        with (root / "xvfb.log").open("w") as xlog, (root / "app.log").open("w") as log:
            read_fd, write_fd = os.pipe()
            try:
                xvfb = subprocess.Popen(
                    ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1000x700x24", "-nolisten", "tcp"],
                    pass_fds=(write_fd,), env=env, stdout=xlog, stderr=xlog)
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise TimeoutError("Private Xvfb failed to start")
                display = os.read(read_fd, 64).decode().strip()
                if not display.isdigit():
                    raise RuntimeError("Invalid private display")
                env["DISPLAY"] = ":" + display
            finally:
                os.close(read_fd)
                if write_fd is not None:
                    os.close(write_fd)
            app = subprocess.Popen([str(binary), "--hot-reload", str(plugin)],
                                   env=env, stdout=log, stderr=log)
            processes.append(app)
            for generation in range(1, args.reloads + 2):
                if generation > 1:
                    subprocess.run(["xdotool", "key", "ctrl+r"], env=env, check=True, timeout=10)
                wait_for(lambda: diagnostics.exists() and
                         f"activated UI generation {generation} " in diagnostics.read_text(), app)
                time.sleep(1)
                capture_id = f"generation-{generation}"
                request = root / "runtime/jcode-desktop-profile.json"
                request.write_text(json.dumps({"capture_id": capture_id,
                                               "until_unix_ms": int(time.time() * 1000) + args.seconds * 1000}))
                time.sleep(args.seconds + 1)
                if app.poll() is not None:
                    raise RuntimeError("App exited during capture")
                files = list((root / "runtime").glob(f"*-{capture_id}.jsonl"))
                rows = [json.loads(line) for file in files for line in file.read_text().splitlines()]
                rss = next(line.split()[1] for line in Path(f"/proc/{app.pid}/status").read_text().splitlines()
                           if line.startswith("VmRSS:"))
                result = {"generation": generation, "capture_seconds": args.seconds,
                          "samples": len(rows), "samples_per_second": len(rows) / args.seconds,
                          "rss_kib": int(rss), "windows": sorted({row["window"] for row in rows})}
                results.append(result)
                print(json.dumps(result), flush=True)
                (root / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    finally:
        for process in reversed(processes):
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    # A sampler waits >=100ms per sample. Allow margin, but reject duplicate
    # active tasks instead of deduplicating their output. Missing data fails too.
    assert all(result["samples"] > 0 and len(result["windows"]) == 1 for result in results), results
    assert all(result["samples_per_second"] <= 11 for result in results), (
        "Retained live profiler tasks after actual cdylib reload", results)


if __name__ == "__main__":
    main()
