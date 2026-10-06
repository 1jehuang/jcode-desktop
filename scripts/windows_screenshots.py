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

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
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
    user32.SetCursorPos(5, 5)
    time.sleep(0.5)
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
                user32.ShowWindow(hwnd, 3)  # SW_MAXIMIZE, matching the Linux screenshots
                time.sleep(3)  # software-rendered frames and fixture animations settle
                if dismiss:
                    press_escape(hwnd)
                    time.sleep(0.8)
                user32.SetForegroundWindow(hwnd)
                time.sleep(0.5)
                if name in CAPTION_STATES:
                    ok = caption_checks(name, hwnd, out_dir)
                    (out_dir / f"{name}.state.txt").write_text(state.read_text(errors="ignore"))
                    if not ok:
                        raise RuntimeError(f"{name}: caption checks failed")
                    print(f"captured {name}")
                    return
                rect = wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                screen = (0, 0, user32.GetSystemMetrics(0), user32.GetSystemMetrics(1))
                box = (max(rect.left, screen[0]), max(rect.top, screen[1]),
                       min(rect.right, screen[2]), min(rect.bottom, screen[3]))
                ImageGrab.grab(bbox=box, all_screens=True).save(out_dir / f"{name}.png")
                (out_dir / f"{name}.state.txt").write_text(state.read_text(errors="ignore"))
            finally:
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
