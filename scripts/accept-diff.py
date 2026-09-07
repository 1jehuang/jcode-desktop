#!/usr/bin/env python3
"""Exercise diff metadata and its file tree in the real app on a private display."""
import csv
import io
import json
from pathlib import Path
import subprocess
import sys
import time

from PIL import Image

import screenshot


def verify(output, env, root, run):
    def capture(suffix):
        path = output.with_name(output.stem + suffix + ".png")
        time.sleep(0.4)
        run(["import", "-window", "root", "png:" + str(path)],
            env=env, cwd=root, check=True, timeout=15)
        return path

    def words(path):
        ocr_path = root / "review-ocr.png"
        with Image.open(path) as image:
            image.resize((image.width * 2, image.height * 2)).save(ocr_path)
        result = run(["tesseract", str(ocr_path), "stdout", "tsv"],
                     env={**env, "OMP_THREAD_LIMIT": "1"}, cwd=root, check=True, capture_output=True,
                     text=True, timeout=30)
        rows = [row for row in csv.DictReader(io.StringIO(result.stdout), delimiter="\t", quoting=csv.QUOTE_NONE)
                if row.get("text", "").strip()]
        for row in rows:
            for field in ("left", "top", "width", "height"):
                row[field] = str(int(row[field]) // 2)
        return rows

    def click(word):
        x = int(word["left"]) + int(word["width"]) // 2
        y = int(word["top"]) + int(word["height"]) // 2
        run(["xdotool", "mousemove", str(x), str(y), "click", "1", "mousemove", "100", "100"],
            env=env, cwd=root, check=True, timeout=10)

    def text(rows):
        return " ".join(row["text"] for row in rows)

    def reviews(rows):
        return [row for row in rows if row["text"] == "Review" and int(row["left"]) > 276 and int(row["top"]) > 130]

    def observe(suffix, predicate):
        deadline = time.monotonic() + 30
        while True:
            rows = words(capture(suffix))
            if predicate(rows):
                return rows
            if time.monotonic() >= deadline:
                raise AssertionError(f"Timed out waiting for {suffix}: {text(rows)}")

    def panels():
        state = Path(env["JCODE_DESKTOP_STATE"]).read_text()
        navigation = json.loads(next(line.removeprefix("navigation=")
                                     for line in state.splitlines() if line.startswith("navigation=")))
        return [panel for row in navigation["rows"] for panel in row["panels"] if not panel["closing"]]

    initial = observe("-ready", lambda rows: len(reviews(rows)) == 2)
    original = panels()[0]
    click(reviews(initial)[0])
    opened_words = observe("-review", lambda rows: any(row["text"] == "Close" for row in rows) and "Copy diff" in text(rows))
    opened_panels = panels()
    assert len(opened_panels) == 2, opened_panels
    assert opened_panels[0]["id"] == original["id"], "Review replaced the chat"
    assert opened_panels[0]["history_items"] == original["history_items"], "Review changed the transcript"
    assert opened_panels[1]["session"].startswith("review://"), opened_panels
    assert opened_panels[1]["focused"], "Review did not receive focus"
    assert "src/navigation.rs" in text(opened_words), text(opened_words)
    assert "Copy diff" in text(opened_words), text(opened_words)
    files = [w for w in opened_words if "navigation.rs" in w["text"] and int(w["left"]) > 700]
    tree_left = min(int(w["left"]) for w in files)
    files = [w for w in files if int(w["left"]) < tree_left + 40]
    assert len(files) == 2, "both changed files must appear in the tree"
    click(files[1])
    selected = observe("-selected", lambda rows: "tests/navigation.rs" in text(rows))
    assert "navigation_label_is_clear" in text(selected), text(selected)
    run(["xdotool", "key", "Escape"], env=env, cwd=root, check=True, timeout=10)
    closed = observe("-closed", lambda rows: len(reviews(rows)) == 2)
    assert len(panels()) == 1 and panels()[0]["id"] == original["id"]
    click(reviews(closed)[1])
    reopened = observe("-reopened", lambda rows: any(row["text"] == "Close" for row in rows) and "Copy diff" in text(rows))
    click(next(w for w in reopened if w["text"] == "Close"))
    observe("-back", lambda rows: len(reviews(rows)) == 2)
    assert len(panels()) == 1 and panels()[0]["id"] == original["id"]
    print("Diff acceptance passed: Review opens a separate focused panel, preserves the chat, supports file selection, and closes by Escape and Close review.")


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
