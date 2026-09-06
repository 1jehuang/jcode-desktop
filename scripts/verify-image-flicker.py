#!/usr/bin/env python3
"""Rerun every isolated image-flicker acceptance check over one built result."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        parser.error("output directory must be empty")
    env = dict(os.environ, MOLD_JOBS="2", CARGO_BUILD_JOBS="1")
    results = []

    def run(name, command):
        print('JCODE_PROGRESS ' + json.dumps({"message": name}), flush=True)
        started = time.monotonic()
        with (output / (name + ".log")).open("w") as log:
            completed = subprocess.run(command, cwd=repo, env=env, stdout=log,
                                       stderr=subprocess.STDOUT, timeout=450)
        results.append({"check": name, "exit_code": completed.returncode,
                        "seconds": round(time.monotonic() - started, 2)})
        (output / "results.json").write_text(json.dumps(results, indent=2))
        return completed.returncode == 0

    if not run("build-app", ["cargo", "build", "-p", "jcode-desktop", "-p", "jcode-desktop-ui"]):
        raise SystemExit(1)
    if not run("build-tests", ["cargo", "test", "-p", "jcode-desktop-ui", "--lib",
                               "--no-run", "--message-format=json"]):
        raise SystemExit(1)
    executable = None
    for line in (output / "build-tests.log").read_text().splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("reason") == "compiler-artifact" and event.get("executable"):
            if event["target"]["name"] == "jcode_desktop_ui":
                executable = event["executable"]
    if executable is None:
        raise SystemExit("Cargo did not report the UI test executable")
    # Other agents may rebuild target/debug. Keep all acceptance checks on the
    # exact artifacts built above rather than following those replacements.
    import shutil
    artifacts = output / "artifacts"
    artifacts.mkdir()
    for name, source in [("ui-tests", Path(executable)),
                         ("jcode-desktop", repo / "target/debug/jcode-desktop"),
                         ("libjcode_desktop_ui.so", repo / "target/debug/libjcode_desktop_ui.so")]:
        shutil.copy2(source, artifacts / name)
    executable = str(artifacts / "ui-tests")
    for name, query in [("cache-diagnostics", "image_cache::"),
                        ("geometry-diagnostics", "panel::flicker"),
                        ("streaming-image-matrix", "image_flicker_tests"),
                        ("streaming-neighbors", "stream"),
                        ("selection-neighbors", "text_selection::"),
                        ("layout-neighbors", "startup_lifecycle_tests")]:
        run(name, [executable, query, "--nocapture", "--test-threads=1"])
    run("fixture-arguments", ["python3", "-m", "unittest", "discover", "-s", "scripts", "-p", "test_screenshot.py"])
    for name, options in [
        ("one-panel", ["--transcript", "image", "--image-cache-interact"]),
        ("two-panels", ["--transcript", "image", "--panels", "2", "--image-cache-interact"]),
        ("preview-lifecycle", ["--transcript", "image", "--image-interact"]),
        ("html-controls", ["--transcript", "html", "--html-interact"]),
    ]:
        run(name, ["python3", "scripts/screenshot.py", str(output / (name + ".png")),
                   "--binary", str(artifacts / "jcode-desktop"), "--no-build", *options])
    run("reload-pixels", ["python3", "scripts/accept-mermaid.py", "--output-dir", str(output / "reload-pixels"),
                          "--binary", str(artifacts / "jcode-desktop"),
                          "--plugin", str(artifacts / "libjcode_desktop_ui.so")])
    print(json.dumps(results, indent=2), flush=True)
    raise SystemExit(any(result["exit_code"] != 0 for result in results))


if __name__ == "__main__":
    main()
