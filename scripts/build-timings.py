#!/usr/bin/env python3
"""Measure Jcode Desktop compile times and append them to a JSONL log.

Each scenario touches one file (or nothing), runs the same cargo command the
host's Ctrl+R path uses, and records wall time plus the machine/git context so
runs can be compared over time.

Usage:
  python3 scripts/build-timings.py                  # default scenarios, dev
  python3 scripts/build-timings.py --release        # what a release host's Ctrl+R runs
  python3 scripts/build-timings.py --runs 3 --scenario ui-leaf
  python3 scripts/build-timings.py --summary        # print the log as a table

Log: target/build-timings.jsonl (override with --log).
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_LOG = ROOT / "target" / "build-timings.jsonl"

# name -> file to touch (None = no-op build)
SCENARIOS: dict[str, str | None] = {
    "noop": None,
    "ui-leaf": "crates/jcode-desktop-ui/src/sidebar_selection.rs",
    "ui-workspace": "crates/jcode-desktop-ui/src/workspace.rs",
    "api": "crates/jcode-desktop-api/src/lib.rs",
    "host": "src/main.rs",
}
DEFAULT_SCENARIOS = ["noop", "ui-leaf", "ui-workspace", "host"]


def git(*args: str) -> str:
    try:
        return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    except subprocess.CalledProcessError:
        return ""


def cargo_cmd(release: bool) -> list[str]:
    # Mirrors rebuild_ui() in src/main.rs.
    cmd = [os.environ.get("CARGO", "cargo"), "build", "-p", "jcode-desktop", "-p", "jcode-desktop-ui"]
    if release:
        cmd.append("--release")
    return cmd


def build(release: bool) -> tuple[float, bool, str]:
    start = time.monotonic()
    proc = subprocess.run(cargo_cmd(release), cwd=ROOT, capture_output=True, text=True)
    elapsed = time.monotonic() - start
    return elapsed, proc.returncode == 0, proc.stderr[-2000:]


def touch(rel: str) -> None:
    path = ROOT / rel
    now = time.time()
    os.utime(path, (now, now))


def context() -> dict:
    load = os.getloadavg() if hasattr(os, "getloadavg") else (0, 0, 0)
    rustc = subprocess.run(["rustc", "-V"], capture_output=True, text=True).stdout.strip()
    return {
        "commit": git("rev-parse", "--short", "HEAD"),
        "branch": git("rev-parse", "--abbrev-ref", "HEAD"),
        "dirty": bool(git("status", "--porcelain", "--untracked-files=no")),
        "host": platform.node(),
        "cpus": os.cpu_count(),
        "load1": round(load[0], 2),
        "rustc": rustc,
        "linker": os.environ.get("JCODE_FAST_LINKER", "default"),
    }


def run(args: argparse.Namespace) -> int:
    log = Path(args.log)
    log.parent.mkdir(parents=True, exist_ok=True)
    profile = "release" if args.release else "dev"
    scenarios = args.scenario or DEFAULT_SCENARIOS

    print(f"warming {profile} build...", file=sys.stderr)
    warm, ok, err = build(args.release)
    if not ok:
        print(f"warm build failed:\n{err}", file=sys.stderr)
        return 1

    ctx = context()
    failed = False
    for name in scenarios:
        target = SCENARIOS[name]
        samples = []
        for _ in range(args.runs):
            if target:
                touch(target)
            elapsed, ok, err = build(args.release)
            if not ok:
                print(f"{name}: build failed\n{err}", file=sys.stderr)
                failed = True
                break
            samples.append(elapsed)
        if not samples:
            continue
        record = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "source": "build-timings.py",
            "profile": profile,
            "scenario": name,
            "touched": target,
            "samples_s": [round(s, 2) for s in samples],
            "median_s": round(statistics.median(samples), 2),
            **ctx,
        }
        with log.open("a") as fh:
            fh.write(json.dumps(record) + "\n")
        print(f"{profile:<7} {name:<13} median {record['median_s']:>6.2f}s  samples {record['samples_s']}")
    return 1 if failed else 0


def summary(args: argparse.Namespace) -> int:
    log = Path(args.log)
    if not log.exists():
        print(f"no log at {log}")
        return 0
    rows = [json.loads(line) for line in log.read_text().splitlines() if line.strip()]
    print(f"{'when':<20} {'source':<16} {'profile':<8} {'scenario':<13} {'median':>8}  commit")
    for r in rows[-args.limit:]:
        med = r.get("median_s", r.get("build_s"))
        when = time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(r["unix"])) if "unix" in r else r["ts"][:19]
        print(f"{when:<20} {r.get('source',''):<16} {r['profile']:<8} {r.get('scenario',''):<13} {med:>7.2f}s  {r.get('commit','')}")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--release", action="store_true", help="measure the release profile (release host Ctrl+R)")
    p.add_argument("--runs", type=int, default=2)
    p.add_argument("--scenario", action="append", choices=sorted(SCENARIOS))
    p.add_argument("--log", default=str(DEFAULT_LOG))
    p.add_argument("--summary", action="store_true", help="print recorded timings")
    p.add_argument("--limit", type=int, default=40)
    args = p.parse_args()
    return summary(args) if args.summary else run(args)


if __name__ == "__main__":
    sys.exit(main())
