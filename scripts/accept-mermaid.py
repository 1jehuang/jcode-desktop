#!/usr/bin/env python3
"""Accept Mermaid rendering across two real UI hot reloads on private Xvfb.

Uses the existing target/debug host and UI plugin. It does not run an initial
build itself. Ctrl+R intentionally exercises the application's real rebuild and
reload action. Artifacts include three screenshots plus app/Xvfb logs.
"""
import argparse
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time

from PIL import Image, ImageChops, ImageStat
from screenshot import isolated_env


def wait_for(description, predicate, app, app_log, timeout=90):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if app.poll() is not None:
            app_log.flush()
            raise RuntimeError(f"App exited while waiting for {description}:\n{app_log_path(app_log).read_text(errors='replace')}")
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    app_log.flush()
    raise RuntimeError(f"Timed out waiting for {description}:\n{app_log_path(app_log).read_text(errors='replace')}")


def app_log_path(handle):
    return Path(handle.name)


def log_count(path, needle):
    return path.read_text(errors="replace").count(needle) if path.exists() else 0


def parse_crop(value, width, height):
    try:
        left, top, right, bottom = (int(part) for part in value.split(","))
    except ValueError as error:
        raise argparse.ArgumentTypeError("crop must be left,top,right,bottom") from error
    if not (0 <= left < right <= width and 0 <= top < bottom <= height):
        raise argparse.ArgumentTypeError("crop must lie inside the screen")
    return left, top, right, bottom


def capture(path, env, root):
    subprocess.run(
        ["import", "-window", "root", "png:" + str(path)],
        env=env,
        cwd=root,
        check=True,
        timeout=15,
    )


