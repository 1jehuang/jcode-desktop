"""Verify shared inline Mermaid gestures on screenshot.py's private X11.

No live desktop sockets or harness actions are used. Native vector pixels must
change on zoom/pan and restore on Fit, with the sidebar staying mounted.
"""
from dataclasses import dataclass
import subprocess
import time

from PIL import Image, ImageChops, ImageDraw


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
    """Find connected node fills, not unrelated cards with the same palette.

    The stop notice shares node fill, but spans almost the entire transcript.
    Connected-component width rejects that notice without excluding diagrams
    near the top, which otherwise sends the zoom gesture to the wrong row.
    """
    remaining = color_mask(image, (48, 43, 39))
    remaining.paste(0, (0, 0, image.width, 110))
    remaining.paste(0, (0, 0, 232, image.height))
    remaining.paste(0, (image.width - 24, 0, image.width, image.height))
    remaining.paste(0, (0, image.height - 110, image.width, image.height))
    mask = Image.new("L", image.size)
    while (bounds := remaining.getbbox()) is not None:
        left, top, right, bottom = bounds
        seed = next((x, top) for x in range(left, right) if remaining.getpixel((x, top)))
        previous = remaining.copy()
        ImageDraw.floodfill(remaining, seed, 0)
        component = ImageChops.subtract(previous, remaining)
        x, y, end_x, end_y = component.getbbox()
        if (8 <= end_x - x < image.width * .7 and 8 <= end_y - y < (end_x - x) * 8
                and component.histogram()[255] >= 64):
            mask = ImageChops.lighter(mask, component)
    if mask.histogram()[255] < 200:
        raise AssertionError("Mermaid fixture's diagram nodes did not paint")
    return Diagram(mask, mask.histogram()[255], mask.getbbox())


def restored(initial, actual):
    changed = ImageChops.difference(initial.mask, actual.mask).histogram()[255]
    return changed < initial.count * .03


def verify(output, env, root):
    def mouse(*args):
        subprocess.run(["xdotool", *map(str, args)],
                       env=env, cwd=root, check=True, timeout=10)

    def capture(suffix, predicate):
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
                if predicate(actual):
                    return frame, actual
            except AssertionError as error:
                detail = str(error)
        raise AssertionError(f"Mermaid inline {suffix} failed: {detail}; screenshot={path}")

    before, initial = capture("-settled", lambda _: True)
    # One Ctrl-wheel step changes the same inline viewport as normal images.
    mouse("mousemove", *initial.point, "keydown", "ctrl", "click", "4", "keyup", "ctrl")
    zoom_frame, zoomed = capture("-inline-zoomed", lambda actual: not restored(initial, actual)
                                 and actual.height > initial.height * 1.05)
    mouse("mousemove", *initial.point, "mousedown", "1", "mousemove_relative", "--", "-15", "-10", "mouseup", "1")
    _, panned = capture("-inline-drag-panned", lambda actual: not restored(zoomed, actual))
    # Native diagram geometry must translate, not just a surrounding frame.
    # Top/left edges may already touch the viewport and clip during this drag.
    # The far node edges remain visible and must translate by both deltas.
    assert abs(panned.bounds[2] - zoomed.bounds[2] + 15) <= 2, (zoomed.bounds, panned.bounds)
    assert abs(panned.bounds[3] - zoomed.bounds[3] + 10) <= 2, (zoomed.bounds, panned.bounds)
    mouse("mousemove", *initial.point, "click", "5")
    _, wheel_panned = capture("-inline-wheel-panned", lambda actual: not restored(panned, actual))
    # Zooming back down clamps to Fit and restores every diagram pixel.
    mouse("mousemove", *initial.point, "keydown", "ctrl", "click", "--repeat", "12", "--delay", "30", "5", "keyup", "ctrl")
    capture("-inline-fit", lambda actual: restored(initial, actual))
    mouse("mousemove", *initial.point, "click", "1", "mousemove", "16", "400")
    expanded_frame, expanded = capture("-inline-expanded", lambda actual: actual.height > initial.height * 1.05)
    sidebar = (0, 48, 216, before.height)
    changed = ImageChops.difference(before.crop(sidebar), expanded_frame.crop(sidebar)).convert("L")
    assert sum(changed.histogram()[21:]) < 250, "Diagram click opened a separate overlay"
    mouse("mousemove", *expanded.point, "click", "1", "mousemove", "16", "400")
    capture("-inline-collapsed", lambda actual: restored(initial, actual))
    print(f"Mermaid inline acceptance passed: native pixels {initial.count} -> {zoomed.count}, "
          f"drag bounds {zoomed.bounds} -> {panned.bounds}, wheel bounds={wheel_panned.bounds}; "
          "Ctrl-wheel zoom, two-axis drag pan, scroll pan, Fit restoration, inline expand/collapse, sidebar preserved. "
          "Pinch anchoring, draft focus and momentum routing are covered by the GPUI regression test.")
