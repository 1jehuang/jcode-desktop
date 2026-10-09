#!/usr/bin/env python3
"""Regenerate the jcode.sh/desktop media set from the real app, headlessly.

Every screenshot and video is rendered by the real jcode-desktop binary with
offline fixtures on private Xvfb displays (see screenshot.py), so nothing touches
the user's session. Output: <out>/<name>.webp and <out>/<name>.mp4 plus a poster
<name>.webp for each video.

    python3 scripts/website_media.py ~/jcode-website/public/desktop-media
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

REPO = Path(__file__).resolve().parents[1]

SESSIONS = {
    "panel_transcripts": "diff-rich,mermaid,image,tool-icons",
    "panel_titles": "Review the session diff,Plan the release flow,Read the benchmark chart,Survey the tool surface",
}

SHOTS = {
    "workspace": dict(panels=4, **SESSIONS),
    "three-sessions": dict(panels=3, panel_transcripts="diff-rich,reasoning,mermaid",
                           panel_titles="Review the session diff,Quiet the thinking display,Plan the release flow"),
    "diff-review": dict(transcript="diff-rich", panel_titles="Review the session diff"),
    "mermaid": dict(transcript="mermaid", panel_titles="Plan the release flow"),
    "inline-image": dict(transcript="image", panel_titles="Read the benchmark chart"),
    "tools": dict(transcript="tool-icons", panel_titles="Survey the tool surface"),
    "orchestration": dict(transcript="orchestration"),
    "applet": dict(transcript="applet"),
    "todos": dict(transcript="todos"),
    "light-theme": dict(panels=3, theme="light-neutral", panel_transcripts="diff-rich,reasoning,mermaid",
                        panel_titles="Review the session diff,Quiet the thinking display,Plan the release flow"),
}

VIDEOS = {
    "tour": dict(panels=4, demo="tour", **SESSIONS),
    "overview": dict(panels=4, demo="overview", **SESSIONS),
    "spawn": dict(panels=2, demo="spawn", transcript="empty", panel_transcripts="diff-rich,mermaid",
                  panel_titles="Review the session diff,Plan the release flow"),
    "map": dict(panels=4, demo="map", **SESSIONS),
}


def command(png, spec, video=None):
    cmd = [sys.executable, str(REPO / "scripts/screenshot.py"), "--no-build", str(png)]
    for key, value in spec.items():
        if key == "demo":
            continue
        flag = "--" + key.replace("_", "-")
        cmd += [flag] if value is True else [flag, str(value)]
    if video is not None:
        cmd += ["--record", str(video), "--demo", spec["demo"]]
    return cmd


def webp(png, out, width=1440):
    subprocess.run(["magick", str(png), "-resize", f"{width}x>", "-quality", "86",
                    "-define", "webp:method=6", str(out)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out", type=Path)
    parser.add_argument("--only", nargs="*", help="limit to these shot/video names")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if not shutil.which("magick"):
        parser.error("requires ImageMagick 7 (magick) for WebP output")
    subprocess.run(["cargo", "build", "-p", "jcode-desktop"], cwd=REPO, check=True)
    args.out.mkdir(parents=True, exist_ok=True)
    wanted = lambda name: not args.only or name in args.only
    with tempfile.TemporaryDirectory(prefix="website-media-") as scratch:
        scratch = Path(scratch)

        def shot(name):
            png = scratch / f"{name}.png"
            subprocess.run(command(png, SHOTS[name]), check=True, stdout=subprocess.DEVNULL)
            webp(png, args.out / f"{name}.webp")
            return name

        def video(name):
            png = scratch / f"{name}-end.png"
            mp4 = args.out / f"{name}.mp4"
            mp4.unlink(missing_ok=True)
            subprocess.run(command(png, VIDEOS[name], mp4), check=True, stdout=subprocess.DEVNULL)
            poster = scratch / f"{name}-poster.png"
            subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-ss", "0.5", "-i", str(mp4),
                            "-frames:v", "1", str(poster)], check=True)
            webp(poster, args.out / f"{name}-poster.webp")
            return name

        jobs = [(shot, n) for n in SHOTS if wanted(n)] + [(video, n) for n in VIDEOS if wanted(n)]
        with ThreadPoolExecutor(args.jobs) as pool:
            for done in pool.map(lambda job: job[0](job[1]), jobs):
                print("rendered", done)


if __name__ == "__main__":
    main()
