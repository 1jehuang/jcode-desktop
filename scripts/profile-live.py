#!/usr/bin/env python3
"""Capture actual desktop window timing plus timestamp-correlated Linux process load.

No synthetic input, focus changes, compositor commands, restart, or redraw requests.
Enable once by rebuilding/reloading the UI, then run this against the live PID.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import uuid


def process_sample(pid):
    stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    io = dict(line.split(": ") for line in Path(f"/proc/{pid}/io").read_text().splitlines())
    status = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines())
    if 'VmRSS' not in status:
        raise ProcessLookupError(f"Desktop PID {pid} exited during capture")
    return {
        "unix_ms": int(time.time() * 1000),
        "main_cpu_ticks": int(stat[11]) + int(stat[12]),
        "main_minor_faults": int(stat[7]), "main_major_faults": int(stat[9]),
        "read_bytes": int(io["read_bytes"]), "write_bytes": int(io["write_bytes"]),
        "rss_kb": int(status["VmRSS"].split()[0]),
        "loadavg": Path("/proc/loadavg").read_text().strip(),
        "pressure": {name: Path(f"/proc/pressure/{name}").read_text().strip()
                     for name in ("cpu", "io", "memory")},
    }


def desktop_pid():
    candidates = []
    for path in Path("/proc").glob("[0-9]*/exe"):
        try:
            if os.readlink(path).removesuffix(" (deleted)").endswith("/jcode-desktop"):
                candidates.append(int(path.parent.name))
        except (OSError, PermissionError):
            pass
    if len(candidates) != 1:
        raise SystemExit(f"Expected one running desktop, found {candidates}. Use --pid.")
    return candidates[0]


def cadence_summary(frames):
    """Never infer FPS from idle wall time or invert a percentile as an average."""
    def weighted_mean(field, count):
        samples = [s for s in frames if s.get(field) is not None and s.get(count, 0)]
        total = sum(s[count] for s in samples)
        return sum(s[field] * s[count] for s in samples) / total if total else None
    present_mean = weighted_mean('animation_present_mean_ms', 'animation_present_count')
    return {
        'sample_windows': len(frames),
        'draws': sum(s['draw_count'] for s in frames),
        'draw_mean_ms': weighted_mean('draw_mean_ms', 'draw_count'),
        'animation_intervals': sum(s.get('animation_present_count', 0) for s in frames),
        'animation_mean_ms': present_mean,
        'animation_fps': 1000 / present_mean if present_mean else None,
        'inactive_windows': sum(s.get('window_active') is False for s in frames),
        'thermal_states': sorted({s['thermal_state'] for s in frames if s.get('thermal_state')}),
    }


def analyze(output):
    frames = [json.loads(line) for line in (output / "frames.jsonl").read_text().splitlines()]
    process = [json.loads(line) for line in (output / "process.jsonl").read_text().splitlines()]
    inputs = [sample for sample in frames if sample["input_frame_count"]]
    draws = sum(sample["draw_count"] for sample in frames)
    print(f"Live capture: {len(frames)} sampling windows, {draws} draws, "
          f"{sum(s['input_frame_count'] for s in inputs)} input-bearing frames")
    if not inputs:
        print("INCONCLUSIVE FOR INPUT LAG: no input-bearing frames. Repeat while performing the laggy action.")
    for field in ("ui_wake_lag_ms", "draw_max_ms", "input_max_ms", "animation_present_p95_ms"):
        values = [sample[field] for sample in frames if sample[field] is not None]
        print(f"{field}: maximum={max(values):.2f} ms" if values else f"{field}: no samples")
    print("Worst input windows (not pooled percentiles):")
    for sample in sorted(inputs, key=lambda s: s["input_max_ms"], reverse=True)[:10]:
        near = min(process, key=lambda p: abs(p["unix_ms"] - sample["unix_ms"]))
        print(json.dumps({"frame": sample, "system": near}))
    summary = {
        "sample_windows": len(frames), "draws": draws,
        "input_frames": sum(s["input_frame_count"] for s in inputs),
        "input_over_50ms_windows": sum(s["input_max_ms"] > 50 for s in inputs),
        "draw_over_16ms_windows": sum((s["draw_max_ms"] or 0) > 16.7 for s in frames),
        "wake_over_20ms_windows": sum(s["ui_wake_lag_ms"] > 20 for s in frames),
    }
    # Reloaded UI generations may overlap. Report each sampler independently,
    # not a combined FPS that silently counts the same presented frame twice.
    groups = {}
    for sample in frames:
        key = f"{sample.get('pid', 'unknown')}/{sample.get('window', 'unknown')}/{sample.get('sampler_id', 'legacy')}"
        groups.setdefault(key, []).append(sample)
    summary['cadence_by_sampler'] = {key: cadence_summary(samples) for key, samples in groups.items()}
    for key, cadence in summary['cadence_by_sampler'].items():
        print(f"Cadence {key}: {json.dumps(cadence)}")
        if cadence['animation_intervals'] < 30:
            print("INCONCLUSIVE FOR SUSTAINED FPS: fewer than 30 animation intervals. Idle draws are not missed frames.")
    if len(groups) > 1:
        print("Multiple windows or reload generations sampled. Counts above are raw sampler totals, not deduplicated frames.")
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int)
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--scenario", choices=["idle", "typing", "scrolling", "panel-switch", "mixed"], default="mixed")
    parser.add_argument("--perf", action="store_true", help="also sample live userspace stacks at 99 Hz")
    parser.add_argument("--analyze", type=Path)
    args = parser.parse_args()
    if args.analyze:
        analyze(args.analyze)
        return
    if not 5 <= args.seconds <= 110:
        parser.error("--seconds must be 5..110 (capture expires automatically)")
    if args.perf and shutil.which('perf') is None:
        print("WARNING: perf is unavailable. Capturing frame timing and process load without CPU stacks.")
        args.perf = False
    runtime = Path(os.environ["XDG_RUNTIME_DIR"])
    pid = args.pid or desktop_pid()
    control = runtime / "jcode-desktop-profile.json"
    capture_id = uuid.uuid4().hex
    output = (args.output or Path("target/live-profile") / capture_id).resolve()
    output.mkdir(parents=True, exist_ok=False)
    start = time.time()
    request = {"capture_id": capture_id, "until_unix_ms": int((start + args.seconds + 2) * 1000)}
    # Never replace another live capture. Expired files are safe to remove.
    if control.exists():
        prior = json.loads(control.read_text())
        if prior.get("until_unix_ms", 0) > start * 1000:
            raise SystemExit("Another live capture is active. Wait for its expiry.")
        control.unlink()
    fd = os.open(control, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(fd, "w") as file:
        json.dump(request, file)
    (output / "capture.json").write_text(json.dumps({
        **request, "pid": pid, "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
        "executable": os.readlink(f"/proc/{pid}/exe"), "perf": args.perf,
        "unix_ns": time.time_ns(), "monotonic_ns": time.monotonic_ns(),
        "scenario": args.scenario,
    }, indent=2) + "\n")
    perf = None
    perf_log = None
    stopped_early = None
    source = runtime / f"jcode-desktop-profile-{pid}-{capture_id}.jsonl"
    print(f"Capturing live desktop PID {pid} for {args.seconds}s. Use the app normally.\nOutput: {output}", flush=True)
    try:
        if args.perf:
            perf_log = (output / "perf.log").open("w")
            perf = subprocess.Popen([
                "perf", "record", "-e", "cpu-clock", "-F", "99", "--no-inherit",
                "--clockid", "mono", "--timestamp",
                "--call-graph", "fp", "-p", str(pid), "-o", str(output / "perf.data"),
                "--", "sleep", str(args.seconds),
            ], stdout=perf_log, stderr=perf_log)
        with (output / "process.jsonl").open("w") as file:
            while time.time() < start + args.seconds:
                try:
                    sample = process_sample(pid)
                except (FileNotFoundError, ProcessLookupError):
                    stopped_early = f"Desktop PID {pid} exited during capture"
                    print(f"WARNING: {stopped_early}. Preserving partial timing evidence.")
                    metadata = json.loads((output / 'capture.json').read_text())
                    metadata['stopped_early_reason'] = stopped_early
                    (output / 'capture.json').write_text(json.dumps(metadata, indent=2) + '\n')
                    break
                file.write(json.dumps(sample) + "\n")
                file.flush()
                if time.time() > start + 5 and not source.exists():
                    raise RuntimeError("No live-window samples. Rebuild/reload UI with live profiling support, and check PID/runtime directory.")
                time.sleep(.25)
        if source.exists():
            data = source.read_bytes()
            # A live writer may be appending the final line as we take the
            # snapshot. Never interpret a truncated record as valid evidence.
            (output / "frames.jsonl").write_bytes(data[:data.rfind(b"\n") + 1])
        else:
            raise RuntimeError("No frame samples received")
    finally:
        try:
            if json.loads(control.read_text()).get("capture_id") == capture_id:
                control.unlink()
        except FileNotFoundError:
            pass
        if perf is not None:
            try:
                perf.wait(timeout=30)
            except subprocess.TimeoutExpired:
                perf.terminate()
                perf.wait(timeout=10)
            if perf.returncode:
                print(f"Perf failed with {perf.returncode}. See {output / 'perf.log'}.")
        if perf_log is not None:
            perf_log.close()
    analyze(output)
    if stopped_early:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
