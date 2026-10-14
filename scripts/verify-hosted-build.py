#!/usr/bin/env python3
"""Reject local fallback, stale reports, or mismatched release build evidence."""
import argparse
import json
from pathlib import Path


def verify(report, binary, target, started_after, rust_version):
    if report.get("exit") != 0 or report.get("remote", {}).get("machine") != "hosted":
        raise ValueError("release compilation must succeed remotely on hosted, not fall back locally")
    if report.get("start", 0) < started_after:
        raise ValueError("hosted build report predates this compilation")
    if report.get("toolchain") != rust_version:
        raise ValueError("hosted compiler identity does not match the exact requested Rust version")
    args = report.get("args", [])
    if not args or args[0] != "build" or "--release" not in args or "--locked" not in args:
        raise ValueError("hosted report is not a locked release build")
    for flag, value in (("--bin", binary), ("--target", target)):
        if flag not in args or args.index(flag) + 1 >= len(args) or args[args.index(flag) + 1] != value:
            raise ValueError(f"hosted report does not match {flag} {value}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--started-after", type=float, required=True)
    parser.add_argument("--rust-version", required=True)
    args = parser.parse_args()
    try:
        verify(json.loads(args.report.read_text()), args.binary, args.target, args.started_after, args.rust_version)
    except (ValueError, OSError, TypeError, AttributeError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
