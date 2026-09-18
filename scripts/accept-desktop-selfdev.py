#!/usr/bin/env python3
"""Real desktop_selfdev acceptance using a private daemon and offline Xvfb host.

No provider/model request, inherited auth, shared daemon, live display, cargo shim,
or focus stealing. Builds both Desktop packages, then verifies real tool calls and
an actual Ctrl+R-equivalent Cargo rebuild/UI generation. Retains all artifacts.
"""
import argparse
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import time

from screenshot import isolated_env


EXPECTED_STATES = {"empty", "streaming", "login-error", "model-access-error",
                   "rate-limit", "disconnected", "login-dialog-error"}


def wait_for(predicate, processes, message, timeout=60):
    deadline = time.monotonic() + timeout
    while True:
        for process in processes:
            if process.poll() is not None:
                raise RuntimeError(f"{message}: process {process.pid} exited {process.returncode}")
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError(message)
        time.sleep(.1)


def debug_request(path, command, session_id=None, timeout=30):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(timeout)
        client.connect(str(path))
        request = {"type": "debug_command", "id": 1, "command": command,
                   "session_id": session_id}
        client.sendall(json.dumps(request).encode() + b"\n")
        with client.makefile("rb") as reader:
            line = reader.readline(2 * 1024 * 1024)
    reply = json.loads(line)
    if reply.get("type") != "debug_response" or not reply.get("ok"):
        raise RuntimeError(f"Debug command failed: {reply}")
    return json.loads(reply["output"])


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--jcode", type=Path, required=True, help="Freshly built daemon binary")
    parser.add_argument("--output", type=Path,
                        default=repo / "target" / f"selfdev-accept-{int(time.time())}")
    args = parser.parse_args()
    daemon_binary = args.jcode.resolve(strict=True)
    root = args.output.resolve()
    target = (repo / "target").resolve()
    if not root.is_relative_to(target):
        parser.error("output must be a new directory inside Desktop target/")
    for tool in ("cargo", "rustc", "Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool, path="/usr/bin:/bin"):
            parser.error(f"required executable missing: {tool}")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe Vulkan driver is required")
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    env = isolated_env(root)
    for name in ("home", "runtime", "config", "cache", "data", "jcode", "logs"):
        (root / name).mkdir(mode=0o700)
    # Only compiler homes cross the isolation boundary. No auth/model environment.
    env["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))
    env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
    env["CARGO_NET_OFFLINE"] = "true"
    env["CARGO"] = "/usr/bin/cargo"
    env["JCODE_SCRATCH_DIR"] = str(root)
    env["JCODE_RUNTIME_DIR"] = str(root / "runtime")
    env["JCODE_SOCKET"] = str(root / "runtime/daemon.sock")
    env["JCODE_API_SOCKET"] = str(root / "runtime/api.sock")
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_NO_TELEMETRY"] = "1"
    env["JCODE_TEMP_SERVER"] = "1"
    env["JCODE_SERVER_OWNER_PID"] = str(os.getpid())
    env["JCODE_TEMP_SERVER_IDLE_SECS"] = "900"
    env["JCODE_DESKTOP_SELF_DEV"] = "1"
    env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = "empty"
    env["VK_DRIVER_FILES"] = str(drivers[0])
    (root / "jcode/config.toml").write_text("[features]\nmemory = false\n")
    desktop_config = root / "desktop.toml"
    desktop_config.write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n')
    env["JCODE_DESKTOP_CONFIG"] = str(desktop_config)
    debug_socket = root / "runtime/daemon-debug.sock"
    if len(str(debug_socket).encode()) >= 104:
        parser.error("output pathname is too long for private Unix sockets")
    report = {"scope": "real tool calls, isolated daemon, offline Desktop, private Xvfb, real Cargo rebuilds",
              "jcode": str(daemon_binary), "repo": str(repo), "artifacts": str(root), "checks": {}}
    processes = []
    files = []

    def spawn(command, log_name, **kwargs):
        log = (root / log_name).open("w")
        files.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log,
                                   start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def native(*arguments):
        return subprocess.check_output(["xdotool", *map(str, arguments)], env=env,
                                       cwd=root, stderr=subprocess.STDOUT, timeout=10).decode().strip()

    def navigation():
        path = root / "state"
        if not path.exists():
            return None
        for line in path.read_text().splitlines():
            if line.startswith("navigation="):
                try:
                    return json.loads(line.split("=", 1)[1])
                except json.JSONDecodeError:
                    return None
        return None

    def capture(name):
        path = root / f"{name}.png"
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=env, check=True, timeout=20)
        return path

    def check_draft(name, phrase):
        # Real native draft content, observed in rendered pixels rather than an internal stub.
        path = capture(name)
        text = subprocess.check_output(["tesseract", str(path), "stdout", "--psm", "11"],
                                       env=env, stderr=subprocess.DEVNULL, timeout=30).decode()
        (root / f"{name}.txt").write_text(text)
        assert phrase in " ".join(text.lower().split()), text
        return str(path)

    try:
        print("JCODE_CHECKPOINT " + json.dumps({"message": "Building paired Desktop host/UI with real Cargo"}), flush=True)
        with (root / "paired-build.log").open("w") as build_log:
            subprocess.run(["/usr/bin/cargo", "build", "-p", "jcode-desktop", "-p", "jcode-desktop-ui"],
                           cwd=repo, env=env, stdout=build_log, stderr=build_log, check=True, timeout=900)
        report["checks"]["paired_build"] = True
        binary = repo / "target/debug/jcode-desktop"
        plugin = repo / "target/debug/libjcode_desktop_ui.so"
        assert binary.is_file() and plugin.is_file()
        read_fd, write_fd = os.pipe()
        try:
            xvfb = spawn(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"],
                         "xvfb.log", pass_fds=(write_fd,))
            os.close(write_fd)
            write_fd = None
            if not select.select([read_fd], [], [], 15)[0]:
                raise TimeoutError("Xvfb did not allocate a private display")
            display = os.read(read_fd, 64).decode().strip()
            assert display.isdigit(), display
            env["DISPLAY"] = ":" + display
        finally:
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
        wm_config = root / "openbox.xml"
        wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                             '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                             '</application></applications></openbox_config>')
        spawn(["openbox", "--sm-disable", "--config-file", str(wm_config)], "openbox.log")
        time.sleep(.5)
        daemon = spawn([str(daemon_binary), "--no-update", "--no-selfdev", "--provider", "jcode", "serve"], "daemon.log")
        wait_for(debug_socket.exists, processes, "Private daemon debug socket did not appear")
        created = debug_request(debug_socket, "create_session:" + str(repo))
        session = created["session_id"]
        report["session_id"] = session

        def tool(action, **params):
            params.update(action=action, intent="Private real Desktop selfdev acceptance")
            envelope = debug_request(debug_socket, "tool:desktop_selfdev " + json.dumps(params), session, timeout=650)
            value = json.loads(envelope["output"])
            (root / f"tool-{action}.json").write_text(json.dumps(value, indent=2) + "\n")
            return value

        host = spawn([str(binary), "--hot-reload", str(plugin)], "host.log")
        diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"

        def generation():
            if not diagnostics.exists():
                return 0
            text = diagnostics.read_text()
            if "UI rebuild failed:" in text or "UI reload failed after rebuild:" in text:
                raise RuntimeError(text)
            matches = re.findall(r"activated UI generation (\d+) ", text)
            return int(matches[-1]) if matches else 0

        wait_for(lambda: generation() >= 1 and navigation(), processes,
                 "Initial real host rebuild/reload did not activate", timeout=600)
        time.sleep(2)
        native("key", "--clearmodifiers", "Escape")
        time.sleep(.5)
        status = tool("status")
        assert status["mode"] == "desktop" and status["instance"]["pid"] == host.pid, status
        assert status["instance"]["profile"] == "debug", status
        report["checks"]["status"] = status
        inspected = tool("inspect", timeout_seconds=30)
        assert inspected["success"], inspected
        catalog = json.loads(inspected["stdout"])
        assert catalog["ok"] and {state["id"] for state in catalog["states"]} == EXPECTED_STATES, catalog
        report["checks"]["inspect_catalog"] = sorted(EXPECTED_STATES)
        window_before = native("search", "--onlyvisible", "--pid", host.pid).splitlines()
        assert window_before, "No visible private Desktop window"
        nav_before = navigation()
        phrase = "desktop selfdev draft survives reload"
        native("mousemove", 850, 950, "click", "1")
        native("type", "--clearmodifiers", "--delay", "30", phrase)
        time.sleep(.5)
        report["checks"]["draft_before"] = check_draft("before-reload", phrase)
        before = generation()
        ack = tool("reload")
        assert ack["acknowledged"] is True and ack["completed"] is False and ack["pid"] == host.pid, ack
        print("JCODE_CHECKPOINT " + json.dumps({"message": "Real tool reload acknowledged. Waiting for real Cargo generation"}), flush=True)
        wait_for(lambda: generation() > before, processes, "Tool reload did not activate a new generation", timeout=600)
        time.sleep(2)
        assert host.poll() is None
        window_after = native("search", "--onlyvisible", "--pid", host.pid).splitlines()
        assert window_after == window_before, (window_before, window_after)
        nav_after = navigation()
        sessions = lambda nav: [panel["session"] for row in nav["rows"] for panel in row["panels"]]
        assert sessions(nav_before) == sessions(nav_after), (nav_before, nav_after)
        report["checks"]["draft_after"] = check_draft("after-reload", phrase)
        report["checks"]["real_reload"] = {"ack": ack, "generation_before": before,
                                             "generation_after": generation(), "preserved_pid": host.pid,
                                             "preserved_windows": window_after, "preserved_sessions": sessions(nav_after)}
        screenshot_path = target / "desktop-selfdev/ui-review.png"
        screenshot = tool("screenshot", output=str(screenshot_path.relative_to(target)), timeout_seconds=600)
        assert screenshot["success"], screenshot
        assert screenshot_path.read_bytes().startswith(b"\x89PNG\r\n\x1a\n")
        from PIL import Image
        with Image.open(screenshot_path) as image:
            assert image.size == (1440, 1000), image.size
        report["checks"]["screenshot_fresh_build"] = {"path": str(screenshot_path), "size": [1440, 1000]}
        assert "--no-build" not in screenshot["args"], screenshot
        report["passed"] = True
        print(json.dumps(report, indent=2), flush=True)
    except BaseException as error:
        report["passed"] = False
        report["error"] = repr(error)
        raise
    finally:
        (root / "acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
        for process in reversed(processes):
            try:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=5)
            except ProcessLookupError:
                pass
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        for file in files:
            file.close()


if __name__ == "__main__":
    main()
