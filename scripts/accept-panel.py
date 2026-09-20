#!/usr/bin/env python3
"""Accept real panel tools -> harness API -> native Desktop documents.

Runs the actual panel tool through the daemon's debug control socket, not
injected UI events, fixture documents, or provider inference. All homes, sockets,
processes and X11 input are private. Uses current binaries without building.
Requires Xvfb, Openbox, xdotool, ImageMagick, tesseract and Mesa lavapipe.

Example: python3 scripts/accept-panel.py target/panel-accept \
    --binary /path/to/jcode-desktop --jcode-binary /path/to/jcode \
    --bridge /path/to/jcode-harness-api-bridge
"""
import argparse
import csv
import io
import re
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


def tool_payload(result):
    """Decode the actual debug tool envelope without mistaking text for JSON."""
    assert not result.get("is_error", False), result
    assert isinstance(result.get("output"), str), result
    snapshot = result.get("metadata")
    assert isinstance(snapshot, dict) and isinstance(snapshot.get("pages", []), list), result
    payload = {"panels": snapshot.get("pages", []), "snapshot": snapshot}
    # Only the header carries routing identity. Never parse user document text.
    lines = result["output"].splitlines()
    if lines and lines[0].startswith("panel_id: "):
        assert len(lines) >= 2 and lines[1].startswith("identity: "), result
        payload["panel_id"] = lines[0].removeprefix("panel_id: ").strip()
        payload["identity"] = lines[1].removeprefix("identity: ").strip()
    return payload


def check_listing(payload, expected):
    ids = [page["id"] for page in payload["panels"]]
    assert len(ids) == len(set(ids)), f"Duplicate persisted panel ids: {ids}"
    assert set(ids) == set(expected), (ids, expected)


def spawn_identity(payload, owner, existing=()):
    panel_id = payload.get("panel_id")
    assert isinstance(panel_id, str) and panel_id, payload
    identity = payload.get("identity")
    assert identity == f"side-panel://{owner}/{panel_id}", payload
    assert identity not in existing, "Spawn reused an existing identity"
    assert sum(page["id"] == panel_id for page in payload["panels"]) == 1, "Spawn missing from snapshot"
    return panel_id, identity


def same_entities(before, after):
    """Updates and toolbar actions must not recreate any existing panel."""
    expected = {p["session"]: p["id"] for p in panels(before)}
    actual = {p["session"]: p["id"] for p in panels(after)}
    assert len(expected) == len(panels(before)) and len(actual) == len(panels(after)), "Duplicate UI identities"
    assert expected == actual, (expected, actual)


