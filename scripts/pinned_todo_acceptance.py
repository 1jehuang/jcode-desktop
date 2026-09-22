"""Exercise the real pinned todo on screenshot.py's private Xvfb display."""
import subprocess
import time


def verify(output, env):
    from PIL import Image, ImageChops

    def capture(suffix):
        path = output.with_name(output.stem + suffix + ".png")
        subprocess.run(["import", "-window", "root", str(path)], env=env, check=True)
        return path

    def click():
        subprocess.run(["xdotool", "mousemove", "600", "58", "click", "1"],
                       env=env, check=True)
        time.sleep(2)

    time.sleep(2)  # Let the task-label type-in animation settle.
    collapsed = capture("-collapsed")
    click()
    expanded = capture("-expanded")
    # Compare the exact header paper, badge, typography and progress indicators.
    # Exclude only the disclosure chevron, which should change direction.
    header = (245, 40, 1400, 75)
    with Image.open(collapsed) as before, Image.open(expanded) as after:
        assert ImageChops.difference(before.crop(header), after.crop(header)).getbbox() is None, \
            "expansion changed the compact todo header"
        assert ImageChops.difference(before.crop((274, 78, 1400, 180)),
                                     after.crop((274, 78, 1400, 180))).getbbox(), \
            "expansion did not reveal task details"
        # The details use the exact same paper color as the compact task pill.
        assert before.getpixel((1300, 50)) == after.getpixel((1300, 90)), \
            "expanded details do not match the compact paper background"
    text = subprocess.check_output(["tesseract", str(expanded), "stdout"],
                                   env=env, stderr=subprocess.DEVNULL).decode()
    assert "Keep the pinned plan compact" in text, text
    click()
    restored = capture("-recollapsed")
    with Image.open(collapsed) as before, Image.open(restored) as after:
        assert ImageChops.difference(before.crop(header), after.crop(header)).getbbox() is None
    print("Pinned todo: stable header, matching expanded paper, and native toggle verified")
