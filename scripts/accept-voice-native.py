#!/usr/bin/env python3
"""Opt-in native Nari acceptance with synthetic audio, never a physical device.

Runs the unmodified production host outside screenshot mode in private Xvfb and
bubblewrap. Only the explicit Nari key file is copied, then removed on every exit.
The native app has no daemon executable on PATH, no shared sockets or home, and
no /dev/snd. No Enter/submit input is sent. Cloud mode must be explicitly enabled.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import threading
import time

from screenshot import isolated_env
from voice_virtual_alsa import VirtualAlsa, BYTES_PER_SECOND, FIXTURE_SHA256

PREFIX = "Native dictation review:"
EXPECTED = "hello welcome to nari labs"


def words(text):
    return " ".join(re.findall(r"[a-z]+", text.casefold()))


def sandbox(root):
    # No host home, /run, audio devices, or live desktop socket is mounted.
    # Network stays enabled only for the explicitly authorized Nari stream.
    return ["/usr/bin/bwrap", "--die-with-parent", "--new-session", "--unshare-pid",
            "--ro-bind", "/usr", "/usr", "--ro-bind", "/etc", "/etc",
            "--symlink", "usr/bin", "/bin", "--symlink", "usr/lib", "/lib",
            "--symlink", "usr/lib", "/lib64", "--proc", "/proc", "--dev", "/dev",
            "--tmpfs", "/tmp", "--ro-bind", "/tmp/.X11-unix", "/tmp/.X11-unix",
            "--bind", str(root), str(root), "--chdir", str(root)]


def native_env(root, display, driver):
    env = isolated_env(root)
    env.pop("JCODE_DESKTOP_SCREENSHOT")
    env.update({"DISPLAY": display, "VK_DRIVER_FILES": str(driver),
                "PATH": str(root / "empty-bin"),
                "JCODE_API_SOCKET": str(root / "runtime/absent-api.sock"),
                "JCODE_SOCKET": str(root / "runtime/absent-daemon.sock"),
                "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
                "ALSA_CONFIG_PATH": str(root / "asound.conf")})
    return env


def draft_snapshot(root):
    path = root / "logs/jcode-desktop/crash-recovery.json"
    try:
        snapshot = json.loads(path.read_text())["snapshot"]
        panels = [slot["panel"] for slot in snapshot["slots"]]
        if len(panels) != 1 or panels[0]["session_id"] != "startup://draft":
            return None
        return panels[0]
    except (OSError, ValueError, KeyError):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new private evidence directory")
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--prepare-only", action="store_true",
                        help="validate normal native startup isolation without credentials/audio/cloud")
    parser.add_argument("--allow-cloud", action="store_true",
                        help="explicitly authorize one short synthetic-audio Nari stream")
    parser.add_argument("--key-file", type=Path, help="Nari-only nari.env, copied without printing")
    parser.add_argument("--pcm", type=Path, help="official Nari hello.pcm")
    args = parser.parse_args()
    if args.prepare_only and args.allow_cloud:
        parser.error("choose either --prepare-only or --allow-cloud")
    if not args.prepare_only and not (args.allow_cloud and args.key_file and args.pcm):
        parser.error("cloud mode requires --allow-cloud, --key-file and --pcm")
    binary = args.binary.resolve(strict=True)
    for tool in ("bwrap", "Xvfb", "openbox", "xdotool", "xprop", "import", "tesseract"):
        if not shutil.which(tool):
            parser.error(f"missing required isolation/inspection tool: {tool}")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe required")
    root = args.output.resolve()
    root.mkdir(parents=True, mode=0o700, exist_ok=False)
    for name in ("home", "runtime", "config", "cache", "data", "jcode", "logs", "empty-bin", "bin"):
        (root / name).mkdir(mode=0o700)
    native_binary = root / "bin/jcode-desktop"
    shutil.copyfile(binary, native_binary)
    native_binary.chmod(0o700)
    key_copy = root / "jcode/config/jcode/nari.env"
    pump = None
    (root / "desktop.toml").write_text(
        '[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n'
        '[workspace]\naccount_sign_in_handled = true\n')
    if args.prepare_only:
        (root / "asound.conf").write_text("# Intentionally no PCM devices.\n")
    else:
        # Independently verified against actual CPAL with networking disabled.
        # No includes, plug PCM, hardware PCM, PulseAudio, or PipeWire fallback.
        pump = VirtualAlsa(root / "audio", args.pcm, lead_seconds=3, tail_seconds=20)
        shutil.copyfile(pump.config, root / "asound.conf")
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc">'
                         '<applications><application class="*"><decor>no</decor>'
                         '<maximized>yes</maximized></application></applications></openbox_config>')
    tools_env = isolated_env(root)
    processes = []
    read_fd, write_fd = os.pipe()
    try:
        with (root / "native.log").open("w+") as log:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                                     "1440x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,),
                                    env=tools_env, cwd=root, stdout=log, stderr=log)
            processes.append(xvfb)
            os.close(write_fd)
            write_fd = None
            if not select.select([read_fd], [], [], 15)[0]:
                raise RuntimeError("private Xvfb startup timed out")
            display = os.read(read_fd, 64).decode().strip()
            if not display.isdigit():
                raise RuntimeError("private Xvfb display allocation failed")
            tools_env["DISPLAY"] = ":" + display
            app_env = native_env(root, ":" + display, drivers[0])

            def run(*command, check=True, env=tools_env):
                return subprocess.run(command, env=env, cwd=root, text=True,
                                      capture_output=True, check=check, timeout=15)

            jail = sandbox(root)
            run(*jail, "/usr/bin/test", "!", "-e", "/dev/snd", env=app_env)
            run(*jail, "/usr/bin/test", "!", "-e", "/run/user", env=app_env)
            wm = subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(wm_config)],
                                  env=tools_env, cwd=root, stdout=log, stderr=log)
            processes.append(wm)
            wm_deadline = time.monotonic() + 10
            while True:
                if wm.poll() is not None:
                    raise RuntimeError("private window manager exited")
                properties = run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK").stdout
                if "window id # 0x" in properties:
                    break
                if time.monotonic() > wm_deadline:
                    raise RuntimeError("private window manager did not become ready")
                time.sleep(0.1)
            app = subprocess.Popen([*jail, str(native_binary), "--no-hot-reload"], env=app_env,
                                   cwd=root, stdout=log, stderr=log)
            processes.append(app)

            def wait_for(predicate, seconds, reason):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    if app.poll() is not None:
                        raise RuntimeError("isolated host exited; inspect native.log")
                    result = predicate()
                    if result:
                        return result
                    time.sleep(0.1)
                raise RuntimeError(reason)

            map_requested = False
            render_started = time.monotonic()

            def rendered():
                nonlocal map_requested
                if (root / "state").exists():
                    return True
                # Known intermittent X11 startup leaves the window unmapped.
                # Repair only this private display, and disclose the intervention.
                if not map_requested and time.monotonic() - render_started > 5:
                    windows = run("xdotool", "search", "--class", "jcode-desktop", check=False).stdout.split()
                    if len(windows) == 1:
                        properties = run("xprop", "-id", windows[0], "WM_STATE", "_NET_WM_STATE").stdout
                        (root / "pre-map-properties.txt").write_text(properties)
                        run("import", "-window", "root", str(root / "pre-map.png"))
                        run("xdotool", "windowmap", "--sync", windows[0])
                        run("xdotool", "windowactivate", "--sync", windows[0])
                        map_requested = True
                return False

            wait_for(rendered, 60, "normal native host did not render")
            time.sleep(1)
            windows = run("xdotool", "search", "--onlyvisible", "--class", "jcode-desktop").stdout.split()
            if len(windows) != 1:
                raise AssertionError("expected one private native window")
            run("xdotool", "windowactivate", "--sync", windows[0])
            # Normal first launch has both beta notice and changelog surfaces.
            run("xdotool", "key", "--clearmodifiers", "Escape")
            time.sleep(0.3)
            run("xdotool", "key", "--clearmodifiers", "Escape")
            time.sleep(0.5)
            run("import", "-window", "root", str(root / "startup.png"))
            run("xdotool", "type", "--clearmodifiers", PREFIX)
            panel = wait_for(lambda: (p if (p := draft_snapshot(root)) and
                                      p["draft"]["content"] == PREFIX else None),
                             10, "native startup draft was not preserved")
            run("import", "-window", "root", str(root / "before.png"))
            text = run("tesseract", str(root / "before.png"), "stdout", "--psm", "11").stdout
            (root / "before.txt").write_text(text)
            if words(PREFIX) not in words(text):
                raise AssertionError("draft is not visibly rendered")
            if list((root / "runtime").glob("absent-*.sock")):
                raise AssertionError("unexpected daemon or API socket")
            if panel["prompt_queue"]["prompts"]:
                raise AssertionError("draft unexpectedly queued for sending")
            def image_text(name):
                image = root / f"{name}.png"
                run("import", "-window", "root", str(image))
                text = run("tesseract", str(image), "stdout", "--psm", "11").stdout
                (root / f"{name}.txt").write_text(text)
                return words(text)

            # This is a real native action, not the screenshot microphone guard.
            # No credential has been copied, so it cannot open audio or a stream.
            run(*jail, str(native_binary), "--toggle-voice", env=app_env)
            wait_for(lambda: "configure a valid nari" in image_text("missing-key"),
                     10, "native missing-key guard did not appear")
            if draft_snapshot(root)["draft"]["content"] != PREFIX:
                raise AssertionError("missing-key action changed the draft")
            result = {"passed": True, "prepare_only": True, "normal_startup_draft": True,
                      "native_missing_key_guard": True,
                      "physical_audio_devices_mounted": False, "daemon_executable_available": False,
                      "daemon_sockets_created": False, "cloud_streams": 0,
                      "private_x11_map_requested": map_requested,
                      "binary_sha256": hashlib.sha256(native_binary.read_bytes()).hexdigest()}
            if not args.prepare_only:
                # Copy only the explicitly supplied Nari file, never ambient
                # credentials. JCODE_HOME takes precedence over XDG_CONFIG_HOME.
                key_bytes = args.key_file.read_bytes()
                if len(key_bytes) > 4096 or not any(
                    line.startswith(b"NARI_API_KEY=") for line in key_bytes.splitlines()
                ) or any(line.strip() and not line.startswith((b"#", b"NARI_API_KEY="))
                         for line in key_bytes.splitlines()):
                    raise ValueError("key file must contain only a Nari API key and comments")
                key_copy.parent.mkdir(parents=True, mode=0o700)
                with key_copy.open("xb") as secret:
                    key_copy.chmod(0o600)
                    secret.write(key_bytes)
                del key_bytes
                socket_inode = (root / "runtime/jcode-desktop.sock").stat().st_ino
                run(*jail, str(native_binary), "--toggle-voice", env=app_env)
                wait_for(lambda: "streaming to nari" in image_text("recording"),
                         15, "real native recording state did not appear")
                pump.start()
                audio_started = time.monotonic()
                stop_result = {}

                def stop_recording():
                    time.sleep(6.5)
                    try:
                        run(*jail, str(native_binary), "--toggle-voice", env=app_env)
                        stop_result["at"] = time.monotonic()
                    except Exception as error:
                        stop_result["error"] = type(error).__name__
                        app.terminate()

                stopper = threading.Thread(target=stop_recording, daemon=True)
                stopper.start()

                def audio_budget_guard():
                    # Fail closed even if the native action is unexpectedly
                    # ignored. The tiny output FIFO adds less than 12 ms.
                    while app.poll() is None:
                        if pump.bytes_drained / BYTES_PER_SECOND >= 7:
                            app.terminate()
                            return
                        time.sleep(0.01)

                threading.Thread(target=audio_budget_guard, daemon=True).start()
                # This image is captured before the timed CLI Stop. OCR can
                # finish later without extending capture or delaying Stop.
                time.sleep(5.6)
                partial_capture_at = time.monotonic()
                partial = image_text("live")
                stopper.join(timeout=17)
                if "at" not in stop_result:
                    raise RuntimeError("timed production CLI Stop failed")
                expected = words(PREFIX) + " " + EXPECTED
                final = wait_for(lambda: (p if (p := draft_snapshot(root)) and
                                         words(p["draft"]["content"]) == expected else None),
                                 20, "expected public transcript did not enter the native draft")
                if final["draft"]["content"].count(PREFIX) != 1:
                    raise AssertionError("draft prefix was replaced or duplicated")
                if final["prompt_queue"]["prompts"] or final["draft"]["history"] != panel["draft"]["history"]:
                    raise AssertionError("voice action submitted or queued the draft")
                after = image_text("after")
                if expected not in after:
                    raise AssertionError("final transcript is not visibly rendered")
                if pump.failure or pump.bytes_drained / BYTES_PER_SECOND > 8:
                    raise AssertionError("virtual capture failed or exceeded bounded audio")
                if (root / "runtime/jcode-desktop.sock").stat().st_ino != socket_inode:
                    raise AssertionError("voice request replaced the native host")
                if list((root / "runtime").glob("absent-*.sock")):
                    raise AssertionError("unexpected daemon/API socket")
                result.update({"prepare_only": False, "cloud_streams": 1,
                               "provider": "Nari", "public_fixture_sha256": FIXTURE_SHA256,
                               "virtual_audio_seconds": pump.bytes_drained / BYTES_PER_SECOND,
                               "stop_after_pump_seconds": stop_result["at"] - audio_started,
                               "live_partial_visible": "hello" in partial,
                               "live_image_before_stop": partial_capture_at < stop_result["at"],
                               "final_draft": final["draft"]["content"],
                               "draft_preserved_once": True, "auto_sent": False,
                               "prompt_queue_empty": True, "same_host_socket": True})
            (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(f"PASS native voice acceptance ({result['cloud_streams']} cloud streams): {root}")
    finally:
        key_copy.unlink(missing_ok=True)
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
        # Never stop draining while native capture might still be active.
        if pump is not None:
            pump.close()


if __name__ == "__main__":
    main()
