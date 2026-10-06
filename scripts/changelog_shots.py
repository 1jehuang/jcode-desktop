#!/usr/bin/env python3
"""Render the screenshots shown under each Theme in the in-app changelog.

Screenshots live in assets/changelog/<version>/N-name.png. The number N matches
the Nth bullet under `#### Themes` in that release's CHANGELOG.md section, and
the build embeds every PNG there. CHANGELOG.md itself never references them, so
GitHub and Discord release text is unchanged.

Each shot is a real offline render from scripts/screenshot.py (private Xvfb,
fixture data, no credentials), cropped to the relevant surface. Edit SHOTS for a
new release, then run:

    python3 scripts/changelog_shots.py 0.4.0
    python3 scripts/changelog_shots.py 0.4.0 --only 2   # re-render one shot

Review every PNG before committing. Crops are pixel boxes on a 1440x1000 render,
so a layout change can shift them.
"""
import argparse
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent

# version -> [(number, name, screenshot.py args, crop WxH+X+Y, resize width or None)]
SHOTS = {
    "0.4.0": [
        (1, "applets", ["--transcript", "applet"], "592x370+848+140", None),
        (2, "accounts", ["--accounts"], "1190x480+236+150", 1000),
        (3, "model-picker", ["--model-interact"], "1200x460+232+505", 1000),
    ],
}


def render(args, output):
    command = [sys.executable, str(ROOT / "scripts/screenshot.py"), "--no-build", *args, str(output)]
    subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("version", help="Desktop version without a v, such as 0.4.0")
    parser.add_argument("--only", type=int, help="render only the shot with this theme number")
    parser.add_argument("--build", action="store_true", help="build Desktop before rendering")
    options = parser.parse_args()
    shots = SHOTS.get(options.version)
    if not shots:
        parser.error(f"no shots defined for {options.version}. Add them to SHOTS in this script.")
    magick = shutil.which("magick") or shutil.which("convert")
    if not magick:
        parser.error("ImageMagick (magick or convert) is required to crop screenshots")
    if options.build:
        subprocess.run(["cargo", "build"], cwd=ROOT, check=True)
    out_dir = ROOT / "assets/changelog" / options.version
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as scratch:
        for number, name, args, crop, width in shots:
            if options.only is not None and number != options.only:
                continue
            raw = pathlib.Path(scratch) / f"{number}-raw.png"
            render(args, raw)
            for stale in out_dir.glob(f"{number}-*.png"):
                stale.unlink()
            target = out_dir / f"{number}-{name}.png"
            command = [magick, str(raw), "-crop", crop, "+repage"]
            if width:
                command += ["-resize", f"{width}x"]
            # 8-bit palette PNGs keep embedded screenshots small.
            command += ["-strip", f"PNG8:{target}"]
            subprocess.run(command, check=True)
            print(f"{target.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
