"""Exercise real image clicks on screenshot.py's private X11 display."""
import subprocess
import time
from PIL import Image, ImageChops


def chart_pixels(image, top_left=False):
    """Find the fixture's blue bar, independent of panel or image geometry."""
    points = []
    for y in range(image.height):
        for x in range(image.width):
            r, g, b = image.getpixel((x, y))[:3]
            if abs(r - 92) <= 4 and abs(g - 124) <= 4 and abs(b - 173) <= 4:
                points.append((x, y))
    if not points:
        raise AssertionError("Image fixture's blue bar did not paint")
    xs, ys = zip(*points)
    point = (min(xs), min(ys)) if top_left else ((min(xs) + max(xs)) // 2, (min(ys) + max(ys)) // 2)
    return len(points), point


def image_bounds(image):
    """Find the fixture canvas, excluding its label and Fit/% footer."""
    channels = image.convert("RGB").split()
    masks = [channel.point([255 if abs(value - target) <= 2 else 0
                            for value in range(256)])
             for channel, target in zip(channels, (244, 244, 240))]
    bounds = ImageChops.multiply(ImageChops.multiply(masks[0], masks[1]), masks[2]).getbbox()
    assert bounds is not None, "Image fixture canvas did not paint"
    return bounds


def assert_inline_expanded(initial, expanded):
    before, after = image_bounds(initial), image_bounds(expanded)
    assert abs(after[0] - before[0]) <= 2, ("Image left the transcript container", before, after)
    assert after[2] - after[0] > (before[2] - before[0]) * 1.05, (before, after)
    # A lightbox replaces the sidebar. Inline expansion must leave it mounted
    # and visually unchanged, apart from transient hover/cursor animation.
    sidebar = (0, 48, max(0, min(before[0], after[0]) - 16), initial.height)
    changed = ImageChops.difference(initial.crop(sidebar), expanded.crop(sidebar)).convert("L")
    assert sum(changed.histogram()[21:]) < 250, "Transcript click opened an overlay"


def verify(output, env, root):
    # The fixture's state file can be ready before X11 has presented its first
    # frame, especially during parallel builds. Wait for real chart pixels, not
    # an arbitrary startup delay, and retain the settled baseline screenshot.
    deadline = time.monotonic() + 30
    while True:
        try:
            initial_count, initial_point = chart_pixels(Image.open(output).convert("RGB"))
            break
        except AssertionError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(.2)
            subprocess.run(["import", "-window", "root", "png:" + str(output)],
                           env=env, cwd=root, check=True, timeout=10)

    def click(point):
        subprocess.run(["xdotool", "mousemove", *map(str, point), "click", "1", "mousemove", "100", "100"],
                       env=env, cwd=root, check=True, timeout=10)

    def capture(suffix, predicate):
        path = output.with_name(output.stem + suffix + ".png")
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            time.sleep(.2)
            subprocess.run(["import", "-window", "root", "png:" + str(path)], env=env, cwd=root, check=True, timeout=10)
            count, point = chart_pixels(Image.open(path).convert("RGB"))
            if predicate(count):
                return count, point
        raise AssertionError(f"Image preview interaction failed: {suffix}, initial={initial_count}, actual={count}")

    def mouse(*args):
        subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root, check=True, timeout=10)

    # Zoom and pan the fitted image within its inline viewport.
    mouse("mousemove", *initial_point, "keydown", "ctrl", "click", "4", "keyup", "ctrl")
    inline_count, inline_point = capture("-inline-zoomed", lambda count: count > initial_count * 1.1)
    mouse("mousemove", *inline_point, "mousedown", "1", "mousemove_relative", "--", "-15", "-10", "mouseup", "1")
    capture("-inline-panned", lambda count: abs(count - inline_count) < inline_count * .08)
    edges = [chart_pixels(Image.open(output.with_name(output.stem + suffix + ".png")).convert("RGB"), top_left=True)[1]
             for suffix in ("-inline-zoomed", "-inline-panned")]
    assert abs(edges[1][0] - edges[0][0] + 15) <= 2, edges
    assert abs(edges[1][1] - edges[0][1] + 10) <= 2, edges
    mouse("mousemove", *initial_point, "keydown", "ctrl", "click", "--repeat", "12", "--delay", "30", "5", "keyup", "ctrl")
    capture("-inline-fit", lambda count: abs(count - initial_count) < initial_count * .05)

    # The 1000x600 fixture now fits at 666.7x400, not 533.3x320.
    # Expansion is container-wide, so a 2x pixel-area increase is no longer
    # guaranteed. Compare canvas geometry as well as blue-bar pixels.
    initial_image = Image.open(output).convert("RGB")
    bounds = image_bounds(initial_image)
    assert abs(bounds[3] - bounds[1] - 400) <= 2, ("Expected 400px default height cap", bounds)
    click(initial_point)
    enlarged_count, enlarged_point = capture("-inline-expanded", lambda count: count > initial_count * 1.1)
    assert_inline_expanded(initial_image, Image.open(output.with_name(output.stem + "-inline-expanded.png")).convert("RGB"))
    click(enlarged_point)
    capture("-inline-collapsed", lambda count: abs(count - initial_count) < initial_count * .05)
    click(initial_point)
    _, enlarged_point = capture("-inline-reexpanded", lambda count: count > initial_count * 1.1)

    # Ctrl+wheel and drag still operate within the expanded inline viewport.
    mouse("mousemove", *enlarged_point, "keydown", "ctrl", "click", "4", "keyup", "ctrl")
    zoomed_count, zoomed_point = capture("-wheel-zoomed", lambda count: count > enlarged_count * 1.1)
    mouse("mousemove", *zoomed_point, "mousedown", "1", "mousemove_relative", "--", "-35", "-25", "mouseup", "1")
    _, panned_point = capture("-drag-panned", lambda count: abs(count - zoomed_count) < zoomed_count * .08)
    edges = [chart_pixels(Image.open(output.with_name(output.stem + suffix + ".png")).convert("RGB"), top_left=True)[1]
             for suffix in ("-wheel-zoomed", "-drag-panned")]
    assert abs(edges[1][0] - edges[0][0] + 35) <= 2, edges
    assert abs(edges[1][1] - edges[0][1] + 25) <= 2, edges
    click(panned_point)
    capture("-gesture-collapsed", lambda count: abs(count - initial_count) < initial_count * .05)

    # Do not call image_flicker_acceptance.verify_repeated_preview here: that
    # helper exercises the separate lightbox lifecycle and dismisses via Escape.
    # Transcript images toggle inline on a second click instead.
    signatures = {}
    for cycle in range(4):
        click(initial_point)
        _, point = capture(f"-cycle-{cycle}-expanded", lambda count: count > initial_count * 1.1)
        for state in ("expanded", "collapsed"):
            if state == "collapsed":
                click(point)
            for frame in range(4):
                predicate = (lambda count: count > initial_count * 1.1) if state == "expanded" else (
                    lambda count: abs(count - initial_count) < initial_count * .05)
                suffix = f"-cycle-{cycle}-{state}-{frame}"
                capture(suffix, predicate)
                image = Image.open(output.with_name(output.stem + suffix + ".png")).convert("RGB")
                signature = tuple((x, y) for y in range(image.height) for x in range(image.width)
                                  if all(abs(a - b) <= 4 for a, b in zip(image.getpixel((x, y)), (92, 124, 173))))
                signatures.setdefault(state, signature)
                assert signature == signatures[state], (cycle, state, frame, "Inline image flickered or shifted")

    diagnostics = (root / "logs/jcode-desktop/jcode-desktop.log").read_text()
    output.with_suffix(".preview-diagnostics.log").write_text(diagnostics)
    for anomaly in ("ready-to-pending", "texture-changed", "became-error", "panel_geometry_oscillation"):
        assert anomaly not in diagnostics, anomaly
    decodes = diagnostics.count("desktop-image decode-start")
    assert decodes == 1, f"Inline lifecycle unexpectedly decoded {decodes} times"
    print(f"Image preview native acceptance passed: blue-bar pixels {initial_count} -> inline {inline_count} -> expanded {enlarged_count} -> {zoomed_count}; "
          "400px default cap, inline click expand/collapse, wheel zoom, drag pan and fit, 32 stable lifecycle frames, one decode. "
          "Expanded/zoomed PNGs retain the label and Fit/% footer below the image for visual review.")
