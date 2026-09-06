#!/usr/bin/env python3
"""Exercise Machines controls and persisted defaults in the real app on private Xvfb.

The screenshot fixture has an inert runtime bridge. No SSH connection or user
configuration is touched. Backend SSH behavior has separate SDK/bridge tests.
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time
import tomllib

from screenshot import isolated_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/jcode-desktop"))
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    scratch = os.environ.get("JCODE_SCRATCH_DIR", str(output))
    with tempfile.TemporaryDirectory(prefix="machines-ui-", dir=scratch) as temporary:
        root = Path(temporary)
        env = isolated_env(root)
        for name in ("HOME", "XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "JCODE_HOME"):
            Path(env[name]).mkdir(parents=True, exist_ok=True)
        os.chmod(env["XDG_RUNTIME_DIR"], 0o700)
        config = root / "desktop.toml"
        config.write_text('[workspace]\nremote_hosts = ["workstation", "build-server"]\n')
        env["JCODE_DESKTOP_CONFIG"] = str(config)
        read_fd, write_fd = os.pipe()
        xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        os.close(write_fd)
        processes = [xvfb]
        app = None
        logs = []
        try:
            if not select.select([read_fd], [], [], 10)[0]:
                raise RuntimeError("Xvfb did not start")
            env["DISPLAY"] = ":" + os.read(read_fd, 80).decode().strip()
            os.close(read_fd)
            wm_config = root / "openbox.xml"
            wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
            manager = subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(wm_config)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            processes.append(manager)
            drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
            if drivers:
                env["VK_DRIVER_FILES"] = str(drivers[0])

            def command(*args):
                return subprocess.check_output(args, env=env, text=True, stderr=subprocess.DEVNULL)

            def capture(name):
                path = output / (name + ".png")
                deadline = time.monotonic() + 30
                while True:
                    command("import", "-window", "root", str(path))
                    tsv = command("tesseract", str(path), "stdout", "tsv")
                    words = []
                    for line in tsv.splitlines()[1:]:
                        fields = line.split("\t", 11)
                        if len(fields) == 12 and fields[11].strip():
                            words.append({"text": fields[11].strip(), "x": int(fields[6]) + int(fields[8]) // 2, "y": int(fields[7]) + int(fields[9]) // 2})
                    if words or time.monotonic() > deadline:
                        return words
                    time.sleep(0.5)

            def click_word(words, text, after=0):
                candidates = [w for w in words if w["text"] == text and w["x"] < 275 and w["y"] > after]
                if not candidates:
                    raise AssertionError(f"Cannot find {text!r} after y={after}: {words}")
                word = min(candidates, key=lambda w: (w["y"], w["x"]))
                command("xdotool", "mousemove", str(word["x"]), str(word["y"]), "click", "1")
                time.sleep(0.7)
                return word["y"]

            def launch(label):
                nonlocal app
                log = open(output / f"{label}.log", "w")
                logs.append(log)
                state = Path(env["JCODE_DESKTOP_STATE"])
                state.unlink(missing_ok=True)
                app = subprocess.Popen([str(args.binary.resolve()), "--no-hot-reload"], env=env, cwd=root, stdout=log, stderr=log)
                processes.append(app)
                for _ in range(100):
                    if app.poll() is not None:
                        raise RuntimeError(f"App exited {app.returncode}, see {label}.log")
                    try:
                        windows = command("xdotool", "search", "--onlyvisible", "--class", "jcode").splitlines()
                        if windows:
                            command("xdotool", "windowactivate", "--sync", windows[-1])
                            deadline = time.monotonic() + 45
                            while not state.exists() or "widths=" not in state.read_text():
                                if app.poll() is not None or time.monotonic() > deadline:
                                    raise RuntimeError(f"App did not render; see {label}.log")
                                time.sleep(0.1)
                            time.sleep(2)
                            return
                    except subprocess.CalledProcessError:
                        pass
                    time.sleep(0.1)
                raise RuntimeError("No fixture window")

            def preference():
                return tomllib.loads(config.read_text()).get("workspace", {})

            def wait_default(host):
                deadline = time.monotonic() + 10
                while preference().get("default_remote_host") != host and time.monotonic() < deadline:
                    time.sleep(0.1)
                if preference().get("default_remote_host") != host:
                    capture("default-failed")
                    raise AssertionError(f"Default click did not persist {host!r}: {preference()}")

            launch("initial")
            click_word(capture("initial"), "Machines")
            words = capture("picker")
            workstation = next(w for w in words if w["text"] == "workstation" and w["x"] < 275)
            click_word(words, "Set", after=workstation["y"])
            wait_default("workstation")
            capture("remote-default")
            app.terminate()
            app.wait(timeout=10)
            launch("restored")
            words = capture("restored")
            assert any(w["text"] == "workstation" and w["x"] < 275 for w in words), words
            if any(w["text"] == "Add" and w["x"] < 275 for w in words):
                click_word(words, "Add")
            else:
                click_word(words, "Machines")
            words = capture("ready-to-type")
            windows = command("xdotool", "search", "--onlyvisible", "--class", "^jcode-desktop$").splitlines()
            command("xdotool", "windowactivate", "--sync", windows[-1])
            time.sleep(0.3)
            placeholder = next(w for w in words if w["text"] == "SSH" and w["x"] < 100 and w["y"] > 400)
            command("xdotool", "mousemove", str(placeholder["x"]), str(placeholder["y"]), "click", "1")
            time.sleep(0.5)
            command("xdotool", "type", "--clearmodifiers", "--delay", "50", "builder@lab")
            capture("typed-host")
            command("xdotool", "key", "Return")
            deadline = time.monotonic() + 5
            while "builder@lab" not in preference()["remote_hosts"] and time.monotonic() < deadline:
                time.sleep(0.1)
            capture("after-enter")
            assert "builder@lab" in preference()["remote_hosts"], preference()
            words = capture("custom-host")
            click_word(words, "Set")  # This computer is the first row.
            wait_default("")
            capture("local-default")
            report = {"native_picker": "passed", "remote_default_persisted": "passed", "remote_default_restored_after_restart": "passed", "typed_host_enter": "passed", "local_default_restored": "passed", "runtime": "offline fixture, no SSH or model calls", "config": preference()}
            (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            for log in logs:
                log.close()
            diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
            if diagnostics.exists():
                (output / "diagnostics.log").write_text(diagnostics.read_text())


if __name__ == "__main__":
    main()
