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
                     env={**env, "OMP_THREAD_LIMIT": "1"}, cwd=root, check=True, capture_output=True,
                     text=True, timeout=30)
        return [row for row in csv.DictReader(io.StringIO(result.stdout), delimiter="\t", quoting=csv.QUOTE_NONE)
                if row.get("text", "").strip()]

    def click(word):
        x = int(word["left"]) + int(word["width"]) // 2
        y = int(word["top"]) + int(word["height"]) // 2
        run(["xdotool", "mousemove", str(x), str(y), "click", "1", "mousemove", "100", "100"],
            env=env, cwd=root, check=True, timeout=10)

    def text(rows):
        return " ".join(row["text"] for row in rows)

    def reviews(rows):
        return [row for row in rows if row["text"] == "Review" and int(row["left"]) > 1200]

    def observe(suffix, predicate):
        deadline = time.monotonic() + 30
        while True:
            rows = words(capture(suffix))
            if predicate(rows):
                return rows
            if time.monotonic() >= deadline:
                raise AssertionError(f"Timed out waiting for {suffix}: {text(rows)}")

    initial = observe("-ready", lambda rows: len(reviews(rows)) == 2)
    click(reviews(initial)[0])
    opened_words = observe("-review", lambda rows: any(row["text"] == "Back" for row in rows) and "Copy diff" in text(rows))
    assert "src/navigation.rs" in text(opened_words), text(opened_words)
    assert "Copy diff" in text(opened_words), text(opened_words)
    files = [w for w in opened_words if "navigation.rs" in w["text"] and int(w["left"]) < 520]
    assert len(files) == 2, "both changed files must appear in the tree"
    click(files[1])
    selected = observe("-selected", lambda rows: "tests/navigation.rs" in text(rows))
    assert "navigation_label_is_clear" in text(selected), text(selected)
    run(["xdotool", "key", "Escape"], env=env, cwd=root, check=True, timeout=10)
    closed = observe("-closed", lambda rows: len(reviews(rows)) == 2)
    click(reviews(closed)[1])
    reopened = observe("-reopened", lambda rows: any(row["text"] == "Back" for row in rows) and "Copy diff" in text(rows))
    click(next(w for w in reopened if w["text"] == "Back"))
    observe("-back", lambda rows: len(reviews(rows)) == 2)
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
