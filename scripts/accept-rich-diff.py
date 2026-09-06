#!/usr/bin/env python3
"""Exercise the rich Markdown diff viewer through native clicks on private X11."""
import csv
import io
from pathlib import Path
import subprocess
import sys
import time

from PIL import Image
import screenshot


def verify(output, env, root, run):
    fixture = Path(__file__).resolve().parents[1] / "assets/previews/change-review.diff"
    expected_files = []
    for section in fixture.read_text().split("diff --git ")[1:]:
        lines = section.splitlines()
        path = lines[0].split(" b/", 1)[1]
        expected_files.append("Edit: " + path + "\n" + "\n".join(lines[3:]) + "\n")
    expected_copy = "\n".join(expected_files)

    def clipboard():
        # Read the real X11 clipboard on the private display, not an app test hook.
        program = """import gi, sys
gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gtk, Gdk
text = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()
assert text is not None, 'No native clipboard text'
sys.stdout.write(text)
"""
        result = run([sys.executable, "-c", program], env={**env, "GDK_BACKEND": "x11"}, cwd=root,
                     capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, result.stderr
        return result.stdout

    def words(path):
        # Upscale small native text for OCR, then map coordinates back to X11.
        ocr_image = root / "diff-ocr.png"
        with Image.open(path) as image:
            image.resize((image.width * 2, image.height * 2)).save(ocr_image)
        result = run(["tesseract", str(ocr_image), "stdout", "tsv"],
                     env={**env, "OMP_THREAD_LIMIT": "1"},
                     cwd=root, check=True, capture_output=True, text=True, timeout=30)
        rows = [row for row in csv.DictReader(io.StringIO(result.stdout), delimiter="\t", quoting=csv.QUOTE_NONE)
                if row.get("text", "").strip()]
        for row in rows:
            for field in ("left", "top", "width", "height"):
                row[field] = str(int(row[field]) // 2)
        return rows

    def capture(suffix):
        path = output.with_name(output.stem + suffix + ".png")
        time.sleep(0.4)
        run(["import", "-window", "root", "png:" + str(path)],
            env=env, cwd=root, check=True, timeout=15)
        return words(path)

    def click(rows, label, occurrence=0):
        matches = [row for row in rows if label.lower() in row["text"].lower()]
        assert len(matches) > occurrence, (label, [row["text"] for row in rows])
        word = matches[occurrence]
        x = int(word["left"]) + int(word["width"]) // 2
        y = int(word["top"]) + int(word["height"]) // 2
        run(["xdotool", "mousemove", str(x), str(y), "click", "1", "mousemove", "100", "100"],
            env=env, cwd=root, check=True, timeout=10)

    initial = words(output)
    deadline = time.monotonic() + 30
    attempt = 0
    while not initial and time.monotonic() < deadline:
        attempt += 1
        time.sleep(0.5)
        initial = capture(f"-ready-{attempt}")
    if not initial:
        diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
        raise AssertionError("App never painted text: " + (diagnostics.read_text() if diagnostics.exists() else "no diagnostics"))
    text = " ".join(row["text"] for row in initial)
    assert "src/session.rs" in text and "tests/session.rs" in text, text
    assert "Untitled" in text and "chars" in text, text
    click(initial, "Split")
    split = capture("-split")
    text = " ".join(row["text"] for row in split)
    def is_old_header(row):
        return row["text"].lower().replace("0", "o") == "old"

    assert any(is_old_header(row) for row in split) and any(row["text"] == "New" for row in split), text
    click(split, "Wrap:")
    nowrap = capture("-nowrap")
    assert "off" in " ".join(row["text"] for row in nowrap)
    click(nowrap, "Copy", 0)
    copied = capture("-copied")
    assert any("Copied" in row["text"] for row in copied)
    assert clipboard() == expected_copy, "Native clipboard changed or omitted diff content"
    click(copied, "Collapse", 0)
    collapsed = capture("-collapsed")
    assert any("Expand" in row["text"] for row in collapsed)
    assert not any("to_owned" in row["text"] for row in collapsed)
    click(collapsed, "Cop", 0)
    assert clipboard() == expected_copy, "Collapsing a file must not remove its copied content"
    click(collapsed, "Expand", 0)
    expanded = capture("-expanded")
    assert any("to_owned" in row["text"] for row in expanded)
    click(expanded, "Unified")
    unified = capture("-unified")
    assert not any(is_old_header(row) for row in unified)
    print(f"Rich diff acceptance passed: two files, syntax text, native split/unified, wrapping, copy feedback, collapse and expand. Native clipboard exactly preserved {len(expected_copy.encode())} bytes across both files before and after collapse.")


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: accept-rich-diff.py OUTPUT.png")
    output = Path(sys.argv[1]).resolve()
    original_run = subprocess.run
    checked = False

    def capture_and_verify(command, *args, **kwargs):
        nonlocal checked
        result = original_run(command, *args, **kwargs)
        if not checked and command == ["import", "-window", "root", "png:" + str(output)]:
            checked = True
            verify(output, kwargs["env"], kwargs["cwd"], original_run)
        return result

    subprocess.run = capture_and_verify
    try:
        sys.argv = ["screenshot.py", str(output), "--no-build", "--transcript", "diff-rich"]
        screenshot.main()
        assert checked, "capture hook did not run"
    finally:
        subprocess.run = original_run


if __name__ == "__main__":
    main()
