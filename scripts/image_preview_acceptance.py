"""Exercise real image clicks on screenshot.py's private X11 display."""
import subprocess
import time
from PIL import Image


def chart_pixels(image):
    """Find the fixture's blue bar, independent of panel or image geometry."""
    points = []
    for y in range(image.height):
        for x in range(280, image.width):
            r, g, b = image.getpixel((x, y))[:3]
            if abs(r - 92) <= 4 and abs(g - 124) <= 4 and abs(b - 173) <= 4:
                points.append((x, y))
    if not points:
        raise AssertionError("Image fixture's blue bar did not paint")
    xs, ys = zip(*points)
    return len(points), ((min(xs) + max(xs)) // 2, (min(ys) + max(ys)) // 2)


def verify(output, env, root):
    before = Image.open(output).convert("RGB")
    initial_count, initial_point = chart_pixels(before)

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

    click(initial_point)
    enlarged_count, _ = capture("-enlarged", lambda count: count > initial_count * 2)
    subprocess.run(["xdotool", "key", "Escape"], env=env, cwd=root, check=True, timeout=10)
    capture("-escape-closed", lambda count: abs(count - initial_count) < initial_count * .05)
    click(initial_point)
    _, enlarged_point = capture("-reopened", lambda count: count > initial_count * 2)
    click(enlarged_point)
    capture("-click-closed", lambda count: abs(count - initial_count) < initial_count * .05)
    print(f"Image preview native acceptance passed: blue-bar pixels {initial_count} -> {enlarged_count}; Escape and click restore the thumbnail")
