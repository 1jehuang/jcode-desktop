#!/usr/bin/env python3
"""Exercise the rich Markdown diff viewer through native clicks on private X11."""
import csv
import io
from pathlib import Path
import subprocess
import sys
import time

import screenshot


def verify(output, env, root, run):
    def words(path):
        result = run(["tesseract", str(path), "stdout", "tsv"], env=env,
                     cwd=root, check=True, capture_output=True, text=True, timeout=30)
        return [row for row in csv.DictReader(io.StringIO(result.stdout), delimiter="\t")
                if row.get("text", "").strip()]

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
    text = " ".join(row["text"] for row in initial)
    assert "src/session.rs" in text and "tests/session.rs" in text, text
    assert "Untitled" in text and "chars" in text, text
    click(initial, "Split")
    split = capture("-split")
    text = " ".join(row["text"] for row in split)
    assert "Before" in text and "After" in text, text
    click(split, "Wrap:")
    nowrap = capture("-nowrap")
    assert "off" in " ".join(row["text"] for row in nowrap)
    click(nowrap, "Copy", 0)
    copied = capture("-copied")
    assert any("Copied" in row["text"] for row in copied)
    click(copied, "Collapse", 0)
    collapsed = capture("-collapsed")
    assert any("Expand" in row["text"] for row in collapsed)
    assert not any("to_owned" in row["text"] for row in collapsed)
    click(collapsed, "Expand", 0)
    expanded = capture("-expanded")
    assert any("to_owned" in row["text"] for row in expanded)
    click(expanded, "Unified")
    unified = capture("-unified")
    assert not any(row["text"] in ("Before", "After") for row in unified)
    print("Rich diff acceptance passed: two files, syntax text, native split/unified, wrapping, copy feedback, collapse and expand.")


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
