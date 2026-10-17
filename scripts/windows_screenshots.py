#!/usr/bin/env python3
"""Render Jcode Desktop on native Windows with offline fixtures and capture PNGs.

The Windows counterpart of scripts/screenshot.py, used by the on-demand
`Windows screenshots` workflow. Each state launches the real app in an
isolated profile with the same JCODE_DESKTOP_SCREENSHOT_* fixture switches,
waits for the first rendered frame (reported through JCODE_DESKTOP_STATE),
and captures the app window with Pillow.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

# name: (fixture environment, theme, dismiss launch overlay)
STATES = {
    "chat": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all"}, "warm-neutral", True),
    "empty": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "empty"}, "warm-neutral", True),
    "launch-overlay": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "empty"}, "warm-neutral", False),
    "dark-theme": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all"}, "neutral-dark", True),
    "three-panels": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "JCODE_DESKTOP_SCREENSHOT_PANELS": "3"}, "warm-neutral", True),
    "diff": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "diff-rich"}, "warm-neutral", True),
    "tools": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "tool-icons"}, "warm-neutral", True),
    "streaming": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "streaming"}, "warm-neutral", True),
    "mermaid": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "mermaid"}, "warm-neutral", True),
    "applet": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "applet"}, "warm-neutral", True),
    "todos": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "todos"}, "warm-neutral", True),
    "swarm-sidebar": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "JCODE_DESKTOP_SCREENSHOT_SWARM": "1"}, "warm-neutral", True),
    "accounts": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "JCODE_DESKTOP_SCREENSHOT_ACCOUNTS": "1"}, "warm-neutral", False),
    "account-menu": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "JCODE_DESKTOP_SCREENSHOT_ACCOUNT_MENU": "1"}, "warm-neutral", True),
    "changelog": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "JCODE_DESKTOP_SCREENSHOT_CHANGELOG": "1"}, "warm-neutral", False),
    "error-state": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "JCODE_DESKTOP_SCREENSHOT_PREVIEW_STATE": "disconnected"}, "warm-neutral", True),
    # Caption acceptance: hovers, real maximize/restore click, and drag.
    "caption": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all"}, "warm-neutral", True),
    "caption-light": ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all"}, "neutral-light", True),
}

CAPTION_STATES = {"caption", "caption-light"}
# Issue #5 input latency: the reporter's synthetic move/click loop, measured
# with a SendMessageTimeout round trip that approximates UI-thread stalls.
# input-latency-profile also flushes the GPU after every batch to attribute
# time to primitive kinds. That inflates latency, so compare without it.
LATENCY_STATES = {"input-latency", "input-latency-profile"}
STATES["input-latency"] = ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "RUST_LOG": "info"}, "warm-neutral", True)
STATES["input-latency-profile"] = ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "RUST_LOG": "info", "GPUI_DX_PROFILE": "1"}, "warm-neutral", True)
# Issue #5 Windows integration: diagnostics log, GUI subsystem, single instance.
INTEGRATION_STATES = {"windows-integration"}
STATES["windows-integration"] = ({"JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "all", "RUST_LOG": "debug"}, "warm-neutral", True)

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
user32.ShowWindow.restype = wintypes.BOOL
for _window_query in ("IsWindow", "IsWindowVisible", "IsIconic"):
    getattr(user32, _window_query).argtypes = [wintypes.HWND]
    getattr(user32, _window_query).restype = wintypes.BOOL
EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.GetForegroundWindow.restype = wintypes.HWND
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
user32.SetForegroundWindow.restype = wintypes.BOOL
user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                               ctypes.c_int, ctypes.c_int, wintypes.UINT]
user32.SetWindowPos.restype = wintypes.BOOL
user32.GetWindowLongW.argtypes = [wintypes.HWND, ctypes.c_int]
user32.GetWindowLongW.restype = wintypes.LONG
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.PostMessageW.restype = wintypes.BOOL
VK_ESCAPE = 0x1B
KEYEVENTF_KEYUP = 0x2


def set_resolution(width, height):
    """Best effort: hosted runners start at 1024x768."""
    class DEVMODE(ctypes.Structure):
        _fields_ = [("dmDeviceName", wintypes.WCHAR * 32), ("dmSpecVersion", wintypes.WORD),
                    ("dmDriverVersion", wintypes.WORD), ("dmSize", wintypes.WORD),
                    ("dmDriverExtra", wintypes.WORD), ("dmFields", wintypes.DWORD),
                    ("dmPositionX", wintypes.LONG), ("dmPositionY", wintypes.LONG),
                    ("dmDisplayOrientation", wintypes.DWORD), ("dmDisplayFixedOutput", wintypes.DWORD),
                    ("dmColor", ctypes.c_short), ("dmDuplex", ctypes.c_short),
                    ("dmYResolution", ctypes.c_short), ("dmTTOption", ctypes.c_short),
                    ("dmCollate", ctypes.c_short), ("dmFormName", wintypes.WCHAR * 32),
                    ("dmLogPixels", wintypes.WORD), ("dmBitsPerPel", wintypes.DWORD),
                    ("dmPelsWidth", wintypes.DWORD), ("dmPelsHeight", wintypes.DWORD),
                    ("dmDisplayFlags", wintypes.DWORD), ("dmDisplayFrequency", wintypes.DWORD),
                    ("dmICMMethod", wintypes.DWORD), ("dmICMIntent", wintypes.DWORD),
                    ("dmMediaType", wintypes.DWORD), ("dmDitherType", wintypes.DWORD),
                    ("dmReserved1", wintypes.DWORD), ("dmReserved2", wintypes.DWORD),
                    ("dmPanningWidth", wintypes.DWORD), ("dmPanningHeight", wintypes.DWORD)]
    mode = DEVMODE()
    mode.dmSize = ctypes.sizeof(DEVMODE)
    if not user32.EnumDisplaySettingsW(None, -1, ctypes.byref(mode)):
        return False
    mode.dmPelsWidth, mode.dmPelsHeight = width, height
    mode.dmFields = 0x80000 | 0x100000  # DM_PELSWIDTH | DM_PELSHEIGHT
    return user32.ChangeDisplaySettingsW(ctypes.byref(mode), 0) == 0


def windows_of(pid):
    found = []

    def callback(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            rect = wintypes.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
            if rect.right - rect.left > 200 and rect.bottom - rect.top > 200:
                found.append(hwnd)
        return True

    user32.EnumWindows(EnumWindowsProc(callback), 0)
    return found


class ForegroundApp:
    """Temporarily raise only the tested app, never dismiss other applications."""
    def __init__(self, pid, hwnd, evidence):
        self.pid, self.hwnd, self.evidence = pid, hwnd, evidence
        self.was_topmost = bool(user32.GetWindowLongW(hwnd, -20) & 8)
        self.closed = False

    def require(self):
        owner = wintypes.DWORD()
        foreground = user32.GetForegroundWindow()
        user32.GetWindowThreadProcessId(foreground, ctypes.byref(owner))
        data = {"pid": self.pid, "hwnd": self.hwnd, "foreground": foreground,
                "foreground_pid": owner.value,
                "visible": bool(user32.IsWindowVisible(self.hwnd)),
                "minimized": bool(user32.IsIconic(self.hwnd))}
        self.evidence.write_text(json.dumps(data, indent=2), encoding="utf-8")
        if (owner.value != self.pid or foreground != self.hwnd
                or not data["visible"] or data["minimized"]
                or self.hwnd not in windows_of(self.pid)):
            raise RuntimeError(f"Tested app is not the foreground window: {data}")

    def request_close(self):
        if self.hwnd not in windows_of(self.pid):
            raise RuntimeError("Refusing WM_CLOSE for an unowned window")
        if not user32.PostMessageW(self.hwnd, 0x0010, 0, 0):  # WM_CLOSE
            raise ctypes.WinError(ctypes.get_last_error())

    def close(self):
        if not self.closed and not self.was_topmost and self.hwnd in windows_of(self.pid):
            user32.SetWindowPos(self.hwnd, -2, 0, 0, 0, 0, 0x13)  # HWND_NOTOPMOST, no activation
        self.closed = True


def foreground_app(pid, evidence, timeout=15):
    deadline = time.monotonic() + timeout
    guard = None
    try:
        while time.monotonic() < deadline:
            windows = windows_of(pid)
            if windows:
                if guard is None:
                    guard = ForegroundApp(pid, windows[0], evidence)
                user32.ShowWindow(guard.hwnd, 9)  # SW_RESTORE, app only
                user32.SetWindowPos(guard.hwnd, -1, 0, 0, 0, 0, 0x43)  # HWND_TOPMOST, show
                user32.SetForegroundWindow(guard.hwnd)
                try:
                    guard.require()
                    return guard
                except RuntimeError:
                    pass
            time.sleep(0.2)
        raise RuntimeError(f"Cannot foreground tested app PID {pid}; see {evidence}")
    except BaseException:
        if guard:
            guard.close()
        raise


def press_escape(hwnd):
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.2)
    user32.keybd_event(VK_ESCAPE, 0, 0, 0)
    user32.keybd_event(VK_ESCAPE, 0, KEYEVENTF_KEYUP, 0)


MOUSEEVENTF_LEFTDOWN = 0x2
MOUSEEVENTF_LEFTUP = 0x4


def window_rect(hwnd):
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    return rect.left, rect.top, rect.right, rect.bottom


def hit_test(hwnd, x, y):
    """Ask the window what Windows sees at a screen point (WM_NCHITTEST).

    GPUI answers from its last mouse hit test, so move the real cursor there
    first and let a frame observe it, as a user's pointer would.
    """
    user32.SetCursorPos(x, y)
    time.sleep(0.5)
    user32.SendMessageW.restype = ctypes.c_ssize_t
    user32.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    lparam = (y & 0xFFFF) << 16 | (x & 0xFFFF)
    return user32.SendMessageW(hwnd, 0x0084, 0, lparam)


def grab(box, path):
    from PIL import ImageGrab
    ImageGrab.grab(bbox=box, all_screens=True).save(path)


def caption_checks(name, hwnd, out_dir):
    """Verify the app-drawn caption behaves like a native Windows caption."""
    HTCAPTION, HTMINBUTTON, HTMAXBUTTON, HTCLOSE = 2, 8, 9, 20
    scale = user32.GetDpiForWindow(hwnd) / 96.0
    report = []

    def check(label, ok, detail=""):
        report.append(f"{'PASS' if ok else 'FAIL'} {label} {detail}".rstrip())

    # Restore to a floating window first so maximize and drag are observable.
    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    time.sleep(1.5)
    user32.SetWindowPos(hwnd, None, 120, 80, 1500, 900, 0x4)  # SWP_NOZORDER
    time.sleep(1.5)
    left, top, right, bottom = window_rect(hwnd)
    cy = top + int(16 * scale)
    close_x = right - int(23 * scale) - 8
    max_x = right - int(69 * scale) - 8
    min_x = right - int(115 * scale) - 8
    for label, x, want in (("close button", close_x, HTCLOSE), ("maximize button", max_x, HTMAXBUTTON),
                           ("minimize button", min_x, HTMINBUTTON),
                           ("empty header drags", right - int(260 * scale), HTCAPTION)):
        got = hit_test(hwnd, x, cy)
        check(f"hit-test {label}", got == want, f"got={got} want={want}")
    # The version pill, left of the tab row, is an interactive header control.
    pill_hit = hit_test(hwnd, left + 8 + int(260 * scale), cy)
    check("header control is clickable, not caption", pill_hit != HTCAPTION, f"got={pill_hit}")
    body_hit = hit_test(hwnd, left + (right - left) // 2, top + (bottom - top) // 2)
    check("content is client area", body_hit == 1, f"got={body_hit}")

    strip = lambda: (left, top, right, top + int(60 * scale))  # noqa: E731
    grab(strip(), out_dir / f"{name}-strip.png")
    for label, x in (("hover-close", close_x), ("hover-max", max_x)):
        user32.SetCursorPos(x, cy)
        time.sleep(0.8)
        grab((right - int(400 * scale), top, right, top + int(60 * scale)), out_dir / f"{name}-{label}.png")

    # Drag the window by the empty caption area.
    user32.SetCursorPos(right - int(260 * scale), cy)
    time.sleep(0.3)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    for step in range(1, 11):
        user32.SetCursorPos(right - int(260 * scale) + 8 * step, cy + 6 * step)
        time.sleep(0.05)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
    time.sleep(0.8)
    moved = window_rect(hwnd)
    check("drag moves window", moved[0] - left >= 40 and moved[1] - top >= 30, f"from={left, top} to={moved[:2]}")

    # Click the real maximize button, then restore with it.
    left, top, right, bottom = moved
    max_x = right - int(69 * scale) - 8
    user32.SetCursorPos(max_x, top + int(16 * scale))
    time.sleep(0.3)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
    time.sleep(1.5)
    check("maximize button maximizes", bool(user32.IsZoomed(hwnd)))
    left, top, right, bottom = window_rect(hwnd)
    grab((max(left, 0), max(top, 0), right, top + int(60 * scale)), out_dir / f"{name}-maximized-strip.png")
    user32.SetCursorPos(right - int(69 * scale) - 8, max(top, 0) + int(16 * scale))
    time.sleep(0.3)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
    time.sleep(1.5)
    check("maximize button restores", not user32.IsZoomed(hwnd))
    # Snap Layouts: Windows 11 opens a flyout owned by explorer when the
    # pointer rests on an HTMAXBUTTON region. Report whether it appeared.
    left, top, right, bottom = window_rect(hwnd)
    user32.SetCursorPos(right - int(69 * scale) - 8, top + int(16 * scale))
    time.sleep(2.5)
    grab((max(right - int(700 * scale), 0), top, right, top + int(400 * scale)), out_dir / f"{name}-snap-hover.png")
    flyouts = []

    def find(hwnd_, _):
        buf = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd_, buf, 256)
        title = ctypes.create_unicode_buffer(256)
        user32.GetWindowTextW(hwnd_, title, 256)
        if user32.IsWindowVisible(hwnd_) and ("Snap" in title.value or "SnapAssist" in buf.value
                                              or "Xaml" in buf.value and "Snap" in title.value):
            flyouts.append(f"{buf.value}:{title.value}")
        return True

    user32.EnumWindows(EnumWindowsProc(find), 0)
    import platform
    report.append(f"INFO snap-layouts flyout={flyouts or 'none'} os={platform.platform()}")
    user32.SetCursorPos(5, 5)
    time.sleep(0.5)
    (out_dir / f"{name}-checks.txt").write_text("\n".join(report) + "\n")
    print("\n".join(report))
    return all(line.startswith(("PASS", "INFO")) for line in report)


def input_latency(name, hwnd, out_dir, root):
    """Replay the issue #5 automation (moves only, moves + clicks, clicks only)
    and report UI-thread round-trip latency plus the app's slow-message log."""
    import json
    import math
    SMTO_ABORTIFHUNG = 0x2
    user32.SendMessageTimeoutW.restype = ctypes.c_ssize_t
    user32.SendMessageTimeoutW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM,
                                           wintypes.UINT, wintypes.UINT, ctypes.POINTER(ctypes.c_size_t)]

    def ping():
        result = ctypes.c_size_t()
        started = time.perf_counter()
        ok = user32.SendMessageTimeoutW(hwnd, 0, 0, 0, SMTO_ABORTIFHUNG, 2000, ctypes.byref(result))
        return bool(ok), (time.perf_counter() - started) * 1000

    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    time.sleep(1)
    user32.SetWindowPos(hwnd, None, 100, 60, 1400, 900, 0x4)
    time.sleep(1.5)
    left, top, right, bottom = window_rect(hwnd)
    cx, cy = (left + right) // 2, (top + bottom) // 2
    report = {}
    for mode, radius, clicks in (("moves", 200, False), ("moves+clicks", 200, True), ("clicks", 0, True)):
        samples = []
        started = time.monotonic()
        step = 0
        while time.monotonic() - started < 10:
            angle = step * 0.15
            user32.SetCursorPos(int(cx + radius * math.cos(angle)), int(cy + radius * math.sin(angle)))
            if clicks and step % 40 == 0:
                user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
            elif clicks and step % 40 == 30:
                user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
            point = wintypes.POINT()
            user32.GetCursorPos(ctypes.byref(point))
            ok, ms = ping()
            samples.append(dict(ms=ms, ok=ok))
            step += 1
            time.sleep(0.008)
        user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
        values = sorted(s["ms"] for s in samples)
        report[mode] = dict(
            samples=len(values),
            timeouts=sum(not s["ok"] for s in samples),
            p50=round(values[len(values) // 2], 1),
            p95=round(values[int(len(values) * 0.95)], 1),
            max=round(values[-1], 1),
            over_30ms=sum(v > 30 for v in values),
        )
        print(name, mode, json.dumps(report[mode]), flush=True)
        time.sleep(1)
    (out_dir / f"{name}-latency.json").write_text(json.dumps(report, indent=2) + "\n")
    for log in (root / "localappdata").rglob("jcode-desktop.log"):
        text = log.read_text(errors="ignore")
        (out_dir / f"{name}-desktop.log").write_text(text)
        slow = [line for line in text.splitlines() if any(k in line for k in ("slow window message", "slow render", "DirectX batches", "DirectX present"))]
        print(f"{name}: desktop log {len(text)} bytes, {len(slow)} slow-message lines")
        for line in slow[:80]:
            print("  ", line)
    return report


def wait_for_condition(check, timeout=5):
    """Bound asynchronous window-state checks without hiding a failed handoff."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return True
        time.sleep(0.05)
    return False


def app_process_ids(binary, env, root):
    """Census the exact packaged executable, including processes without windows."""
    image_name = Path(binary).name.replace("'", "''")
    result = subprocess.run(
        ["pwsh", "-NoProfile", "-NonInteractive", "-Command",
         "$ErrorActionPreference='Stop'; "
         "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); "
         f"Get-CimInstance Win32_Process -Filter \"Name='{image_name}'\" | "
         "Select-Object ProcessId,ExecutablePath | ConvertTo-Json -Compress"],
        env=env, cwd=root, timeout=15, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, encoding="utf-8",
    )
    if result.returncode:
        raise RuntimeError(f"Package process census failed: {result.stderr.strip()}")
    items = json.loads(result.stdout) if result.stdout.strip() else []
    if isinstance(items, dict):
        items = [items]
    expected = Path(binary).resolve()
    return sorted(item["ProcessId"] for item in items
                  if item.get("ExecutablePath")
                  and Path(item["ExecutablePath"]).resolve() == expected)


def integration_checks(name, binary, hwnd, app, env, root, out_dir):
    """Windows behavior from issue #5 that a screenshot cannot show."""
    import struct
    report = []

    def check(label, ok, detail=""):
        report.append(f"{'PASS' if ok else 'FAIL'} {label} {detail}".rstrip())

    # 1. The exe is a GUI-subsystem program, so Explorer launches open no console.
    data = Path(binary).read_bytes()[:4096]
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    subsystem = struct.unpack_from("<H", data, pe + 24 + 68)[0]
    check("GUI subsystem (no console window)", subsystem == 2, f"subsystem={subsystem}")

    # 2. Diagnostics land in %LOCALAPPDATA%\Jcode Desktop and are not empty.
    log = root / "localappdata" / "Jcode Desktop" / "jcode-desktop.log"
    text = log.read_text(errors="ignore") if log.exists() else ""
    (out_dir / f"{name}-desktop.log").write_text(text)
    check("diagnostics log exists", log.exists(), str(log))
    check("diagnostics log records startup", "jcode-desktop started" in text, f"{len(text)} bytes")
    check("RUST_LOG=debug reaches the log", " DEBUG " in text or " INFO " in text,
          "no log records" if " DEBUG " not in text else "")

    # 3. A second launch must actually deliver Show to the existing host.
    # Merely exiting with zero while the first window remains open also passes
    # for a broken secondary that silently discards the handoff.
    user32.ShowWindow(hwnd, 6)  # SW_MINIMIZE
    check("original window minimizes before handoff",
          wait_for_condition(lambda: bool(user32.IsIconic(hwnd))))
    second = subprocess.run([str(binary)], env=env, cwd=root, timeout=30,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    # Secondary hosts can fail after delivering Show. Preserve their output and
    # refresh the shared diagnostics after handoff, not just before the launch.
    (out_dir / f"{name}-second-launch.log").write_bytes(second.stdout)
    if log.exists():
        (out_dir / f"{name}-desktop.log").write_text(log.read_text(errors="replace"))
    check("second launch exits", second.returncode == 0, f"code={second.returncode}")
    restored = wait_for_condition(lambda: app.poll() is None
                                  and bool(user32.IsWindow(hwnd))
                                  and bool(user32.IsWindowVisible(hwnd))
                                  and not user32.IsIconic(hwnd)
                                  and hwnd in windows_of(app.pid))
    check("second launch restores the same window", restored, f"hwnd={hwnd}")
    check("first host still running", app.poll() is None)
    windows = windows_of(app.pid)
    check("still exactly the original app window", windows == [hwnd], f"windows={windows}")
    pids = app_process_ids(binary, env, root)
    check("exactly one package app process", pids == [app.pid], f"pids={pids}")

    # 4. --version still prints when stdout is captured (CI, updater smoke test).
    version = subprocess.run([str(binary), "--version"], env=env, cwd=root, timeout=30,
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    check("--version exits successfully", version.returncode == 0, f"code={version.returncode}")
    check("--version prints through a pipe", version.stdout.startswith("Jcode Desktop"),
          repr(version.stdout.strip()[:60]))

    (out_dir / f"{name}-checks.txt").write_text("\n".join(report) + "\n")
    print("\n".join(report))
    return all(line.startswith("PASS") for line in report)


def capture(name, binary, out_dir, timeout):
    from PIL import ImageGrab

    fixture, theme, dismiss = STATES[name]
    with tempfile.TemporaryDirectory(prefix=f"jcode-shot-{name}-") as temporary:
        root = Path(temporary)
        for sub in ("home", "appdata", "localappdata", "jcode", "temp"):
            (root / sub).mkdir()
        config = root / "desktop.toml"
        config.write_text(f'[appearance]\nlayout_mode = "folder_tabs"\ntheme = "{theme}"\n')
        env = {key: os.environ[key] for key in ("SystemRoot", "SystemDrive", "WINDIR", "PATH", "PATHEXT",
                                                  "COMSPEC", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE")
               if key in os.environ}
        env.update({
            "USERPROFILE": str(root / "home"), "HOME": str(root / "home"),
            "APPDATA": str(root / "appdata"), "LOCALAPPDATA": str(root / "localappdata"),
            "TEMP": str(root / "temp"), "TMP": str(root / "temp"),
            "JCODE_HOME": str(root / "jcode"),
            "JCODE_DESKTOP_SCREENSHOT": "1",
            "JCODE_DESKTOP_STATE": str(root / "state"),
            "JCODE_DESKTOP_CONFIG": str(config),
            "JCODE_DESKTOP_SCREENSHOT_PANELS": "1",
            "RUST_BACKTRACE": "1",
        })
        env.update(fixture)
        log_path = out_dir / f"{name}.log"
        with log_path.open("w") as log:
            app = subprocess.Popen([str(binary)], env=env, cwd=root, stdout=log, stderr=subprocess.STDOUT)
            guard = None
            try:
                state = root / "state"
                deadline = time.monotonic() + timeout
                hwnd = None
                while True:
                    if app.poll() is not None:
                        raise RuntimeError(f"{name}: app exited with {app.returncode}, see {log_path.name}")
                    if time.monotonic() > deadline:
                        raise RuntimeError(f"{name}: no rendered frame within {timeout}s")
                    windows = windows_of(app.pid)
                    if windows and state.exists() and "widths=" in state.read_text(errors="ignore"):
                        hwnd = windows[0]
                        break
                    time.sleep(0.25)
                guard = foreground_app(app.pid, out_dir / f"{name}-foreground.json")
                user32.ShowWindow(hwnd, 3)  # SW_MAXIMIZE, matching the Linux screenshots
                time.sleep(3)  # software-rendered frames and fixture animations settle
                if dismiss:
                    press_escape(hwnd)
                    time.sleep(0.8)
                user32.SetForegroundWindow(hwnd)
                time.sleep(0.5)
                guard.require()
                if name in CAPTION_STATES:
                    ok = caption_checks(name, hwnd, out_dir)
                    guard.require()
                    (out_dir / f"{name}.state.txt").write_text(state.read_text(errors="ignore"))
                    if not ok:
                        raise RuntimeError(f"{name}: caption checks failed")
                    print(f"captured {name}")
                    return
                if name in LATENCY_STATES:
                    input_latency(name, hwnd, out_dir, root)
                    guard.require()
                    print(f"captured {name}")
                    return
                if name in INTEGRATION_STATES:
                    if not integration_checks(name, binary, hwnd, app, env, root, out_dir):
                        raise RuntimeError(f"{name}: Windows integration checks failed")
                    guard.require()
                    print(f"captured {name}")
                    return
                rect = wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                screen = (0, 0, user32.GetSystemMetrics(0), user32.GetSystemMetrics(1))
                box = (max(rect.left, screen[0]), max(rect.top, screen[1]),
                       min(rect.right, screen[2]), min(rect.bottom, screen[3]))
                ImageGrab.grab(bbox=box, all_screens=True).save(out_dir / f"{name}.png")
                guard.require()
                (out_dir / f"{name}.state.txt").write_text(state.read_text(errors="ignore"))
            finally:
                if guard:
                    guard.close()
                app.kill()
                app.wait(timeout=30)
    print(f"captured {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("--states", default="all", help="comma-separated subset of: " + ", ".join(STATES))
    parser.add_argument("--resolution", default="1920x1080")
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    names = list(STATES) if args.states.strip() in ("", "all") else [s.strip() for s in args.states.split(",")]
    unknown = [n for n in names if n not in STATES]
    if unknown:
        parser.error(f"unknown states: {unknown}")
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # per-monitor v2, so bounds are physical pixels
    width, height = (int(n) for n in args.resolution.split("x"))
    print(f"set resolution {width}x{height}: {set_resolution(width, height)}")
    print(f"screen is {user32.GetSystemMetrics(0)}x{user32.GetSystemMetrics(1)}")
    args.out_dir.mkdir(parents=True, exist_ok=True)
    failures = []
    for name in names:
        try:
            capture(name, args.binary.resolve(strict=True), args.out_dir, args.timeout)
        except Exception as error:  # keep capturing the remaining states
            failures.append(name)
            print(f"FAILED {error}")
            from PIL import ImageGrab
            ImageGrab.grab(all_screens=True).save(args.out_dir / f"{name}.FAILED-desktop.png")
    print(f"{len(names) - len(failures)}/{len(names)} states captured")
    if failures:
        raise SystemExit("failed states: " + ", ".join(failures))


if __name__ == "__main__":
    main()