def comparison(reference_path, actual_path, crop):
    reference = Image.open(reference_path).convert("RGB").crop(crop)
    actual = Image.open(actual_path).convert("RGB").crop(crop)
    difference = ImageChops.difference(reference, actual)
    changed = sum(1 for pixel in difference.getdata() if pixel != (0, 0, 0))
    pixels = reference.width * reference.height

    # Mermaid's full-color image should have meaningful chroma. A stretched
    # tab emoji occupying the diagram rectangle changes both this count and the
    # exact crop by far more than the small tolerance below.
    def colorful(image):
        return sum(1 for r, g, b in image.getdata() if max(r, g, b) - min(r, g, b) >= 24)

    ref_color = colorful(reference)
    actual_color = colorful(actual)
    return {
        "changed": changed,
        "pixels": pixels,
        "changed_fraction": changed / pixels,
        "reference_colorful": ref_color,
        "actual_colorful": actual_color,
        "color_fraction_delta": abs(actual_color - ref_color) / max(ref_color, 1),
        "mean_difference": sum(ImageStat.Stat(difference).mean) / 3,
    }


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--plugin", type=Path, default=repo / "target/debug/libjcode_desktop_ui.so")
    parser.add_argument("--output-dir", type=Path, default=repo / "target/accept-mermaid")
    parser.add_argument("--size", default="1500x950")
    parser.add_argument("--crop", default="300,80,1450,850",
                        help="stable transcript crop as left,top,right,bottom")
    parser.add_argument("--max-changed", type=float, default=0.005,
                        help="maximum changed-pixel fraction after each reload")
    parser.add_argument("--max-color-delta", type=float, default=0.01,
                        help="maximum relative change in colorful pixels")
    args = parser.parse_args()

    try:
        width, height = (int(part) for part in args.size.split("x"))
    except ValueError:
        parser.error("size must be WIDTHxHEIGHT")
    crop = parse_crop(args.crop, width, height)
    for tool in ("Xvfb", "openbox", "xdotool", "import", "cargo"):
        if not shutil.which(tool):
            parser.error(f"missing required tool: {tool}")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe Vulkan driver")
    binary = args.binary.resolve(strict=True)
    plugin = args.plugin.resolve(strict=True)
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        parser.error(f"refusing to mix artifacts into non-empty directory: {output}")

    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", repo / "target"))
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="accept-mermaid-", dir=scratch) as temporary:
        root = Path(temporary)
        env = isolated_env(root)
        env.update({
            "JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "mermaid",
            "JCODE_DESKTOP_SCREENSHOT_PANELS": "1",
            "JCODE_DESKTOP_CONFIG": str(root / "desktop.toml"),
            "VK_DRIVER_FILES": str(drivers[0]),
        })
        (root / "desktop.toml").write_text('[appearance]\nlayout_mode = "folder_tabs"\ntheme = "warm-neutral"\n')
        for name in ("home", "runtime", "config", "cache", "data", "jcode"):
            (root / name).mkdir(mode=0o700)
        wm_config = root / "openbox.xml"
        wm_config.write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>''')

        processes = []
        read_fd, write_fd = os.pipe()
        app_log_file = root / "app.log"
        xvfb_log_file = root / "xvfb.log"
        try:
            with xvfb_log_file.open("w+") as xvfb_log, app_log_file.open("w+") as app_log:
                xvfb = subprocess.Popen(
                    ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", f"{width}x{height}x24", "-nolisten", "tcp"],
                    pass_fds=(write_fd,), env=env, cwd=root, stdout=xvfb_log, stderr=xvfb_log,
                )
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise RuntimeError("Xvfb startup timeout")
                display = os.read(read_fd, 64).decode().strip()
                if not display.isdigit():
                    raise RuntimeError("Xvfb failed to allocate a display")
                env["DISPLAY"] = ":" + display
                wm = subprocess.Popen(
                    ["openbox", "--sm-disable", "--config-file", str(wm_config)],
                    env=env, cwd=root, stdout=xvfb_log, stderr=xvfb_log,
                )
                processes.append(wm)
                time.sleep(0.5)
                if wm.poll() is not None:
                    raise RuntimeError("Private Openbox failed to start")

                app = subprocess.Popen(
                    [str(binary), "--hot-reload", str(plugin)],
                    env=env, cwd=repo, stdout=app_log, stderr=app_log,
                )
                processes.append(app)
                state = root / "state"
                diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
                wait_for("fixture render", lambda: state.exists() and "widths=1.00" in state.read_text(), app, app_log, 60)
                wait_for(
                    "startup plugin activation",
                    lambda: log_count(diagnostics, "activated UI generation") >= 1,
                    app, app_log, 180,
                )
                time.sleep(2)

                paths = [output / "generation-0.png", output / "generation-1.png", output / "generation-2.png"]
                capture(paths[0], env, root)
                evidence = []
                for generation in (1, 2):
                    subprocess.run(
                        ["xdotool", "key", "--clearmodifiers", "ctrl+r"],
                        env=env, cwd=root, check=True, timeout=10,
                    )
                    wait_for(
                        f"plugin activation {generation + 1}",
                        lambda expected=generation + 1: log_count(diagnostics, "activated UI generation") >= expected,
                        app, app_log, 180,
                    )
                    time.sleep(2)
                    capture(paths[generation], env, root)
                    result = comparison(paths[0], paths[generation], crop)
                    evidence.append(result)
                    if result["changed_fraction"] > args.max_changed:
                        raise AssertionError(f"Mermaid crop changed after reload {generation}: {result}")
                    if result["color_fraction_delta"] > args.max_color_delta:
                        raise AssertionError(f"Mermaid color population changed after reload {generation}: {result}")

                app_log.flush()
                xvfb_log.flush()
                shutil.copy2(app_log_file, output / "app.log")
                shutil.copy2(xvfb_log_file, output / "xvfb.log")
                if diagnostics.exists():
                    shutil.copy2(diagnostics, output / "desktop.log")
                report = output / "evidence.txt"
                report.write_text(
                    f"crop={crop}\n"
                    + "\n".join(f"reload_{index}={value}" for index, value in enumerate(evidence, 1))
                    + "\n"
                )
                print(f"Mermaid hot-reload acceptance passed. Artifacts: {output}")
                print(report.read_text(), end="")
        finally:
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            # Preserve diagnostics even on failure.
            for source in (app_log_file, xvfb_log_file, root / "logs/jcode-desktop/jcode-desktop.log"):
                destination = output / ("desktop.log" if source.name == "jcode-desktop.log" else source.name)
                if source.exists() and not destination.exists():
                    shutil.copy2(source, destination)


if __name__ == "__main__":
    main()