def ocr_button_center(tsv, label, anchor=None):
    """Locate a unique visible word, never guess screen coordinates."""
    hits = []
    for row in csv.DictReader(io.StringIO(tsv), delimiter="\t", quoting=csv.QUOTE_NONE):
        if row["text"].strip().strip(".,").replace("−", "-").casefold() == label.casefold() and float(row["conf"]) >= 30:
            hits.append((int(row["left"]) + int(row["width"]) // 2,
                         int(row["top"]) + int(row["height"]) // 2))
    if anchor is not None:
        ax, ay = ocr_button_center(tsv, anchor)
        hits = [(x, y) for x, y in hits if abs(y - ay) <= 12 and abs(x - ax) <= 200]
    assert len(hits) == 1, f"Expected one visible {label!r} control, got {hits}"
    return hits[0]


def rendered_text_matches(text, present=(), absent=()):
    normalized = " ".join(text.casefold().split())
    return (all(" ".join(phrase.casefold().split()) in normalized for phrase in present)
            and all(" ".join(phrase.casefold().split()) not in normalized for phrase in absent))


def zoom_percent(text):
    values = re.findall(r"(?<![0-9])([0-9]{1,3})\s*%", text)
    assert len(values) == 1, f"Expected one PDF zoom status: {values}"
    return int(values[0])


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new evidence directory, never overwritten")
    parser.add_argument("--binary", type=Path, required=True, help="fresh Desktop binary")
    parser.add_argument("--zoom", action="store_true", help="also require OCR-targeted PDF zoom controls")
    parser.add_argument("--pdf", type=Path, default=repo / "assets/previews/pdf-preview.pdf")
    parser.add_argument("--jcode-binary", type=Path,
                        help="daemon binary, defaults to jcode from PATH")
    parser.add_argument("--bridge", type=Path,
                        help="fresh standalone bridge, otherwise uses jcode api-bridge")
    args = parser.parse_args()
    if not args.pdf.is_file() or not args.pdf.read_bytes().startswith(b"%PDF-"):
        parser.error("--pdf must be the two-page PDF acceptance fixture")
    args.pdf = args.pdf.resolve()
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
    report = {"passed": False, "scope": "real daemon panel tool -> real harness API bridge -> native Desktop panels and rendered pixels",
              "not_tested": ["provider/model invocation", "live user Desktop", "hot reload", "restart restoration"],
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

    def tool(action=None, **params):
        if action is not None:
            params["action"] = action
        params["intent"] = "Isolated real native panel acceptance"
        value = debug_request(root / "runtime/daemon-debug.sock",
                              "tool:panel " + json.dumps(params), session)
        report["tools"].append({"parameters": params, "response": value})
        (root / "tools.json").write_text(json.dumps(report["tools"], indent=2) + "\n")
        return tool_payload(value)

    def capture(label, phrases=(), text_check=None, absent=()):
        # OCR checks actual glyphs after presentation, not just retained state.
        path = root / f"{label}.png"
        def rendered():
            subprocess.run(["import", "-window", "root", "png:" + str(path)],
                           env=env, check=True, timeout=15)
            text = subprocess.check_output(["tesseract", str(path), "stdout", "--psm", "11"],
                                           env=env, stderr=subprocess.DEVNULL, timeout=30).decode()
            (root / f"{label}.txt").write_text(text)
            return (rendered_text_matches(text, phrases, absent)
                    and (text_check is None or text_check(text)))
        wait(rendered, "rendered pixels: " + label, timeout=25)
        report["checks"].append({"stage": label + "-pixels", "screenshot": str(path), "visible_phrases": list(phrases), "absent_phrases": list(absent)})
        return (root / f"{label}.txt").read_text()

    def click_word(label, stage):
        path = root / f"{stage}-control.png"
        def locate():
            subprocess.run(["import", "-window", "root", "png:" + str(path)],
                           env=env, check=True, timeout=15)
            tsv = subprocess.check_output(["tesseract", str(path), "stdout", "--psm", "11", "tsv"],
                                          env=env, stderr=subprocess.DEVNULL, timeout=30).decode()
            (root / f"{stage}-control.tsv").write_text(tsv)
            try:
                return ocr_button_center(tsv, label, anchor="Fit" if label in ("+", "-") else None)
            except AssertionError:
                return None
        x, y = wait(locate, "visible PDF control " + label, timeout=25)
        subprocess.run(["xdotool", "mousemove", "--sync", str(x), str(y), "click", "1"],
                       env=env, check=True, timeout=10)
        report["checks"].append({"stage": stage, "native_click": [x, y], "control": label})

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
        # First-ever spawn must not focus itself merely because no document was
        # previously selected. Check both persisted tool state and native focus.
        background = tool("spawn", title="Background notes", content=DOCUMENT, focus=False)
        background_id, background_identity = spawn_identity(background, session)
        assert background["snapshot"].get("focused_page_id") is None, background
        verify("first-spawn-without-focus", [session, background_identity], session)
        capture("first-spawn-without-focus")
        check_listing(tool("close", panel_id=background_id), [])
        verify("close-first-background", [session], session)
        # Omit action and focus to exercise spawn defaults, not just explicit flags.
        first_id, first = spawn_identity(tool(title="Acceptance notes", content=DOCUMENT), session, [background_identity])
        verify("spawn-markdown", [session, first], first)
        capture("spawn-markdown", ("Native document acceptance", "Structured Markdown"))
        pdf_id, pdf = spawn_identity(tool("spawn", title="PDF acceptance", file_path=str(args.pdf)),
                                     session, [first])
        both = verify("spawn-pdf", [session, first, pdf], pdf)
        capture("pdf-first-page", ("PDF panel acceptance", "Page 1 of 2"))
        replacement = "# Replaced evidence\n\nUpdated in place without stealing focus.\n"
        tool("update", panel_id=first_id, title="Updated notes", content=replacement)
        updated = verify("update-without-focus", [session, first, pdf], pdf)
        same_entities(both, updated)
        capture("update-keeps-pdf-visible", ("PDF panel acceptance", "Page 1 of 2"))
        click_word("Next", "pdf-next")
        same_entities(both, verify("pdf-second-page-state", [session, first, pdf], pdf))
        capture("pdf-second-page", ("Second PDF page", "Page 2 of 2"))
        click_word("Previous", "pdf-previous")
        text = capture("pdf-previous", ("PDF panel acceptance", "Page 1 of 2"))
        if args.zoom:
            before_zoom = zoom_percent(text)
            click_word("+", "pdf-zoom-in")
            text = capture("pdf-zoom-in", ("PDF panel acceptance", "Page 1 of 2"),
                           text_check=lambda text: zoom_percent(text) > before_zoom)
            assert zoom_percent(text) > before_zoom, "Zoom in did not increase scale"
            # OCR commonly recognizes the Unicode minus as ASCII '-'.
            click_word("-", "pdf-zoom-out")
            text = capture("pdf-zoom-out", ("PDF panel acceptance", "Page 1 of 2"),
                           text_check=lambda text: zoom_percent(text) == before_zoom)
            assert zoom_percent(text) == before_zoom, "Zoom out did not restore scale"
            click_word("Fit", "pdf-fit-width")
            capture("pdf-fit-width", ("PDF panel acceptance", "Page 1 of 2"))
        else:
            report["not_tested"].append("PDF zoom controls (enable --zoom)")
        tool("focus", panel_id=first_id)
        focused = verify("focus-markdown", [session, first, pdf], first)
        same_entities(both, focused)
        capture("updated-markdown", ("Replaced evidence", "Updated notes"))
        tool("update", panel_id=pdf_id, title="Renamed PDF", file_path=str(args.pdf))
        title_update = verify("pdf-update-without-focus", [session, first, pdf], first)
        same_entities(both, title_update)
        tool("focus", panel_id=pdf_id)
        verify("refocus-pdf", [session, first, pdf], pdf)
        capture("pdf-survives-file-update", ("PDF panel acceptance", "Renamed PDF"))
        # Valid header passes core transport validation, but Poppler must fail.
        # Updating the same visible entity must clear the prior rendered page.
        malformed = root / "malformed.pdf"
        malformed.write_bytes(b"%PDF-1.7\nnot a valid PDF document\n%%EOF\n")
        tool("update", panel_id=pdf_id, file_path=str(malformed))
        broken = verify("malformed-pdf-update", [session, first, pdf], pdf)
        same_entities(both, broken)
        capture("malformed-pdf-error", ("Cannot display this PDF",),
                absent=("PDF panel acceptance", "Second PDF page"))
        check_listing(tool("list"), [first_id, pdf_id])
        check_listing(tool("close", panel_id=pdf_id), [first_id])
        # Closing the selected tool panel returns keyboard focus to its owner.
        verify("close-pdf", [session, first], session)
        tool("focus", panel_id=first_id)
        verify("focus-before-identical-spawn", [session, first], first)
        # Same title AND content must still create independent document entities.
        duplicate_id, duplicate = spawn_identity(
            tool("spawn", title="Updated notes", content=replacement, focus=False),
            session, [first, pdf])
        twins = verify("same-title-spawn", [session, first, duplicate], first)
        assert panels(twins)[1]["id"] != panels(twins)[2]["id"], "Spawn reused a UI entity"
        check_listing(tool("list"), [first_id, duplicate_id])
        check_listing(tool("close", panel_id=duplicate_id), [first_id])
        verify("close-unfocused", [session, first], first)
        check_listing(tool("close", panel_id=first_id), [])
        verify("close-last", [session], session)
        capture("close-last")
        empty = tool("list")
        check_listing(empty, [])
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
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
        report["cleanup"] = [{"pid": p.pid, "exit_code": p.returncode} for p in processes]
        (root / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        for log in logs:
            log.close()
    print(json.dumps({"passed": report["passed"], "evidence": str(root)}), flush=True)


if __name__ == "__main__":
    main()
