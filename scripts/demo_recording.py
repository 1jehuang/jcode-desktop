"""Record scripted demos of the real app on screenshot.py's private Xvfb display.

Called only from screenshot.py after the fixture has rendered. ffmpeg grabs the
private X display (never the user's session) while native xdotool input drives
the app, so every frame is real GPUI rendering of the offline fixture.
"""
import json
import shutil
import subprocess
import time

def _navigation(root):
    try:
        text = (root / "state").read_text()
        return json.loads(next(line.partition("=")[2] for line in text.splitlines()
                               if line.startswith("navigation=")))
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


class Driver:
    def __init__(self, env, root, width, height):
        self.env, self.root, self.width, self.height = env, root, width, height

    def xdotool(self, *args):
        subprocess.run(["xdotool", *args], env=self.env, cwd=self.root, check=True, timeout=30)

    def key(self, combo, pause=0.9):
        self.xdotool("key", "--clearmodifiers", combo)
        self.settle()
        time.sleep(pause)

    def type(self, text, delay_ms=55, pause=0.6):
        self.xdotool("type", "--delay", str(delay_ms), text)
        time.sleep(pause)

    def click(self, x, y, pause=0.6):
        self.xdotool("mousemove", str(round(x)), str(round(y)), "click", "1")
        time.sleep(pause)

    def settle(self, timeout=3):
        deadline = time.monotonic() + timeout
        time.sleep(0.05)
        while time.monotonic() < deadline:
            state = _navigation(self.root)
            if state and not (state["camera_motion"] or state["tab_motion"] or state["row_motion"]):
                return state
            time.sleep(0.02)
        return _navigation(self.root)

    def focus_composer(self):
        # The composer sits at the bottom of the focused panel.
        self.click((276 + self.width) / 2, self.height - 55, pause=0.3)


def _navigate(d):
    for combo in ("super+h", "super+h", "super+l", "super+l", "super+l", "super+l", "super+u"):
        d.key(combo, pause=0.7)
    d.key("super+f", pause=1.0)
    d.key("super+f", pause=0.8)


def _overview(d):
    d.key("super+l", pause=0.6)
    d.key("super+o", pause=1.6)
    d.key("super+l", pause=0.7)
    d.key("super+l", pause=0.7)
    d.key("super+o", pause=1.2)


def _map(d):
    # Stack panels into rows through the real move shortcuts, then travel.
    d.key("super+u", pause=0.4)
    d.key("super+shift+j", pause=0.8)
    d.key("super+k", pause=0.8)
    d.key("super+l", pause=0.4)
    d.key("super+shift+j", pause=0.8)
    d.key("super+shift+j", pause=0.8)
    for combo in ("super+k", "super+k", "super+j", "super+j", "super+k"):
        d.key(combo, pause=0.8)


def _typing(d):
    d.focus_composer()
    d.type("Refactor the session loader and add tests", pause=1.2)


def _spawn(d):
    # Open fresh sessions beside the current ones and start typing in each.
    for prompt in ("Profile the slowest test", "Draft the release notes"):
        d.key("super+n", pause=0.9)
        d.type(prompt, pause=0.9)
    d.key("super+o", pause=1.6)
    d.key("super+o", pause=0.8)


def _tour(d):
    _navigate(d)
    _overview(d)


SCRIPTS = {"navigate": _navigate, "overview": _overview, "map": _map, "typing": _typing,
           "spawn": _spawn, "tour": _tour}
DEMOS = tuple(SCRIPTS)


def record(path, demo, env, root, width, height, fps=30, lead_in=0.8, tail=1.0):
    """Record `demo` to `path` (mp4/webm by extension) from the private display."""
    if not shutil.which("ffmpeg"):
        raise RuntimeError("--record requires ffmpeg")
    if demo not in SCRIPTS:
        raise ValueError(f"unknown demo {demo!r}; choose from {DEMOS}")
    path.parent.mkdir(parents=True, exist_ok=True)
    raw = root / "recording.mkv"
    # Lossless capture first so slow software rendering never drops input timing,
    # then encode once to a web-friendly format.
    ffmpeg = subprocess.Popen(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", "0",
         "-framerate", str(fps), "-video_size", f"{width}x{height}", "-i", env["DISPLAY"],
         "-c:v", "libx264rgb", "-preset", "ultrafast", "-crf", "0", str(raw)],
        stdin=subprocess.PIPE, env=env, cwd=root)
    try:
        time.sleep(lead_in)
        SCRIPTS[demo](Driver(env, root, width, height))
        time.sleep(tail)
    finally:
        try:
            ffmpeg.communicate(b"q", timeout=30)
        except subprocess.TimeoutExpired:
            ffmpeg.kill()
            ffmpeg.wait()
    if path.suffix == ".webm":
        codec = ["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "34", "-row-mt", "1"]
    else:
        codec = ["-c:v", "libx264", "-preset", "slow", "-crf", "24", "-pix_fmt", "yuv420p",
                 "-movflags", "+faststart"]
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(raw), "-an", *codec, str(path)],
                   check=True, cwd=root)
    print(f"Recording: {path}")
