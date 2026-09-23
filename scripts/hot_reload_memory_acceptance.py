#!/usr/bin/env python3
"""No-build, private-Xvfb acceptance for a prebuilt release host + UI cdylib.

Example:
  python3 scripts/hot_reload_memory_acceptance.py --no-build \
    --host /path/to/release/jcode-desktop \
    --plugin /path/to/release/libjcode_desktop_ui.so

The pair is copied and SHA256-pinned before launch. No Cargo build is performed:
only the copied fixture receives CARGO=/usr/bin/true, so the real host's R socket
command takes its normal snapshot/load/activate path using the pinned plugin.
Each generation gets its own warmup and >=30s passive /proc sampling window.
A final F6 rollback reuses a retained generation and gets its own measured window.
Retained library mappings and activation steps are reported separately, not
charged against the within-generation growth budget. Linked mode is a control.
All artifacts, including failed runs, stay beneath --output. No live UI/state,
credentials, compositor IPC, shared target writes, or source edits are used.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import time

from screenshot import isolated_env

DRAFT = "Memory reload draft survives"


def growth_metrics(rows):
    """OLS slope plus worst positive increase from the first measured RSS."""
    if len(rows) < 2:
        raise ValueError("at least two RSS samples required")
    xs = [row["seconds"] for row in rows]
    ys = [row["rss_mib"] for row in rows]
    xbar, ybar = sum(xs) / len(xs), sum(ys) / len(ys)
    denominator = sum((x - xbar) ** 2 for x in xs)
    if denominator <= 0:
        raise ValueError("sample timestamps must span positive time")
    return {
        "duration_seconds": xs[-1] - xs[0],
        "slope_mib_per_second": sum((x - xbar) * (y - ybar)
                                    for x, y in zip(xs, ys)) / denominator,
        "growth_mib": max(0.0, max(ys) - ys[0]),
        "start_mib": ys[0], "end_mib": ys[-1], "peak_mib": max(ys),
    }


def growth_failures(metrics, slope_limit, growth_limit):
    failures = []
    if metrics["slope_mib_per_second"] >= slope_limit:
        failures.append(f"RSS slope {metrics['slope_mib_per_second']:.3f} >= {slope_limit} MiB/s")
    if metrics["growth_mib"] >= growth_limit:
        failures.append(f"RSS growth {metrics['growth_mib']:.3f} >= {growth_limit} MiB")
    return failures


def plugin_mappings(text):
    # A library has several ELF mappings. Count unique file paths, not regions.
    return sorted({line.split(maxsplit=5)[5].removesuffix(" (deleted)")
                   for line in text.splitlines()
                   if len(line.split(maxsplit=5)) == 6
                   and re.search(r"/jcode-desktop-ui-\d+\.so(?: \(deleted\))?$", line)})


def host_diagnostics(root):
    """Host redirects stderr here after diagnostics::install, not app.log."""
    path = root / "logs/jcode-desktop/jcode-desktop.log"
    try:
        return path.read_text()
    except FileNotFoundError:
        return ""


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def pin_pair(host, plugin, directory):
    directory.mkdir(mode=0o700)
    manifest = {}
    for source, name in ((host, "jcode-desktop"), (plugin, "libjcode_desktop_ui.so")):
        source = source.resolve(strict=True)
        before = sha256(source)
        destination = directory / name
        shutil.copy2(source, destination)
        after = sha256(destination)
        if before != after or sha256(source) != before:
            raise RuntimeError(f"Input changed while pinning: {source}")
        manifest[name] = {"source": str(source), "sha256": after, "bytes": destination.stat().st_size}
    return manifest


def stop_processes(processes):
    # Each child owns a fresh process group, including any helpers it spawns.
    for process in reversed(processes):
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    for process in reversed(processes):
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def run_mode(mode, pair, root, args):
    root.mkdir(mode=0o700)
    for name in ("home", "runtime", "config", "cache", "data", "jcode", "tmp", "logs"):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.update({"TMPDIR": str(root / "tmp"), "CARGO": "/usr/bin/true",
                "CARGO_TARGET_DIR": str(root / "unused-target"),
                "JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT": "streaming",
                "VK_DRIVER_FILES": str(args.driver)})
    if mode == "plugin":
        env["JCODE_DESKTOP_UI"] = str(pair / "libjcode_desktop_ui.so")
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc">'
                         '<applications><application class="*"><decor>no</decor>'
                         '<maximized>yes</maximized></application></applications></openbox_config>')
    processes = []
    read_fd, write_fd = os.pipe()
    report = {"mode": mode, "windows": [], "failures": [], "draft_check": args.draft_check}
    started = time.monotonic()
    app = None
    try:
        with (root / "app.log").open("w") as log, (root / "samples.jsonl").open("w") as samples:
            def spawn(command, **kwargs):
                child = subprocess.Popen(command, env=env, cwd=root, stdout=log,
                                         stderr=log, start_new_session=True, **kwargs)
                processes.append(child)
                return child

            def alive():
                for child in processes:
                    if child.poll() is not None:
                        raise RuntimeError(f"Fixture child {child.pid} exited: {child.returncode}")

            def wait_for(predicate, description):
                deadline = time.monotonic() + args.startup_timeout
                while True:
                    alive()
                    if predicate():
                        return
                    if time.monotonic() >= deadline:
                        raise RuntimeError("Timed out: " + description)
                    time.sleep(.1)

            def command(*argv):
                return subprocess.check_output(list(map(str, argv)), env=env, cwd=root,
                                               stderr=log, timeout=20)

            def capture(label, check_draft=False):
                path = root / (label + ".png")
                command("import", "-window", "root", "png:" + str(path))
                if check_draft:
                    text = command("tesseract", path, "stdout", "--psm", "11").decode()
                    (root / (label + ".ocr.txt")).write_text(text)
                    if DRAFT.lower() not in " ".join(text.lower().split()):
                        raise RuntimeError(f"Draft missing from rendered OCR: {label}")
                return path

            def mappings():
                return plugin_mappings(Path(f"/proc/{app.pid}/maps").read_text())

            def activation_count():
                return len(re.findall(r"activated UI generation \d+ from ", host_diagnostics(root)))

            def sample(generation, phase):
                alive()
                status = Path(f"/proc/{app.pid}/status").read_text()
                rss = re.search(r"^VmRSS:\s+(\d+)\s+kB$", status, re.M)
                if not rss:
                    raise RuntimeError("Host VmRSS unavailable")
                row = {"seconds": time.monotonic() - started, "rss_mib": int(rss[1]) / 1024,
                       "generation": generation, "phase": phase}
                samples.write(json.dumps(row) + "\n")
                samples.flush()
                return row

            spawn(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24",
                   "-nolisten", "tcp"], pass_fds=(write_fd,))
            os.close(write_fd)
            write_fd = None
            if not select.select([read_fd], [], [], args.startup_timeout)[0]:
                raise RuntimeError("Xvfb startup timeout")
            display = os.read(read_fd, 64).decode().strip()
            if not display.isdigit():
                raise RuntimeError("Invalid private Xvfb display")
            env["DISPLAY"] = ":" + display
            spawn(["openbox", "--sm-disable", "--config-file", str(wm_config)])
            time.sleep(.5)
            app = spawn([str(pair / "jcode-desktop"), *(["--no-hot-reload"] if mode == "linked" else [])])
            state = root / "state"
            wait_for(lambda: state.exists() and "widths=" in state.read_text(), "first offline render")
            if mode == "plugin":
                wait_for(lambda: activation_count() == 1 and len(mappings()) == 1, "initial plugin activation")
            elif mappings():
                raise RuntimeError("Linked control unexpectedly loaded a plugin")
            # Never press Escape: it stops the streaming workload. The beta
            # toast dismisses itself without changing response state.
            time.sleep(.5)
            if args.draft_check:
                # Screenshot fixture starts with the composer focused. Do not
                # submit the draft or use a clipboard shared with another display.
                command("xdotool", "type", "--clearmodifiers", "--delay", "30", DRAFT)
                time.sleep(.5)
                capture("draft-before", True)
            previous_end = None
            cycles = args.reloads + 1 + int(args.rollback) if mode == "plugin" else 1
            for generation in range(cycles):
                rolling_back = mode == "plugin" and generation > args.reloads
                action = "rollback" if rolling_back else ("reload" if generation else "initial")
                if rolling_back:
                    before = mappings()
                    rollback_count = host_diagnostics(root).count("rolled back UI to ")
                    command("xdotool", "key", "--clearmodifiers", "F6")
                    wait_for(lambda: host_diagnostics(root).count("rolled back UI to ")
                             == rollback_count + 1, "F6 rollback activation")
                    if mappings() != before:
                        raise RuntimeError("Rollback loaded a new library instead of reusing a generation")
                elif generation:
                    before = set(mappings())
                    count = activation_count()
                    socket_path = root / "runtime/jcode-desktop.sock"
                    with socket.socket(socket.AF_UNIX) as control:
                        control.settimeout(5)
                        control.connect(str(socket_path))
                        control.sendall(b"R")
                        response = b""
                        while len(response) < 3:
                            chunk = control.recv(3 - len(response))
                            if not chunk:
                                break
                            response += chunk
                        if response != b"ok\n":
                            raise RuntimeError(f"Bad private reload acknowledgement: {response!r}")
                    # Socket ACK means queued, not completed. Require successful
                    # activation log AND exactly one new retained library path.
                    wait_for(lambda: activation_count() == count + 1 and
                             len(set(mappings()) - before) == 1, "reload activation and new mapping")
                expected = min(generation + 1, args.reloads + 1) if mode == "plugin" else 0
                if len(mappings()) != expected:
                    raise RuntimeError(f"Unexpected retained generation count: {mappings()}")
                deadline = time.monotonic() + args.warmup
                while time.monotonic() < deadline:
                    sample(generation, "warmup")
                    time.sleep(min(args.interval, max(0, deadline - time.monotonic())))
                rows = [sample(generation, "measured")]
                deadline = time.monotonic() + args.seconds
                while time.monotonic() < deadline:
                    time.sleep(min(args.interval, max(0, deadline - time.monotonic())))
                    rows.append(sample(generation, "measured"))
                if len(mappings()) != expected:
                    raise RuntimeError("Plugin generation count changed during measurement")
                metrics = growth_metrics(rows)
                failures = growth_failures(metrics, args.max_slope, args.max_growth)
                report["failures"].extend(f"generation {generation}: {item}" for item in failures)
                report["windows"].append({"generation": generation, "action": action,
                                          "active_plugin_generation": args.reloads if rolling_back else expected,
                                          "metrics": metrics,
                                          "mapped_generations": mappings(),
                                          "between_generation_step_mib": None if previous_end is None
                                          else metrics["start_mib"] - previous_end})
                previous_end = metrics["end_mib"]
                (root / f"generation-{generation}.maps").write_text(Path(f"/proc/{app.pid}/maps").read_text())
                if state.exists():
                    shutil.copy2(state, root / f"generation-{generation}.state")
                capture(f"generation-{generation}", args.draft_check)
                print(json.dumps({"mode": mode, "generation": generation, **metrics}), flush=True)
            if mode == "plugin" and activation_count() != args.reloads + 1:
                raise RuntimeError("Unexpected extra activation during measured windows")
    except KeyboardInterrupt:
        report["failures"].append("Interrupted, fixture children cleaned up")
        raise
    except Exception as error:
        report["failures"].append(str(error))
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        stop_processes(processes)
        report["passed"] = not report["failures"]
        (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", "--binary", required=True, type=Path)
    parser.add_argument("--plugin", required=True, type=Path)
    parser.add_argument("--no-build", required=True, action="store_true", help="required safety acknowledgement")
    parser.add_argument("--output", type=Path, default=Path("target/hot-reload-memory"))
    parser.add_argument("--seconds", type=float, default=30)
    parser.add_argument("--warmup", type=float, default=10)
    parser.add_argument("--interval", type=float, default=1)
    parser.add_argument("--reloads", type=int, default=2)
    parser.add_argument("--rollback", action=argparse.BooleanOptionalAction, default=True,
                        help="measure F6 rollback to a retained generation after reloads (default enabled)")
    parser.add_argument("--max-slope", type=float, default=1)
    parser.add_argument("--max-growth", type=float, default=32)
    parser.add_argument("--startup-timeout", type=float, default=45)
    parser.add_argument("--draft-check", action=argparse.BooleanOptionalAction, default=True,
                        help="native typing plus screenshot OCR before and after reloads (default enabled)")
    args = parser.parse_args(argv)
    values = (args.seconds, args.warmup, args.interval, args.max_slope, args.max_growth, args.startup_timeout)
    if (not all(math.isfinite(value) and value > 0 for value in values) or
            args.seconds < 30 or args.reloads < 2 or args.interval > args.seconds / 2):
        parser.error("require finite positive limits, >=30 measured seconds, >=2 reloads, interval <= seconds/2")
    for name in ("Xvfb", "openbox", "xdotool", "import", "true", *(("tesseract",) if args.draft_check else ())):
        if not shutil.which(name, path="/usr/bin:/bin"):
            parser.error("missing prerequisite: " + name)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("Mesa lavapipe Vulkan driver required")
    args.driver = drivers[0]
    output = args.output.resolve()
    if output.exists():
        parser.error(f"refusing to overwrite artifacts: {output}")
    # Linux sockaddr_un has a 108-byte path limit. Check before spawning.
    if len(os.fsencode(output / "plugin/runtime/jcode-desktop.sock")) >= 108:
        parser.error("output path too long for private Unix socket")
    output.mkdir(parents=True, mode=0o700)
    report = {"passed": False, "scope": "private offline prebuilt release pair", "modes": []}
    try:
        report["pair"] = pin_pair(args.host, args.plugin, output / "pair")
        report["settings"] = {key: str(value) if isinstance(value, Path) else value
                              for key, value in vars(args).items()}
        (output / "pair.json").write_text(json.dumps(report["pair"], indent=2))
        for mode in ("linked", "plugin"):
            report["modes"].append(run_mode(mode, output / "pair", output / mode, args))
        report["passed"] = all(mode["passed"] for mode in report["modes"])
    except KeyboardInterrupt:
        report["error"] = "Interrupted, fixture children cleaned up"
    except Exception as error:
        report["error"] = str(error)
    finally:
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"{'PASS' if report['passed'] else 'FAIL'}: {output / 'report.json'}")
    return 0 if report["passed"] else 1


def interrupted(signum, frame):
    """SIGTERM follows the same finally cleanup path as Ctrl+C."""
    raise KeyboardInterrupt


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    raise SystemExit(main())
