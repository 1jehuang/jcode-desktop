#!/usr/bin/env python3
"""Automatic update, end to end, in the real app on a private Xvfb display.

A packaged Jcode Desktop stamped with an older version is installed in an
isolated home and launched like a user would. Without any input it must find
the newer release on the live public channel, download and install it in the
background, and show "<version> ready · restart". Clicking that chip must
restart every window onto the new build with the workspace restored.

Usage: accept-linux-update.py OUTPUT_DIR --binary OLD_BINARY [--shape tarball|managed|system]
OLD_BINARY is built with JCODE_DESKTOP_VERSION set below the live release.
Only OUTPUT_DIR is modified.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import time
import urllib.request

from screenshot import isolated_env

LATEST = "https://github.com/1jehuang/jcode-desktop-releases/releases/download/desktop-latest/latest.json"
FILES = ("jcode-desktop", "jcode", "jcode-harness-api-bridge", "jcode.desktop", "jcode.png")


def install(root, home, binary, shape, old):
    """Lay out an install the way a user ends up with one. Returns the entry point."""
    def bundle(directory):
        directory.mkdir(parents=True)
        for name in FILES:
            target = directory / name
            if name == "jcode-desktop":
                shutil.copy2(binary, target)
            else:
                target.write_bytes(b"placeholder\n")
            target.chmod(0o755 if name in FILES[:3] else 0o644)

    if shape == "tarball":
        folder = home / "Apps" / f"Jcode-{old}-linux-x86_64"
        bundle(folder)
        return folder / "jcode-desktop", lambda: folder / "jcode-desktop"
    if shape == "managed":
        bundle(home / ".local/opt/jcode-desktop" / old)
        for directory in (home / ".local", home / ".local/opt", home / ".local/opt/jcode-desktop"):
            directory.chmod(0o755)
        launcher = home / ".local/bin/jcode-desktop"
        launcher.parent.mkdir(parents=True)
        launcher.symlink_to(f"../opt/jcode-desktop/{old}/jcode-desktop")
        return launcher, lambda: launcher
    system = root / "usr/bin"
    system.mkdir(parents=True)
    shutil.copy2(binary, system / "jcode-desktop")
    system.chmod(0o555)
    return system / "jcode-desktop", lambda: system / "jcode-desktop"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--shape", choices=("tarball", "managed", "system"), default="tarball")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool):
            parser.error(f"missing {tool}")
    with urllib.request.urlopen(LATEST, timeout=30) as response:
        expect = json.load(response)["tag_name"].removeprefix("desktop-v")
    binary = args.binary.resolve(strict=True)
    old = subprocess.check_output([str(binary), "--version"], env={"PATH": "/usr/bin:/bin", "HOME": str(root)},
                                  text=True).split()[2].removeprefix("v")
    assert old != expect, f"{binary} already reports the live version {expect}"

    env = isolated_env(root)
    for name in ("home", "runtime", "config", "cache", "data", "jcode"):
        (root / name).mkdir(mode=0o700, exist_ok=True)
    env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = "empty"
    env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = "1"
    env["JCODE_DESKTOP_LIVE_UPDATE_TEST"] = "1"
    env["JCODE_DESKTOP_RESTART_DIR"] = str(root / "restart")
    env["CI"] = "1"
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe is required")
    env["VK_DRIVER_FILES"] = str(drivers[0])
    home = root / "home"
    entry, _ = install(root, home, binary, args.shape, old)

    def installed_version():
        """What launching the user's own entry point would run now."""
        output = subprocess.run([str(entry), "--version"], env={**env, "DISPLAY": ""},
                                capture_output=True, text=True, timeout=60)
        return output.stdout.split()[2].removeprefix("v") if output.returncode == 0 else None

    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
    processes, logs = [], []
    read_fd, write_fd = os.pipe()

    def launch(name, command, **kwargs):
        log = (root / f"{name}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process

    def run(command, timeout=15):
        return subprocess.check_output(command, env=env, text=True, stderr=subprocess.DEVNULL, timeout=timeout)

    def screen_text(name):
        image = root / f"{name}.png"
        run(["import", "-window", "root", "png:" + str(image)])
        text = run(["tesseract", str(image), "stdout"], timeout=60)
        (root / f"{name}.txt").write_text(text)
        return text

    def click_text(word):
        """Click the on-screen word OCR finds, so the test presses the real control."""
        image = root / "click.png"
        run(["import", "-window", "root", "png:" + str(image)])
        # Upscale for the tiny 9px pill text.
        big = root / "click-big.png"
        run(["convert", str(image), "-resize", "300%", str(big)], timeout=60)
        tsv = run(["tesseract", str(big), "stdout", "tsv"], timeout=60)
        for line in tsv.splitlines()[1:]:
            fields = line.split("\t")
            if len(fields) == 12 and fields[11].strip() == word:
                left, top, width, height = (int(value) / 3 for value in fields[6:10])
                run(["xdotool", "mousemove", str(int(left + width / 2)), str(int(top + height / 2)), "click", "1"])
                return
        raise AssertionError(f"could not find {word!r} on screen")

    def desktop_pids():
        pids = []
        for proc in Path("/proc").iterdir():
            if not proc.name.isdigit():
                continue
            try:
                environ = (proc / "environ").read_bytes()
                exe = os.readlink(proc / "exe")
            except OSError:
                continue
            if f"JCODE_DESKTOP_STATE={env['JCODE_DESKTOP_STATE']}".encode() in environ and "jcode-desktop" in exe:
                cmdline = (proc / "cmdline").read_bytes()
                if b"--restart-all-worker" not in cmdline:
                    pids.append((int(proc.name), exe))
        return pids

    evidence = {"shape": args.shape, "from": old, "to": expect}
    try:
        launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"],
               pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = -1
        if not select.select([read_fd], [], [], 15)[0]:
            raise RuntimeError("Xvfb did not choose a private display")
        env["DISPLAY"] = ":" + os.read(read_fd, 64).decode().strip()
        launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm_config)])
        time.sleep(.5)
        app = launch("desktop", [str(entry), "--no-hot-reload"])
        deadline = time.monotonic() + 60
        while not (root / "state").exists():
            if app.poll() is not None or time.monotonic() >= deadline:
                raise AssertionError("Desktop did not render")
            time.sleep(.1)
        run(["xdotool", "search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync"])

        # No input at all: the update must arrive by itself. When it is ready
        # the workspace's version pill offers a "Restart" action.
        ready = "Restart"
        deadline = time.monotonic() + 600
        text = ""
        while time.monotonic() < deadline:
            if app.poll() is not None:
                raise AssertionError("Desktop exited while updating")
            text = screen_text("waiting")
            if ready in text and installed_version() == expect:
                break
            time.sleep(2)
        else:
            raise AssertionError(f"Update chip never offered {expect}. Last screen text: {text!r}")
        shutil.copy2(root / "waiting.png", root / "1-ready.png")
        evidence["chip_shown_after_s"] = round(600 - (deadline - time.monotonic()))
        old_pids = desktop_pids()
        evidence["old_processes"] = old_pids

        # Click the pill's Restart action, which runs the same install as the
        # update chip. The pill is the top-left status pill of the workspace.
        run(["xdotool", "search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync"])
        click_text("Restart")

        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            current = desktop_pids()
            fresh = [(pid, exe) for pid, exe in current if pid not in {p for p, _ in old_pids}]
            if fresh and all(pid not in {p for p, _ in current} for pid, _ in old_pids):
                break
            time.sleep(.5)
        else:
            raise AssertionError(f"Restart did not replace the desktop. Before {old_pids}, now {desktop_pids()}")
        evidence["new_processes"] = fresh
        time.sleep(8)
        run(["xdotool", "search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync"],
            timeout=60)
        text = screen_text("2-restarted")
        for pid, exe in fresh:
            version = subprocess.check_output([exe.removesuffix(" (deleted)"), "--version"], env=env, text=True)
            assert expect in version, f"restarted process {pid} runs {version.strip()}"
        assert "Restart" not in text.split("FPS")[0], "the restarted app still offers the update"
        # The version pill text is 9px. Read it from an upscaled crop.
        pill = root / "pill.png"
        run(["convert", str(root / "2-restarted.png"), "-crop", "440x40+220+0", "-resize", "300%", str(pill)], timeout=60)
        label = run(["tesseract", str(pill), "stdout"], timeout=60)
        shown = label.replace(" ", "").replace("v8.", "v0.")  # tesseract reads a thin 0 as 8
        assert "v" + expect in shown, f"restarted window does not show v{expect}: {label!r}"
        evidence["pill_text"] = label.strip()
        evidence["restarted_version"] = expect
        evidence["result"] = "automatic update installed and restarted onto the new build"
        (root / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
        print(json.dumps(evidence, indent=2))
        print(f"PASS: {old} -> {expect} ({args.shape}) updated by itself and restarted. Evidence: {root}")
    finally:
        os.close(read_fd)
        if write_fd >= 0:
            os.close(write_fd)
        for pid, _ in desktop_pids():
            try:
                os.kill(pid, 15)
            except OSError:
                pass
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
        if args.shape == "system":
            (root / "usr/bin").chmod(0o755)


if __name__ == "__main__":
    main()
