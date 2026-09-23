#!/usr/bin/env python3
"""Accept the shared single-panel host on private Xvfb using an existing binary.

Several `--single-panel` launches must become windows of ONE host process, each
with its own launch arguments and state, while `--new-process` and the normal
workspace stay separate. Never builds, reloads, connects to a daemon, or sends
input to the live desktop.
"""
import argparse
from contextlib import ExitStack
import importlib.util
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time

from screenshot import isolated_env

SPEC = importlib.util.spec_from_file_location(
    "standalone", Path(__file__).with_name("accept-single-panel.py"))
STANDALONE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STANDALONE)
navigation, wait_for, socket_ready = (
    STANDALONE.navigation, STANDALONE.wait_for, STANDALONE.socket_ready)
WIDTH, HEIGHT = STANDALONE.WIDTH, STANDALONE.HEIGHT


def rss_mib(pid):
    """Proportional set size, so shared libraries are counted once."""
    try:
        for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
            if line.startswith("Pss:"):
                return int(line.split()[1]) / 1024
    except OSError:
        pass
    return None


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new directory for retained evidence")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--windows", type=int, default=4, help="shared windows to open")
    args = parser.parse_args()
    for tool in ("Xvfb", "openbox", "xdotool", "import"):
        if not shutil.which(tool):
            parser.error("missing executable: " + tool)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe is required")
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must be an existing executable. This script never builds.")
    output = args.output.resolve()
    if output.exists():
        parser.error("refusing to overwrite " + str(output))
    output.mkdir(parents=True)
    report = {"scope": "offline shared single-panel host on private Xvfb + Openbox",
              "binary": str(binary), "passed": False, "checks": [], "memory_mib": {}}
    scratch = repo / "target"
    with tempfile.TemporaryDirectory(prefix="ssp-", dir=scratch) as temporary, ExitStack() as stack:
        root = Path(temporary)
        env = isolated_env(root)
        env.update(VK_DRIVER_FILES=str(drivers[0]), JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT="empty",
                   JCODE_DESKTOP_SCREENSHOT_PANELS="1",
                   JCODE_DESKTOP_CONFIG=str(root / "desktop.toml"))
        for name in ("home", "runtime", "config", "cache", "data", "logs", "jcode"):
            (root / name).mkdir(mode=0o700)
        for name in ("alpha", "beta"):
            (root / "home" / name).mkdir()
        (root / "desktop.toml").write_text('[appearance]\nlayout_mode = "folder_tabs"\n')
        (root / "openbox.xml").write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<keyboard/><applications><application class="*"><decor>no</decor>
