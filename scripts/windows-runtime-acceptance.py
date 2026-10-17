#!/usr/bin/env python3
"""Native Windows acceptance of an extracted, real Desktop package.

Only Desktop is launched by the test. It must start its own daemon and bridge.
No credentials, offline screenshots, model calls, or globally installed jcode
are used. All child processes and state are scoped to this test directory.
"""
import argparse
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PureWindowsPath
import re
import subprocess
import sys
import time
import traceback


def pipe_name(path):
    """Match jcode-transport's Windows endpoint naming, including its hash."""
    stem = re.sub(r"[^A-Za-z0-9_-]", "", PureWindowsPath(path).stem)[:32] or "jcode"
    normalized = str(path).replace("\\", "/").translate(str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"))
    return rf"\\.\pipe\{stem}-{hashlib.sha256(normalized.encode()).hexdigest()[:16]}"


def wait_until(check, timeout=90, label="condition"):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (OSError, TimeoutError, RuntimeError) as error:
            last = error
        time.sleep(0.2)
    raise TimeoutError(f"Timed out waiting for {label}. Last error: {last}")


class Api:
    """Bounded NDJSON I/O using PeekNamedPipe, without third-party packages."""
    def __init__(self, path, transcript):
        import msvcrt
        self.file = open(pipe_name(path), "r+b", buffering=0)
        self.handle = msvcrt.get_osfhandle(self.file.fileno())
        self.buffer = b""
        self.sequence = 0
        self.transcript = transcript
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.PeekNamedPipe.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32,
                                            ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p]
        self.kernel.PeekNamedPipe.restype = ctypes.c_int

    def close(self):
        self.file.close()

    def receive(self, deadline):
        while time.monotonic() < deadline:
            if b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                frame = json.loads(line)
                self.transcript.append({"direction": "server", "frame": frame})
                return frame
            available = ctypes.c_uint32()
            if not self.kernel.PeekNamedPipe(self.handle, None, 0, None, ctypes.byref(available), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if available.value:
                data = self.file.read(min(available.value, 65536))
                if not data:
                    raise RuntimeError("Harness API stream closed")
                self.buffer += data
                if len(self.buffer) > 16 * 1024 * 1024:
                    raise RuntimeError("Harness API frame exceeded acceptance-test limit")
            else:
                time.sleep(0.02)
        raise TimeoutError("Harness API reply timed out")

    def request(self, req, expected, timeout=30, **fields):
        self.sequence += 1
        frame = {"v": 1, "id": self.sequence, "req": req, **fields}
        self.transcript.append({"direction": "client", "frame": frame})
        data = (json.dumps(frame, ensure_ascii=False) + "\n").encode()
        while data:
            count = self.file.write(data)
            if not count:
                raise RuntimeError("Harness API write made no progress")
            data = data[count:]
        deadline = time.monotonic() + timeout
        while True:
            reply = self.receive(deadline)
            if reply.get("reply_to") != self.sequence:
                continue
            if reply.get("ev") != expected:
                raise RuntimeError(f"{req}: expected {expected}, received {reply}")
            if reply.get("v") != 1:
                raise RuntimeError(f"Unexpected API version: {reply}")
            return reply

    def hello(self):
        reply = self.request("hello", "hello_ok", min_version=1, max_version=1,
                             client="windows-package-acceptance/1", supports_user_response=False)
        # A busy daemon can make the bridge's initial capability probe fail.
        # This connection cannot report user_connected even when Desktop is
        # attached. Reconnect rather than interpreting an absent field as false.
        if "session_connections" not in reply.get("capabilities", []):
            raise RuntimeError("Observer lacks session_connections; retrying capability probe")
        return reply


def powershell(script):
    result = subprocess.run(["pwsh", "-NoProfile", "-NonInteractive", "-Command", script],
                            capture_output=True, text=True, timeout=25, check=True)
    return result.stdout.strip()


def processes(package):
    # Query only exact executables from the tested package, never kill unrelated runtimes.
    raw = powershell("Get-CimInstance Win32_Process | Where-Object { $_.Name -in "
                     "@('jcode.exe','jcode-desktop.exe','jcode-harness-api-bridge.exe') } | "
                     "Select-Object ProcessId,ParentProcessId,Name,ExecutablePath,CommandLine | ConvertTo-Json -Compress")
    items = json.loads(raw) if raw else []
    if isinstance(items, dict):
        items = [items]
    return [item for item in items if item.get("ExecutablePath") and
            Path(item["ExecutablePath"]).parent == package]


def runtime_processes(package):
    items = processes(package)
    daemon = [p for p in items if p["Name"] == "jcode.exe" and re.search(r"\bserve\b", p["CommandLine"] or "")]
    bridge = [p for p in items if p["Name"] == "jcode-harness-api-bridge.exe" or
              (p["Name"] == "jcode.exe" and "api-bridge" in (p["CommandLine"] or ""))]
    if len(daemon) != 1 or len(bridge) != 1:
        raise RuntimeError(f"Expected one automatic daemon and bridge, got {items}")
    return daemon[0], bridge[0]


def stop(pid):
    powershell(f"Stop-Process -Id {int(pid)} -Force -ErrorAction SilentlyContinue")


def screenshot(destination):
    # Live Windows screen, not the app's offline fixture mode.
    escaped = str(destination).replace("'", "''")
    powershell("Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; "
               "$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen; "
               "$bitmap = New-Object System.Drawing.Bitmap $bounds.Width,$bounds.Height; "
               "$graphics = [System.Drawing.Graphics]::FromImage($bitmap); "
               "$graphics.CopyFromScreen($bounds.Location,[System.Drawing.Point]::Empty,$bounds.Size); "
               f"$bitmap.Save('{escaped}'); $graphics.Dispose(); $bitmap.Dispose()")


def foreground_app(pid, evidence):
    # Reuse only the Win32 ownership guard, never the fixture launch path.
    spec = importlib.util.spec_from_file_location(
        "windows_native_window_guard", Path(__file__).with_name("windows_screenshots.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.foreground_app(pid, evidence, timeout=15)


def run(package, output, timeout):
    output.mkdir(parents=True, exist_ok=True)
    home = output / "isolated user home"
    runtime = output / "isolated runtime"
    work = output / "workspace with spaces and Unicode 雪"
    for directory in (home, runtime, work):
        directory.mkdir()
    env = os.environ.copy()
    # Remove every inherited fixture or socket override. Never inherit user credentials.
    for key in list(env):
        if key.startswith("JCODE_") or key in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_STATE_HOME"):
            env.pop(key)
    env.update(JCODE_HOME=str(home / ".jcode"), JCODE_RUNTIME_DIR=str(runtime), HOME=str(home),
               USERPROFILE=str(home), APPDATA=str(home / "AppData/Roaming"),
               LOCALAPPDATA=str(home / "AppData/Local"), JCODE_NO_AUTO_UPDATE="1")
    # CLI discovery must work from packaged companions, not a preinstalled CLI on PATH.
    env["PATH"] = os.pathsep.join(part for part in env.get("PATH", "").split(os.pathsep)
                                 if not any(Path(part).joinpath(name).is_file() for name in ("jcode.exe", "jcode-harness-api-bridge.exe")))
    api_path = runtime / "jcode-api.sock"
    transcript = []
    report = {"package": str(package), "checks": [], "transcript": transcript}
    app = None
    app_window = None
    api = None
    logs = []
    cleanup_armed = False

    def passed(name, **evidence):
        print(f"PASS: {name}: {json.dumps(evidence, ensure_ascii=False)}", flush=True)
        report["checks"].append({"name": name, **evidence})
        (output / "result.json").write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")

    def launch(label, session_id=None):
        stdout = open(output / f"desktop-{label}.stdout.log", "wb")
        stderr = open(output / f"desktop-{label}.stderr.log", "wb")
        logs.extend([stdout, stderr])
        args = [str(package / "jcode-desktop.exe")]
        if session_id:
            args.append(f"--session={session_id}")
        return subprocess.Popen(args, cwd=work, env=env, stdout=stdout, stderr=stderr)

    def connect():
        if app.poll() is not None:
            raise AssertionError(f"Desktop exited early: {app.returncode}")
        connection = Api(api_path, transcript)
        try:
            hello = connection.hello()
            connection.request("ping", "pong")
        except Exception:
            connection.close()
            raise
        report["hello"] = hello
        return connection

    def capture(name):
        # Full desktop diagnostics count as app evidence only when its actual
        # foreground HWND belongs to this test's process before and after.
        app_window.require()
        screenshot(output / name)
        app_window.require()

    def close_desktop():
        nonlocal app_window
        app_window.request_close()
        app.wait(timeout=30)
        if app.returncode != 0:
            raise AssertionError(f"Desktop normal close failed: {app.returncode}")
        app_window.close()
        app_window = None
        passed("Desktop closed normally through its owned window", desktop_pid=app.pid)

    try:
        if processes(package):
            raise AssertionError("Package processes existed before cold start")
        passed("clean state, no pre-existing package processes, no offline fixtures")
        cleanup_armed = True
        app = launch("cold")
        api = wait_until(connect, timeout, "Desktop auto-started harness handshake")
        daemon, bridge = wait_until(lambda: runtime_processes(package), timeout, "single daemon and bridge")
        passed("cold-start launches daemon and harness automatically", desktop_pid=app.pid,
               daemon=daemon, bridge=bridge, hello=report["hello"])
        app_window = foreground_app(app.pid, output / "foreground-cold.json")
        capture("cold-start.png")
        listed = api.request("list_sessions", "sessions")["sessions"]
        passed("real harness session listing", sessions=listed)
        session = api.request("create_session", "attached", working_dir=str(work))["session"]
        session_id = session["session_id"]
        if Path(session.get("working_dir") or "").resolve() != work.resolve():
            raise AssertionError(f"Session ignored Unicode working directory: {session}")
        passed("create and attach real session in Unicode workspace", session=session)
        api.request("get_history", "history", session_id=session_id)
        api.request("send_message", "ok", session_id=session_id,
                    content="Windows native acceptance: persisted context, no provider call.", no_reply=True)
        history = api.request("get_history", "history", session_id=session_id)
        if "Windows native acceptance" not in json.dumps(history):
            raise AssertionError(f"Context message not persisted: {history}")
        passed("persist context and fetch real history without credentials")
        api.request("detach_session", "ok", session_id=session_id)
        api.close()
        api = None
        # Open the persisted session through the real Desktop UI before failure
        # injection. A signed-out onboarding window need not hold a live session.
        close_desktop()
        app = launch("attached", session_id)
        app_window = foreground_app(app.pid, output / "foreground-attached.json")
        api = wait_until(connect, timeout, "Desktop opens the requested real session")
        def desktop_attached():
            sessions = api.request("list_sessions", "sessions")["sessions"]
            return next((session for session in sessions if session["session_id"] == session_id
                         and session.get("user_connected")), None)
        attached = wait_until(desktop_attached, timeout, "Desktop UI attached before failure injection")
        passed("Desktop UI attaches before disconnect and recovery checks", session=attached)
        capture("desktop-attached.png")
        api.close()
        api = None
        # Repeated connect-close readiness probes must never bring down the server.
        for endpoint in (api_path, runtime / "jcode.sock"):
            for _ in range(20):
                def probe():
                    with open(pipe_name(endpoint), "r+b", buffering=0):
                        return True
                wait_until(probe, 10, "available named-pipe readiness probe")
                time.sleep(0.025)
        api = wait_until(connect, timeout, "harness after abandoned readiness probes")
        api.request("attach_session", "attached", session_id=session_id)
        passed("daemon and bridge survive forty connect-close probes and reattach")
        api.request("detach_session", "ok", session_id=session_id)
        api.close()
        api = None
        current_daemon, current_bridge = runtime_processes(package)
        if (current_daemon["ProcessId"], current_bridge["ProcessId"]) != (daemon["ProcessId"], bridge["ProcessId"]):
            raise AssertionError("Readiness probes restarted the runtime instead of surviving")
        killed_pid = current_bridge["ProcessId"]
        stop(killed_pid)
        wait_until(lambda: not any(p["ProcessId"] == killed_pid for p in processes(package)),
                   15, "injected bridge termination")
        passed("failure injection terminated the live bridge", killed_pid=killed_pid)
        # Desktop, not the test, must revive the bridge. No CLI is manually started.
        api = wait_until(connect, timeout, "Desktop revives killed bridge")
        new_daemon, new_bridge = runtime_processes(package)
        if new_daemon["ProcessId"] != daemon["ProcessId"] or new_bridge["ProcessId"] == killed_pid:
            raise AssertionError("Bridge recovery replaced the daemon or did not replace the bridge")
        attached = wait_until(desktop_attached, timeout, "Desktop UI reconnects after bridge termination")
        passed("Desktop UI reconnects after bridge termination", session=attached)
        api.request("attach_session", "attached", session_id=session_id)
        recovered = api.request("get_history", "history", session_id=session_id)
        if "Windows native acceptance" not in json.dumps(recovered):
            raise AssertionError("History lost during bridge recovery")
        passed("Desktop automatically revives bridge, preserving daemon and history", bridge=new_bridge)
        api.request("detach_session", "ok", session_id=session_id)
        api.close()
        api = None
        close_desktop()
        app = launch("warm", session_id)
        app_window = foreground_app(app.pid, output / "foreground-warm.json")
        api = wait_until(connect, timeout, "Desktop warm relaunch")
        warm_daemon, warm_bridge = runtime_processes(package)
        if (warm_daemon["ProcessId"], warm_bridge["ProcessId"]) != (daemon["ProcessId"], new_bridge["ProcessId"]):
            raise AssertionError("Warm relaunch spawned duplicate or replacement runtime")
        attached = wait_until(desktop_attached, timeout, "Desktop UI attached to requested session")
        passed("Desktop UI attaches to requested session on warm relaunch", session=attached)
        api.request("attach_session", "attached", session_id=session_id)
        history = api.request("get_history", "history", session_id=session_id)
        if "Windows native acceptance" not in json.dumps(history):
            raise AssertionError("History missing after Desktop UI warm relaunch")
        api.request("detach_session", "ok", session_id=session_id)
        if app.poll() is not None:
            raise AssertionError(f"Desktop exited during warm session verification: {app.returncode}")
        attached = wait_until(desktop_attached, timeout, "Desktop remains attached without observer")
        passed("Desktop warm relaunch reuses existing runtime and session", session=attached)
        capture("warm-relaunch.png")
        report["success"] = True
    except Exception as error:
        report["success"] = False
        report["error"] = str(error)
        traceback.print_exc()
        try:
            screenshot(output / "failure.png")
        except Exception as capture_error:
            report["screenshot_error"] = str(capture_error)
        raise
    finally:
        cleanup_errors = []
        def cleanup(label, action):
            try:
                return action()
            except Exception as error:
                cleanup_errors.append(f"{label}: {error}")
                return None
        if api:
            cleanup("close API", api.close)
        if app_window:
            cleanup("release app-only foreground guard", app_window.close)
        if app and app.poll() is None:
            cleanup("terminate Desktop", app.terminate)
            cleanup("wait for Desktop", lambda: app.wait(timeout=15))
        report["final_processes"] = cleanup("process census", lambda: processes(package)) or []
        if cleanup_armed:
            for process in report["final_processes"]:
                cleanup("stop package process", lambda: stop(process["ProcessId"]))
            cleanup("verify package shutdown", lambda: wait_until(
                lambda: not processes(package), 15, "all test package processes stopped"))
        for log in logs:
            cleanup("close log", log.close)
        report["cleanup_errors"] = cleanup_errors
        cleanup_failed = bool(report.get("success") and cleanup_errors)
        if cleanup_failed:
            report["success"] = False
            report["error"] = f"Acceptance cleanup failed: {cleanup_errors}"
        (output / "result.json").write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
        if cleanup_failed:
            raise RuntimeError(report["error"])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--timeout", type=int, default=90)
    args = parser.parse_args(argv)
    if sys.platform != "win32":
        parser.error("This acceptance test requires native Windows")
    run(args.package.resolve(), args.output.resolve(), args.timeout)


if __name__ == "__main__":
    main()
