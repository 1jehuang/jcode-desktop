"""Native-input acceptance checks for screenshot.py --html-interact.

Coordinates come from the real browser surface's white canvas in the screenshot,
not a second HTML renderer. All input goes through the private X11 app window.
"""
import subprocess
import time
from pathlib import Path
from PIL import Image, ImageChops


def surface(image):
    image = image.convert('RGB')
    cropped = image.crop((280, 100, image.width-20, image.height-50))
    r, g, b = cropped.split()
    white = ImageChops.darker(ImageChops.darker(r, g), b).point(lambda p: 255 if p == 255 else 0)
    box = white.getbbox()
    if box is None or box[2]-box[0] < 500 or box[3]-box[1] < 350:
        return None
    return box[0]+280, box[1]+100, box[2]+280, box[3]+100


def changed(a, b, box):
    diff = ImageChops.difference(a.convert('RGB').crop(box), b.convert('RGB').crop(box)).convert('L')
    return sum(count for shade, count in enumerate(diff.histogram()) if shade > 12)


def verify(output, env, root):
    before = Image.open(output).convert('RGB')
    box = surface(before)
    if box is None:
        raise RuntimeError('HTML card did not paint a browser surface')
    left, top, right, bottom = box
    def click(x, y):
        subprocess.run(['xdotool', 'mousemove', str(round(x)), str(round(y)), 'click', '1',
                        'mousemove', '100', '100'], env=env, cwd=root, check=True, timeout=10)
    def capture(suffix, predicate):
        path = output.with_name(output.stem + suffix + '.png')
        end = time.monotonic() + 12
        while time.monotonic() < end:
            time.sleep(.15)
            subprocess.run(['import', '-window', 'root', 'png:' + str(path)],
                           env=env, cwd=root, check=True, timeout=10)
            image = Image.open(path).convert('RGB')
            if predicate(image):
                return image
        raise RuntimeError('Native HTML interaction failed: ' + suffix + ' (' + str(path) + ')')

    # Read only the private X11 clipboard, never the user's desktop clipboard.
    click(right-124, top-17)
    clipboard = subprocess.check_output(['/usr/bin/python3', '-c',
        "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk,Gdk; "
        "print(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text() or '',end='')"],
        env=env, cwd=root, text=True, timeout=10)
    specimen = (Path(__file__).resolve().parents[1] / 'assets/previews/font-pairings.html').read_text().rstrip('\n')
    if clipboard != specimen:
        raise RuntimeError('Copy did not put the exact HTML source on the private clipboard')

    # Choosing a pairing visibly updates status and its selection border.
    click(right-65, top+216)
    status_box = (left+20, top+145, right-20, top+175)
    chosen = capture('-chosen', lambda image: changed(before, image, status_box) > 150)

    # The bundled font sampler has its slider 111 CSS px below the surface top.
    click(left+210, top+111)
    text_box = (left+20, top+260, right-20, bottom-10)
    selected = capture('-interactive', lambda image: changed(chosen, image, text_box) > 400)
    subprocess.run(['xdotool', 'key', 'Right'], env=env, cwd=root, check=True, timeout=10)
    keyed = capture('-keyboard', lambda image: changed(selected, image, text_box) > 100)
    selected = keyed

    # The HTML Reset button restores the original sample text size.
    click(left+310, top+111)
    capture('-reset', lambda image: changed(chosen, image, text_box) < 100)
    click(left+210, top+111)
    selected = capture('-resized', lambda image: changed(chosen, image, text_box) > 400)

    # Expand and collapse the *native* card, not the HTML inside it.
    click(right-78, top-17)
    expanded = capture('-expanded', lambda image: surface(image) is not None and surface(image)[3]-surface(image)[1] >= 690)
    expanded_box = surface(expanded)
    click(expanded_box[2]-78, expanded_box[1]-17)
    collapsed = capture('-collapsed', lambda image: surface(image) is not None and 410 <= surface(image)[3]-surface(image)[1] <= 430)
    left, top, right, bottom = surface(collapsed)

    # Pausing preserves the frame. Retry starts the original 17px document again.
    click(right-28, top-17)
    header = (right-60, top-32, right, top-2)
    paused = capture('-paused', lambda image: changed(collapsed, image, header) > 15)
    click(left+120, top+111)
    time.sleep(.4)
    capture('-paused-input', lambda image: changed(paused, image, (left, top, right, bottom)) == 0)
    click(right-28, top-17)
    capture('-restarted', lambda image: changed(selected, image, text_box) > 400)

    # Native wheel input reaches the browser rather than moving the transcript.
    click(left+700, top+180)
    subprocess.run(['xdotool', 'mousemove', str(left+700), str(top+300),
                    'click', '--repeat', '5', '--delay', '50', '5', 'mousemove', '100', '100'],
                   env=env, cwd=root, check=True, timeout=10)
    capture('-scrolled', lambda image: changed(before, image, text_box) > 400)

    # Escape relinquishes input. At the transcript's bottom, more down-wheel
    # events must neither scroll the browser further nor move the transcript.
    time.sleep(1.2)
    settled = capture('-scroll-settled', lambda image: True)
    subprocess.run(['xdotool', 'key', 'Escape', 'mousemove', str(left+700), str(top+300),
                    'click', '--repeat', '3', '--delay', '50', '5', 'mousemove', '100', '100'],
                   env=env, cwd=root, check=True, timeout=10)
    time.sleep(.5)
    escaped = capture('-escaped', lambda image: True)
    if changed(settled, escaped, (left, top, right, bottom)) > 100:
        raise RuntimeError('Escape did not release browser scroll focus')

    # Source remains native code with a copy control rather than executable HTML.
    click(right-169, top-17)
    capture('-source', lambda image: surface(image) is None)
    print('Native HTML acceptance passed: exact-source Copy, Choose, slider, keyboard, Reset, '
          'expand/collapse, paused input, retry, scroll, Escape, source view')