<maximized>yes</maximized></application></applications></openbox_config>''')
        processes = []
        read_fd, write_fd = os.pipe()
        shared_socket = root / "runtime" / "jcode-desktop-single-panel.sock"

        def spawn(name, command, child_env):
            log = stack.enter_context((output / (name + ".log")).open("w"))
            process = subprocess.Popen(command, env=child_env, cwd=root, stdout=log, stderr=log)
            processes.append(process)
            return process

        def native(*argv, check=True):
            return subprocess.run(["xdotool", *map(str, argv)], env=env, cwd=root,
                                  text=True, capture_output=True, check=check, timeout=10)

        def windows(pid):
            result = native("search", "--onlyvisible", "--pid", pid, check=False)
            return result.stdout.split() if result.returncode == 0 else []

        def launch(name, extra, working_dir=None):
            child_env = dict(env, JCODE_DESKTOP_STATE=str(root / (name + ".state")))
            if working_dir:
                child_env["JCODE_DESKTOP_WORKING_DIR"] = str(working_dir)
            return spawn(name, [str(binary), "--no-hot-reload", *extra], child_env)

        try:
            log = stack.enter_context((output / "xvfb.log").open("w"))
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                                     f"{WIDTH}x{HEIGHT}x24", "-nolisten", "tcp"],
                                    pass_fds=(write_fd,), env=env, cwd=root, stdout=log, stderr=log)
            processes.append(xvfb)
            os.close(write_fd)
            write_fd = None
            assert select.select([read_fd], [], [], 15)[0], "Xvfb did not become ready"
            env["DISPLAY"] = ":" + os.read(read_fd, 64).decode().strip()
            spawn("openbox", ["openbox", "--sm-disable", "--config-file", str(root / "openbox.xml")], env)
            time.sleep(.5)

            host = launch("shared-0", ["--single-panel"], root / "home" / "alpha")
            wait_for(lambda: socket_ready(shared_socket), "shared host socket", processes)
            wait_for(lambda: len(windows(host.pid)) == 1, "first shared window", processes)
            wait_for(lambda: navigation(root / "shared-0.state"), "shared-0 state", processes)
            time.sleep(1.5)
            report["memory_mib"]["1_window"] = rss_mib(host.pid)

            for index in range(1, args.windows):
                name = f"shared-{index}"
                extra = ["--single-panel"] + (["--resume"] if index == 1 else [])
                working = root / "home" / ("beta" if index % 2 else "alpha")
                forwarder = launch(name, extra, working)
                processes.remove(forwarder)
                assert forwarder.wait(timeout=15) == 0, name + " forwarder failed"
                wait_for(lambda: len(windows(host.pid)) == index + 1,
                         f"{index + 1} windows in the shared host", [host])
                wait_for(lambda: navigation(root / (name + ".state")), name + " state", [host])
                time.sleep(1.5)
                report["memory_mib"][f"{index + 1}_windows"] = rss_mib(host.pid)
            report["checks"].append(
                f"{args.windows} --single-panel launches share one host PID {host.pid}; forwarders exit 0")

            ids = windows(host.pid)
            assert len(set(ids)) == args.windows, ids
            for index in range(args.windows):
                state = navigation(root / f"shared-{index}.state")
                assert state and state["single_panel"] is True, (index, state)
                assert state["visible_panels"] == 1 and state["sidebar_visible"] is False, state
            report["checks"].append("every shared window reports its own single-panel state file")

            # Launch arguments are per window: only shared-1 asked for --resume.
            resume_open = []
            for index in range(args.windows):
                text = (root / f"shared-{index}.state").read_text()
                resume_open.append('"resume_open":true' in text.replace(" ", ""))
            report["resume_open_by_window"] = resume_open

            isolated = launch("isolated", ["--single-panel", "--new-process"])
            wait_for(lambda: len(windows(isolated.pid)) == 1, "isolated window", processes)
            assert isolated.pid != host.pid
            assert (root / "runtime" / f"jcode-desktop-single-panel-{isolated.pid}.sock").exists()
            report["checks"].append("--new-process keeps a separate PID and per-PID socket")

            for index, window in enumerate(ids[:2]):
                subprocess.run(["import", "-window", window, "png:" + str(output / f"shared-{index}.png")],
                               env=env, check=True, timeout=20)

            # Closing one window leaves the host and its other windows running.
            first = ids[0]
            native("windowactivate", "--sync", first)
            native("key", "--clearmodifiers", "super+q")
            wait_for(lambda: len(windows(host.pid)) == args.windows - 1, "one window closed", [host])
            assert host.poll() is None, "shared host exited with windows still open"
            assert socket_ready(shared_socket), "shared socket went away"
            report["checks"].append("Super+Q closes one shared window; host and other windows keep running")

            # A later launch still reaches the same host after a close.
            late = launch("shared-late", ["--single-panel"])
            processes.remove(late)
            assert late.wait(timeout=15) == 0
            wait_for(lambda: len(windows(host.pid)) == args.windows, "late window", [host])
            report["checks"].append("launch after a close joins the same host")

            # Keep closing until no window remains.
            deadline = time.monotonic() + 30
            while windows(host.pid) and time.monotonic() < deadline:
                window = windows(host.pid)[0]
                native("windowactivate", "--sync", window, check=False)
                # The resume picker ignores Super+Q (also true of isolated
                # windows). Escape returns it to the chat first.
                native("key", "--clearmodifiers", "Escape", check=False)
                time.sleep(.2)
                native("key", "--clearmodifiers", "super+q", check=False)
                time.sleep(.5)
            report["windows_left_after_close"] = windows(host.pid)
            for index, window in enumerate(windows(host.pid)):
                subprocess.run(["import", "-window", window, "png:" + str(output / f"left-{index}.png")],
                               env=env, check=False, timeout=20)
            wait_for(lambda: host.poll() is not None, "host exit after last window", [], timeout=20)
            processes.remove(host)
            assert host.returncode == 0, host.returncode
            assert not shared_socket.exists(), "shared socket not removed on exit"
            report["checks"].append("host exits 0 after its last window and removes its socket")
            report["passed"] = True
        except BaseException as error:
            report["error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            if write_fd is not None:
                os.close(write_fd)
            os.close(read_fd)
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
            for process in reversed(processes):
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
            for path in root.glob("*.state"):
                shutil.copy2(path, output / path.name)
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print("PASS: shared single-panel host. Evidence: " + str(output))
    print(json.dumps(report["memory_mib"], indent=2))


if __name__ == "__main__":
    main()
