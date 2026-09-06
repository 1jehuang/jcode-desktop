"""Additional native image lifecycle probes using screenshot.py's private display."""
import subprocess
import time
from PIL import Image


def verify_repeated_preview(output, env, root, initial_point):
    from image_preview_acceptance import chart_pixels

    def command(*args):
        subprocess.run(args, env=env, cwd=root, check=True, timeout=15)

    def capture(label):
        path = output.with_name(output.stem + label + ".png")
        command("import", "-window", "root", "png:" + str(path))
        return Image.open(path).convert("RGB")

    # Compare the chart's blue-bar coordinates, not transient hover chrome or
    # the editor cursor. An unchanged count alone could miss an image jumping.
    def signature(image):
        return tuple((x, y) for y in range(image.height)
                     for x in range(280, image.width)
                     if all(abs(a - b) <= 4 for a, b in
                            zip(image.getpixel((x, y))[:3], (92, 124, 173))))

    baselines = {}
    samples = 0
    for cycle in range(4):
        command("xdotool", "mousemove", *map(str, initial_point), "click", "1",
                "mousemove", "100", "100")
        for state in ("open", "closed"):
            if state == "closed":
                command("xdotool", "key", "Escape")
            # Initial layout/transition is intentional, not flicker. Verify a
            # settled image and then repeated frames at each lifecycle state.
            time.sleep(.35)
            for frame in range(4):
                image = capture(f"-cycle-{cycle}-{state}-{frame}")
                chart_pixels(image)  # Failure is explicit if the image vanished.
                current = signature(image)
                if state not in baselines:
                    baselines[state] = current
                assert current == baselines[state], (cycle, state, frame,
                    "chart pixels changed across repeated preview lifecycles")
                samples += 1
                time.sleep(.08)

    log = root / "logs/jcode-desktop/jcode-desktop.log"
    diagnostics = log.read_text()
    output.with_suffix(".preview-diagnostics.log").write_text(diagnostics)
    for anomaly in ("ready-to-pending", "texture-changed", "became-error",
                    "panel_geometry_oscillation"):
        assert anomaly not in diagnostics, anomaly
    decodes = diagnostics.count("desktop-image decode-start")
    assert decodes == 1, f"Preview lifecycle unexpectedly decoded {decodes} times"
    print(f"Repeated image preview stability passed: {samples} frames, four open/close cycles, "
          "identical blue-bar coordinates per state, one decode, no diagnostic anomalies")
