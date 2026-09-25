#!/usr/bin/env python3
"""Prepare a fresh Cargo home for the pinned GPUI FreeBSD compatibility fix.

Run on FreeBSD before the unchanged, tagged package-freebsd.sh. Export CARGO_HOME
to the same --cargo-home path for packaging. The path must not exist, including
on retries. No shared Cargo cache or Desktop source/lockfile is modified.
"""

import argparse
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import sys
import tomllib


TARGET = "x86_64-unknown-freebsd"
# The shared host/UI GPUI fork. Its gpui.rs is byte-identical to the previously
# reviewed upstream revision, so the same before/after hashes still apply.
REVISION = "ff1a26d02a7326d679583e63fdd4d509fc571291"
SOURCE = f"git+https://github.com/1jehuang/zed?rev={REVISION}#{REVISION}"
BEFORE_SHA256 = "7da10d7a7896fabd1bf0a29a989c433b946f322a3355dbd6cd1866243b1ca680"
AFTER_SHA256 = "1b4154a26a2b84b978cf07903f459398d4c763901be7b7fd570423168d01b972"
RELATIVE_SOURCE = Path("crates/gpui/src/gpui.rs")
EDITS = (
    (
        b'    target_os = "linux",\n    target_family = "wasm",\n    feature = "test-support"',
        b'    target_os = "linux",\n    target_os = "freebsd",\n    target_family = "wasm",\n    feature = "test-support"',
    ),
    (
        b'#[cfg(any(target_os = "windows", target_os = "linux", target_family = "wasm"))]\npub use queue::',
        b'#[cfg(any(\n    target_os = "windows",\n    target_os = "linux",\n    target_os = "freebsd",\n    target_family = "wasm"\n))]\npub use queue::',
    ),
)


def add_freebsd_cfg(data):
    """Only the queue module and its public re-export gain a FreeBSD cfg."""
    for old, new in EDITS:
        if data.count(old) != 1:
            raise ValueError("GPUI cfg patch context must occur exactly once")
        data = data.replace(old, new, 1)
    return data


def patch_source(path):
    if path.is_symlink() or path.stat().st_nlink != 1:
        raise ValueError("refusing linked GPUI source")
    data = path.read_bytes()
    before = hashlib.sha256(data).hexdigest()
    if before != BEFORE_SHA256:
        raise ValueError(f"GPUI source drift: expected {BEFORE_SHA256}, got {before}")
    patched = add_freebsd_cfg(data)
    after = hashlib.sha256(patched).hexdigest()
    if after != AFTER_SHA256:
        raise ValueError(f"GPUI patch drift: expected {AFTER_SHA256}, got {after}")
    path.write_bytes(patched)
    if hashlib.sha256(path.read_bytes()).hexdigest() != AFTER_SHA256:
        raise ValueError("GPUI patched source verification failed")
    print(f"GPUI FreeBSD compatibility: revision={REVISION} before={before} after={after}", flush=True)


def find_source(cargo_home):
    candidates = list((cargo_home / "git/checkouts").glob(f"zed-*/*/{RELATIVE_SOURCE}"))
    matches = []
    for source in candidates:
        if not source.resolve(strict=True).is_relative_to(cargo_home):
            raise ValueError("GPUI source escaped isolated Cargo home")
        if source.is_symlink() or source.stat().st_nlink != 1:
            raise ValueError("refusing linked GPUI source")
        checkout = source.parents[3]
        revision = subprocess.check_output(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True,
        ).strip()
        # Cargo may also fetch the Linux platform fork. Directory names are
        # not identity evidence, and that checkout must remain untouched.
        if revision == REVISION:
            matches.append(source)
    if len(matches) != 1:
        raise ValueError(f"expected one isolated Zed GPUI checkout revision {REVISION}, found {len(matches)}")
    return matches[0]


def prepare(desktop_root, cargo_home, target, verify_only=False):
    if platform.system() != "FreeBSD" or target != TARGET:
        raise ValueError(f"this compatibility patch requires native FreeBSD and {TARGET}")
    desktop_root = desktop_root.resolve(strict=True)
    manifest = desktop_root / "Cargo.toml"
    lock = desktop_root / "Cargo.lock"
    lock_bytes = lock.read_bytes()
    packages = tomllib.loads(lock_bytes.decode())["package"]
    gpui = [p for p in packages if p["name"] == "gpui"]
    if len(gpui) != 1 or gpui[0].get("source") != SOURCE:
        raise ValueError("Cargo.lock does not contain the exact reviewed GPUI revision")

    cargo_home = cargo_home.absolute()
    if verify_only:
        if cargo_home.is_symlink():
            raise ValueError("refusing linked Cargo home")
        source = find_source(cargo_home.resolve(strict=True))
        changed = subprocess.check_output(
            ["git", "-C", str(source.parents[3]), "diff", "--name-only", "-z", "HEAD", "--"],
        ).split(b"\0")
        if changed != [RELATIVE_SOURCE.as_posix().encode(), b""]:
            raise ValueError("Zed tracked diff must contain only crates/gpui/src/gpui.rs")
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        if digest != AFTER_SHA256:
            raise ValueError(f"GPUI patched source drift: expected {AFTER_SHA256}, got {digest}")
        print(f"Verified GPUI FreeBSD compatibility: revision={REVISION} sha256={digest}", flush=True)
        return

    # mkdir(exist_ok=False) rejects shared homes, symlinks, and previous attempts.
    # Do not copy caches: hardlinks/symlinks could modify another build's source.
    cargo_home.mkdir(mode=0o700)
    cargo_home = cargo_home.resolve(strict=True)
    env = dict(os.environ, CARGO_HOME=str(cargo_home))
    print(f"Preparing isolated CARGO_HOME={cargo_home}", flush=True)
    subprocess.run(
        ["cargo", "fetch", "--locked", "--manifest-path", str(manifest), "--target", target],
        env=env, cwd=desktop_root, check=True,
    )
    if lock.read_bytes() != lock_bytes:
        raise ValueError("Cargo.lock changed during locked fetch")
    patch_source(find_source(cargo_home))
    print(f"Ready. Package with CARGO_HOME={cargo_home}. Native release gates remain required.", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--desktop-root", required=True, type=Path)
    parser.add_argument("--cargo-home", required=True, type=Path)
    parser.add_argument("--target", default=TARGET, choices=[TARGET])
    parser.add_argument("--verify-only", action="store_true", help="verify patched source after build, without fetching or writing")
    args = parser.parse_args()
    try:
        prepare(args.desktop_root, args.cargo_home, args.target, args.verify_only)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
