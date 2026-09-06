#!/usr/bin/env python3
"""Exercise diff metadata and its file tree in the real app on a private display."""
import csv
import io
from pathlib import Path
import subprocess
import sys
import time

import screenshot


def verify(output, env, root, run):
    def capture(suffix):
        path = output.with_name(output.stem + suffix + ".png")
        time.sleep(0.4)
        run(["import", "-window", "root", "png:" + str(path)],
            env=env, cwd=root, check=True, timeout=15)
        return path

    def words(path):
        result = run(["tesseract", str(path), "stdout", "tsv"],
                     env=env, cwd=root, check=True, capture_output=True,
                     text=True, timeout=30)
        return [row for row in csv.DictReader(io.StringIO(result.stdout), delimiter="\t")
                if row.get("text", "").strip()]

    def click(word):
        x = int(word["left"]) + int(word["width"]) // 2
        y = int(word["top"]) + int(word["height"]) // 2
        run(["xdotool", "mousemove", str(x), str(y), "click", "1", "mousemove", "100", "100"],
            env=env, cwd=root, check=True, timeout=10)

    initial = words(output)
    reviews = [word for word in initial if word["text"] == "Review"]
    assert len(reviews) == 2, "both files must have review metadata"
    click(reviews[0])
    opened = capture("-review")
    opened_words = words(opened)
    text = " ".join(w["text"] for w in opened_words)
    assert "Change review" in text, text
    assert "src/navigation.rs" in text, text
    assert "Continue" in text, text
    files = [w for w in opened_words if "navigation.rs" in w["text"] and int(w["left"]) < 520]
    assert len(files) == 2, "both changed files must appear in the tree"
    click(files[1])
    selected = capture("-selected")
    text = " ".join(w["text"] for w in words(selected))
    assert "tests/navigation.rs" in text, text
    assert "navigation_label_is_clear" in text, text
    run(["xdotool", "key", "Escape"], env=env, cwd=root, check=True, timeout=10)
    closed = capture("-closed")
    closed_words = words(closed)
    assert len([w for w in closed_words if w["text"] == "Review"]) == 2
    click([w for w in closed_words if w["text"] == "Review"][1])
    reopened = capture("-reopened")
    reopened_words = words(reopened)
    click(next(w for w in reopened_words if w["text"] == "Back"))
    final = capture("-back")
    assert len([w for w in words(final) if w["text"] == "Review"]) == 2
    print("Diff acceptance passed: two metadata links, full diff, tree file selection, Escape and Back to chat.")


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: accept-diff.py OUTPUT.png")
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
        sys.argv = ["screenshot.py", str(output), "--no-build", "--transcript", "diff"]
        screenshot.main()
        assert checked, "capture hook did not run"
    finally:
        subprocess.run = original_run


if __name__ == "__main__":
    main()
