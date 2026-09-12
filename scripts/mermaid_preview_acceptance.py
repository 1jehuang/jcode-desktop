"""Verify Mermaid preview pixels using native clicks on screenshot.py's private X11.

Run through screenshot.py --transcript mermaid --mermaid-interact. No harness
actions or live desktop sockets are used. Each interaction leaves a PNG beside
the initial screenshot, including the last observed frame on failure.
"""
from dataclasses import dataclass
import subprocess
import time

from PIL import Image, ImageChops


def color_mask(image, color):
    channels = image.convert("RGB").split()
    masks = [channel.point([255 if value == target else 0 for value in range(256)])
             for channel, target in zip(channels, color)]
    return ImageChops.multiply(ImageChops.multiply(masks[0], masks[1]), masks[2])


@dataclass
class Diagram:
    mask: Image.Image
    count: int
    bounds: tuple

    @property
    def point(self):
        left, top, right, bottom = self.bounds
        return (left + right) // 2, (top + bottom) // 2

    @property
    def height(self):
        return self.bounds[3] - self.bounds[1]


def diagram_pixels(image):
    """Measure diagram node fills in the default warm-neutral fixture.

    The full-window overlay starts at (0, 0). In the transcript, exclude the
    sidebar, user prompt, and composer, which share the node fill color.
    Keep the legacy renderer palette supported for older linked binaries.
    """
    canvas = color_mask(image, (51, 51, 51))
    nodes = color_mask(image, (31, 32, 32))
    if canvas.histogram()[255] >= 200 and nodes.histogram()[255] >= 200:
        mask = ImageChops.lighter(canvas, nodes)
        mask.paste(0, (0, 0, 264, image.height))
    else:
        mask = color_mask(image, (48, 43, 39))
        if image.getpixel((0, 0))[:3] != (37, 34, 31):
            mask.paste(0, (0, 0, image.width, 190))
            mask.paste(0, (0, 0, 280, image.height))
            # The outer window gutter shares the node fill. Including it
            # stretches the bounds and sends the native click below the SVG.
            mask.paste(0, (image.width - 24, 0, image.width, image.height))
            mask.paste(0, (0, image.height - 110, image.width, image.height))
    if mask.histogram()[255] < 200:
        raise AssertionError("Mermaid fixture's diagram nodes did not paint")
    return Diagram(mask, mask.histogram()[255], mask.getbbox())


def enlarged(initial, actual):
    return actual.count > initial.count * 1.25 and actual.height > initial.height * 1.25


def restored(initial, actual):
    # Compare position as well as area. A still-open or shifted diagram is not
    # restoration, even if it happens to contain the same number of gray pixels.
    changed = ImageChops.difference(initial.mask, actual.mask).histogram()[255]
    return changed < initial.count * .03


def zoom_changed(fitted, previous, actual):
    # A window-wide fitted SVG is already near both horizontal edges. Zooming
    # clips entire nodes, so visible pixel area need not increase by 25%.
    # Node height must grow and the actual rendered pixels must change.
    return actual.height > fitted.height * 1.25 and not restored(previous, actual)


def close_button_point(before, preview, diagram):
    """Locate the newly painted Close text at the top right from screenshot ink.

    The window-sized overlay's rightmost header control is 'Close ×'. Its text
    occupies the last 100px, above the diagram. Derive its y coordinate from
    the pixels instead of assuming a particular panel/header height. Excluding
    unchanged pixels avoids mistaking the app's tabs for the close control.
    """
    right = preview.width - 16
    left = preview.width - 100
    bottom = min(diagram.bounds[1], diagram_pixels(before).bounds[1])
    region = preview.crop((left, 0, right, bottom)).convert("RGB")
    old = before.crop((left, 0, right, bottom)).convert("RGB")
    background = max(region.getcolors(region.width * region.height), key=lambda entry: entry[0])[1]
    points = []
    pixels, previous = region.load(), old.load()
    for y in range(region.height):
        for x in range(region.width):
            color = pixels[x, y]
            if (max(abs(a - b) for a, b in zip(color, background)) > 45
                    and max(abs(a - b) for a, b in zip(color, previous[x, y])) > 30):
                points.append((left + x, y))
    if len(points) < 20:
        raise AssertionError("Preview's explicit top-right Close button did not paint")
    xs, ys = zip(*points)
    if max(xs) - min(xs) < 25 or max(ys) - min(ys) < 6:
        raise AssertionError("Preview's Close text is missing or clipped")
    return (min(xs) + max(xs)) // 2, (min(ys) + max(ys)) // 2


