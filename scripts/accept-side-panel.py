#!/usr/bin/env python3
"""Accept real side_panel tools -> harness API -> native Desktop documents.

Runs the actual side_panel tool through the daemon's debug control socket, not
injected UI events, fixture documents, or provider inference. All homes, sockets,
processes and X11 input are private. Uses current binaries without building.
Requires Xvfb, Openbox, xdotool, ImageMagick, tesseract and Mesa lavapipe.

Example: python3 scripts/accept-side-panel.py target/side-panel-accept \
    --bridge /path/to/jcode-harness-api-bridge
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import time

from screenshot import isolated_env


DOCUMENT = """# Native document acceptance

This document lives beside the chat.

## Structured Markdown

- **Bold checklist item**
- A second readable item

```python
print("native document")
```

| Feature | Result |
| --- | --- |
| Markdown | Native |
"""


def navigation_state(path):
    try:
        line = next(line for line in path.read_text().splitlines()
                    if line.startswith("navigation="))
        return json.loads(line.partition("=")[2])
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


def panels(state):
    return [panel for row in state.get("rows", []) for panel in row["panels"]]


def check_state(state, identities, focused):
    """Assert real panel identity, adjacency, stable chat and keyboard focus."""
    assert state is not None, "No native navigation state"
    actual = panels(state)
    assert [panel["session"] for panel in actual] == identities, actual
    rows = [row for row in state["rows"] if row["panels"]]
    assert len(rows) == 1, "Documents must remain beside their owning chat"
    assert not any(panel["closing"] for panel in actual), actual
    selected = [panel for panel in actual if panel["focused"]]
    assert len(selected) == 1 and selected[0]["session"] == focused, selected
    assert state["keyboard_panel"] == selected[0]["slot"], state
    assert state["focused_slot"] == selected[0]["slot"], state
    assert not state["camera_motion"] and not state["tab_motion"], "Unsettled geometry"
    return state


def debug_request(path, command, session):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(30)
        client.connect(str(path))
        client.sendall(json.dumps({"type": "debug_command", "id": 1,
                                  "command": command, "session_id": session}).encode() + b"\n")
        with client.makefile("rb") as reader:
            reply = json.loads(reader.readline(2 * 1024 * 1024))
    assert reply.get("type") == "debug_response" and reply.get("ok"), reply
    result = json.loads(reply["output"])
    assert not result.get("is_error", False), result
    return result


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new evidence directory, never overwritten")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--jcode-binary", type=Path,
                        help="daemon binary, defaults to jcode from PATH")
    parser.add_argument("--bridge", type=Path,
                        help="fresh standalone bridge, otherwise uses jcode api-bridge")
    args = parser.parse_args()
    daemon = args.jcode_binary or (Path(shutil.which("jcode")) if shutil.which("jcode") else None)
    if daemon is None:
        parser.error("jcode is missing, supply --jcode-binary")
    for name, binary in (("Desktop", args.binary), ("daemon", daemon), ("bridge", args.bridge)):
        if binary is not None and (not binary.is_file() or not os.access(binary, os.X_OK)):
            parser.error(f"{name} binary is not executable: {binary}")
    binary, daemon = args.binary.resolve(), daemon.resolve()
    bridge = args.bridge.resolve() if args.bridge else None
    root = args.output.resolve()
    if len(str(root / "runtime/daemon-debug.sock").encode()) >= 104:
        parser.error("output path is too long for private Unix sockets")
    if root.exists():
        parser.error("refusing to overwrite existing evidence directory")
    for executable in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(executable, path="/usr/bin:/bin"):
            parser.error("missing executable: " + executable)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe Vulkan driver is required")
    root.mkdir(parents=True, mode=0o700)
    for directory in ("home", "runtime", "config", "cache", "data", "jcode", "logs", "shims"):
        (root / directory).mkdir(mode=0o700)
    env = isolated_env(root)
    env.pop("JCODE_DESKTOP_SCREENSHOT")  # Real sessions and API, never screenshot fixtures.
    env.update({
        "JCODE_RUNTIME_DIR": str(root / "runtime"),
        "JCODE_SOCKET": str(root / "runtime/daemon.sock"),
        "JCODE_API_SOCKET": str(root / "runtime/api.sock"),
        "JCODE_DEBUG_CONTROL": "1", "JCODE_NO_TELEMETRY": "1",
        "JCODE_TEMP_SERVER": "1", "JCODE_SERVER_OWNER_PID": str(os.getpid()),
        "JCODE_TEMP_SERVER_IDLE_SECS": "300",
        "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
        "VK_DRIVER_FILES": str(drivers[0]),
        "PATH": str(root / "shims") + ":/usr/bin:/bin",
    })
    # Explicit startup below owns both runtime processes. This shim only lets
    # Desktop resolve the same executable during SDK runtime readiness checks.
    (root / "shims/jcode").symlink_to(daemon)
    (root / "jcode/config.toml").write_text("[features]\nmemory = false\n")
    (root / "desktop.toml").write_text(
        '[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n'
        '[workspace]\ncoaching_hints = false\n')
    wm = root / "openbox.xml"
    wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                  '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                  '</application></applications></openbox_config>')
    processes, logs = [], []
    report = {"passed": False, "scope": "real daemon side_panel tool -> real harness API bridge -> native Desktop panels and rendered pixels",
              "not_tested": ["provider/model invocation", "live user Desktop", "hot reload"],
              "binary": str(binary), "daemon": str(daemon), "bridge": str(bridge) if bridge else "jcode api-bridge",
              "checks": [], "tools": []}
    state_path = root / "state"

    def launch(name, command, **kwargs):
        log = (root / f"{name}.log").open("w")
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log,
                                   start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait(predicate, label, timeout=60):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            exited = [(process.args, process.poll()) for process in processes if process.poll() is not None]
            assert not exited, f"Child exited during {label}: {exited}"
            result = predicate()
            if result:
                return result
            time.sleep(.1)
        raise AssertionError(f"Timed out: {label}; navigation={navigation_state(state_path)}")

    def ready_socket(path):
        try:
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(.2)
                client.connect(str(path))
            return True
        except OSError:
            return False

    def verify(label, identities, focused):
        def ready():
            try:
                return check_state(navigation_state(state_path), identities, focused)
            except AssertionError:
                return False
        state = wait(ready, label, timeout=20)
        report["checks"].append({"stage": label, "navigation": state})
        (root / f"{label}.json").write_text(json.dumps(state, indent=2) + "\n")
        print("JCODE_CHECKPOINT " + json.dumps({"message": label}), flush=True)
        return state

    def tool(action, **params):
        params.update(action=action, intent="Isolated real native side-panel acceptance")
        value = debug_request(root / "runtime/daemon-debug.sock",
                              "tool:side_panel " + json.dumps(params), session)
        report["tools"].append({"parameters": params, "response": value})
        (root / "tools.json").write_text(json.dumps(report["tools"], indent=2) + "\n")
        return value

    def capture(label, phrases=()):
        # OCR checks actual glyphs after presentation, not just retained state.
        path = root / f"{label}.png"
        def rendered():
            subprocess.run(["import", "-window", "root", "png:" + str(path)],
                           env=env, check=True, timeout=15)
            text = subprocess.check_output(["tesseract", str(path), "stdout", "--psm", "11"],
                                           env=env, stderr=subprocess.DEVNULL, timeout=30).decode()
            (root / f"{label}.txt").write_text(text)
            return all(phrase.lower() in " ".join(text.lower().split()) for phrase in phrases)
        wait(rendered, "rendered pixels: " + label, timeout=25)
        report["checks"].append({"stage": label + "-pixels", "screenshot": str(path), "visible_phrases": list(phrases)})

    read_fd, write_fd = os.pipe()
    try:
        launch("daemon", [str(daemon), "--no-update", "--no-selfdev", "--provider", "jcode", "serve"])
        wait(lambda: ready_socket(root / "runtime/daemon-debug.sock"), "private daemon debug socket")
        launch("bridge", [str(bridge)] if bridge else [str(daemon), "--no-update", "--no-selfdev", "api-bridge"])
        wait(lambda: ready_socket(env["JCODE_API_SOCKET"]), "private API socket")
        launch("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1800x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0], "Xvfb startup timeout"
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit(), display
        env["DISPLAY"] = ":" + display
        launch("wm", ["openbox", "--sm-disable", "--config-file", str(wm)])
        time.sleep(.5)
        launch("app", [str(binary), "--no-hot-reload"])
        state = wait(lambda: (state if (state := navigation_state(state_path)) and
                             any(panel["session"].startswith("session_") for panel in panels(state))
                             else None), "Desktop real session attachment", timeout=90)
        session = next(panel["session"] for panel in panels(state)
                       if panel["session"].startswith("session_"))
        # Current real first launch shows a beta overlay and release notes.
        # Dismiss them as a user would, only on this run's private X server.
        subprocess.run(["xdotool", "search", "--sync", "--onlyvisible", "--class",
                        "^jcode-desktop$", "windowactivate", "--sync"],
                       env=env, check=True, timeout=10)
        for _ in range(3):
            subprocess.run(["xdotool", "key", "--clearmodifiers", "Escape"],
                           env=env, check=True, timeout=10)
            time.sleep(.2)
        report["session_id"] = session
        verify("initial", [session], session)
        first, second = f"side-panel://{session}/acceptance", f"side-panel://{session}/secondary"
        tool("write", page_id="acceptance", title="Acceptance notes", content=DOCUMENT)
        state = verify("write", [session, first], first)
        document_entity = panels(state)[1]["id"]
        capture("write", ("Native document acceptance", "Structured Markdown", "read-only"))
        tool("append", page_id="acceptance", content="\n\n## Appended evidence\n\nLive updates reached the native document.\n")
        state = verify("append", [session, first], first)
        assert panels(state)[1]["id"] == document_entity, "Append must update the same panel"
        capture("append", ("Appended evidence", "Native document acceptance"))
        tool("write", page_id="secondary", title="Second document", content="# Secondary evidence\n\nA separate native page.\n", focus=False)
        verify("write-without-focus", [session, first, second], first)
        tool("focus", page_id="secondary")
        verify("focus", [session, first, second], second)
        capture("focus", ("Secondary evidence", "Second document"))
        tool("delete", page_id="secondary")
        verify("delete-secondary", [session, first], first)
        tool("write", page_id="acceptance", title="Updated notes", content="# Replaced evidence\n\nUpdated in place without another chat.\n")
        state = verify("rewrite", [session, first], first)
        assert panels(state)[1]["id"] == document_entity, "Rewrite must not duplicate the document"
        capture("rewrite", ("Replaced evidence", "Updated notes"))
        tool("delete", page_id="acceptance")
        verify("delete-last", [session], session)
        capture("delete-last")
        tool("status")
        report["passed"] = True
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        for process in reversed(processes):
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for process in reversed(processes):
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
        report["cleanup"] = [{"pid": p.pid, "exit_code": p.returncode} for p in processes]
        (root / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        for log in logs:
            log.close()
    print(json.dumps({"passed": report["passed"], "evidence": str(root)}), flush=True)


if __name__ == "__main__":
    main()