def zoom_button_points(preview, close_point):
    """Separate header controls by their visible inter-control whitespace.

    Rightmost controls are minus, Fit, plus, percentage, Esc hint, Close.
    Their 12px gaps plus button padding exceed spaces inside the text labels.
    """
    y = close_point[1]
    pixels = preview.convert("RGB").load()
    background = pixels[preview.width - 20, y]
    columns = []
    for x in range(264, preview.width - 16):
        if any(max(abs(a - b) for a, b in zip(pixels[x, row], background)) > 45
               for row in range(max(0, y - 10), min(preview.height, y + 11))):
            columns.append(x)
    groups = []
    for x in columns:
        if not groups or x - groups[-1][-1] > 12:
            groups.append([])
        groups[-1].append(x)
    if len(groups) < 6:
        raise AssertionError("Preview zoom controls did not paint as distinct header controls")
    centers = [(group[0] + group[-1]) // 2 for group in groups]
    if abs(centers[-1] - close_point[0]) > 15:
        raise AssertionError("Preview zoom controls are not aligned with the Close button")
    return (centers[-4], y), (centers[-5], y)


def verify(output, env, root):
    # Fixture-state readiness can precede the first software-rendered frame
    # under load. Wait for real diagram pixels, not an arbitrary extra sleep.
    deadline = time.monotonic() + 15
    while True:
        with Image.open(output) as image:
            before = image.convert("RGB")
        try:
            initial = diagram_pixels(before)
            break
        except AssertionError:
            if time.monotonic() >= deadline:
                raise AssertionError(f"Initial Mermaid diagram did not paint: {output}") from None
        time.sleep(.2)
        subprocess.run(["import", "-window", "root", "png:" + str(output)],
                       env=env, cwd=root, check=True, timeout=10)

    def click(point):
        subprocess.run(["xdotool", "mousemove", *map(str, point), "click", "1",
                        "mousemove", "100", "100"],
                       env=env, cwd=root, check=True, timeout=10)

    def escape():
        subprocess.run(["xdotool", "key", "Escape"],
                       env=env, cwd=root, check=True, timeout=10)

    def capture(suffix, predicate, reference=initial):
        path = output.with_name(output.stem + suffix + ".png")
        if path.exists():
            raise FileExistsError(f"Refusing to overwrite acceptance artifact {path}")
        deadline = time.monotonic() + 15
        detail = "no frame captured"
        while time.monotonic() < deadline:
            time.sleep(.2)
            subprocess.run(["import", "-window", "root", "png:" + str(path)],
                           env=env, cwd=root, check=True, timeout=10)
            with Image.open(path) as image:
                frame = image.convert("RGB")
            try:
                actual = diagram_pixels(frame)
                detail = f"pixels={actual.count}, bounds={actual.bounds}"
                if predicate(reference, actual):
                    return frame, actual
            except AssertionError as error:
                detail = str(error)
        raise AssertionError(f"Mermaid preview {suffix} failed: initial pixels={initial.count}, "
                             f"bounds={initial.bounds}; {detail}; screenshot={path}")

    click(initial.point)
    preview, first = capture("-enlarged", enlarged)
    # The overlay must cover the sidebar and app header, not just grow inside
    # the right panel. All four window corners belong to its background.
    for point in ((0, 0), (preview.width - 1, 0),
                  (0, preview.height - 1), (preview.width - 1, preview.height - 1)):
        assert preview.getpixel(point) == (37, 34, 31), (
            f"Preview does not cover window corner {point}: {preview.getpixel(point)}")
    close_point = close_button_point(before, preview, first)
    zoom_point, fit_point = zoom_button_points(preview, close_point)
    click(zoom_point)
    _, zoomed = capture("-zoomed", lambda fitted, actual: zoom_changed(fitted, first, actual), first)
    maximum_zoom = zoomed
    for percent in (200, 250, 300, 350, 400):
        previous = maximum_zoom
        click(zoom_point)
        # Every + click must visibly change the rendered diagram. At high
        # zoom the viewport clips the image, so its total visible area need
        # not grow each time. Diagram nodes must still be present and the
        # diagram must remain larger than Fit instead of moving offscreen.
        _, maximum_zoom = capture(
            f"-zoom-{percent}",
            lambda fitted, actual: zoom_changed(fitted, previous, actual),
            first,
        )
    click(fit_point)
    capture("-fit", restored, first)
    escape()
    capture("-escape-closed", restored)
    click(initial.point)
    preview, second = capture("-reopened", enlarged)
    close_point = close_button_point(before, preview, second)
    click(close_point)
    capture("-close-button-closed", restored)
    # Verify opening still works after the explicit button, not just Escape.
    click(initial.point)
    capture("-reopened-again", enlarged)
    escape()
    capture("-final-closed", restored)
    print(f"Mermaid preview native acceptance passed: diagram pixels {initial.count} -> {first.count}; "
          f"height {initial.height} -> {first.height}; + zoom pixels={zoomed.count}, "
          f"400% pixels={maximum_zoom.count} remains visible and Fit restores; "
          f"Escape and Close {close_point} restore "
          "the thumbnail; repeated opening works")
