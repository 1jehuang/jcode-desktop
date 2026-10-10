#!/usr/bin/env python3
"""End-to-end: an older packaged Jcode Desktop updates itself from the live
public release channel, in every installation shape users actually have.

Each scenario builds a real install from a desktop binary stamped with an older
version, runs the real `jcode-desktop --update` (the same updater the app runs
in the background) against https://jcode.sh, then launches the user's own
entry point and requires it to report the published version.

Scenarios (Linux):
  tarball   extracted .tar.gz in a user folder, updated in place
  managed   ~/.local/opt/jcode-desktop/<version> with ~/.local/bin launcher
  system    root-owned copy (a .deb in /usr/bin), adopted into a per-user copy

Windows runs the `zip` scenario through scripts/e2e-update.ps1.

Usage: e2e-update.py BINARY [--scenario NAME ...] [--expect VERSION]
BINARY must report an older version than the live release (build with
JCODE_DESKTOP_VERSION=<older>). Only temporary directories are modified.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.request

LATEST = "https://github.com/1jehuang/jcode-desktop-releases/releases/download/desktop-latest/latest.json"
FILES = ("jcode-desktop", "jcode", "jcode-harness-api-bridge", "jcode.desktop", "jcode.png")
TELEMETRY = True


def live_version():
    with urllib.request.urlopen(LATEST, timeout=30) as response:
        return json.load(response)["tag_name"].removeprefix("desktop-v")


def version_of(executable, env):
    output = subprocess.run([str(executable), "--version"], env=env, capture_output=True,
                            text=True, timeout=60)
    assert output.returncode == 0, f"{executable} --version failed: {output.stderr}"
    return output.stdout.split()[2].removeprefix("v")


def bundle(directory, binary):
    directory.mkdir(parents=True)
    for name in FILES:
        target = directory / name
        if name == "jcode-desktop":
            shutil.copy2(binary, target)
        else:
            target.write_bytes(b"placeholder companion, replaced by the update\n")
        target.chmod(0o755 if name in FILES[:3] else 0o644)


def environment(root):
    home = root / "home"
    home.mkdir(mode=0o700, parents=True)
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(home),
        "XDG_RUNTIME_DIR": str(root / "runtime"),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_STATE_HOME": str(home / ".local/state"),
        "JCODE_HOME": str(home / ".jcode"),
        # Real telemetry from the CI-tagged harness exercises the fleet
        # pipeline end to end. Pass --no-telemetry to suppress it.
        "CI": "1",
        "LANG": "C.UTF-8",
    }
    if not TELEMETRY:
        env["JCODE_NO_TELEMETRY"] = "1"
    return home, env


def update(entry, env):
    result = subprocess.run([str(entry), "--update"], env=env, capture_output=True,
                            text=True, timeout=900)
    print(result.stdout.strip())
    if result.stderr.strip():
        print(result.stderr.strip(), file=sys.stderr)
    assert result.returncode == 0, f"--update exited {result.returncode}"
    return result.stdout


def scenario_tarball(root, binary, old, expect):
    home, env = environment(root)
    folder = home / "Apps" / f"Jcode-{old}-linux-x86_64"
    bundle(folder, binary)
    entry = folder / "jcode-desktop"
    assert version_of(entry, env) == old
    output = update(entry, env)
    assert f"Installed Jcode Desktop {expect} (portable)" in output, output
    assert version_of(entry, env) == expect
    # Every published file was replaced, not just the desktop.
    for name in FILES[1:]:
        assert (folder / name).read_bytes() != b"placeholder companion, replaced by the update\n", name
    return {"entry": str(entry), "version_after": version_of(entry, env)}


def scenario_managed(root, binary, old, expect):
    home, env = environment(root)
    install = home / ".local/opt/jcode-desktop" / old
    bundle(install, binary)
    for directory in (home / ".local", home / ".local/opt", home / ".local/opt/jcode-desktop"):
        directory.chmod(0o755)
    launcher = home / ".local/bin/jcode-desktop"
    launcher.parent.mkdir(parents=True)
    launcher.symlink_to(f"../opt/jcode-desktop/{old}/jcode-desktop")
    assert version_of(launcher, env) == old
    output = update(launcher, env)
    assert f"Installed Jcode Desktop {expect} (linux_managed)" in output, output
    assert os.readlink(launcher).endswith(f"jcode-desktop/{expect}/jcode-desktop"), os.readlink(launcher)
    assert (install / "jcode-desktop").is_file(), "previous version must be kept for running windows"
    assert version_of(launcher, env) == expect
    # A stale shortcut naming the old version directory also runs the new build.
    assert version_of(install / "jcode-desktop", env) == expect
    return {"entry": str(launcher), "launcher": os.readlink(launcher)}


def scenario_system(root, binary, old, expect):
    home, env = environment(root)
    system = root / "usr/bin"
    system.mkdir(parents=True)
    entry = system / "jcode-desktop"
    shutil.copy2(binary, entry)
    for name in FILES[1:3]:
        (system / name).write_bytes(b"system companion\n")
    # A system package is not writable by the user.
    system.chmod(0o555)
    try:
        assert version_of(entry, env) == old
        output = update(entry, env)
        assert f"Installed Jcode Desktop {expect} (linux_system)" in output, output
        assert entry.read_bytes() == binary.read_bytes(), "the system copy must not be modified"
        # Launching the system copy (its menu entry runs /usr/bin) now runs the update.
        assert version_of(entry, env) == expect
        launcher = home / ".local/bin/jcode-desktop"
        assert version_of(launcher, env) == expect
    finally:
        system.chmod(0o755)
    return {"entry": str(entry), "adopted": os.readlink(home / ".local/bin/jcode-desktop")}


SCENARIOS = {"tarball": scenario_tarball, "managed": scenario_managed, "system": scenario_system}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--scenario", action="append", choices=sorted(SCENARIOS))
    parser.add_argument("--expect", help="published version to require (default: the live channel)")
    parser.add_argument("--evidence", type=Path, help="write a JSON evidence file here")
    parser.add_argument("--no-telemetry", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    expect = args.expect or live_version()
    global TELEMETRY
    TELEMETRY = not args.no_telemetry
    with tempfile.TemporaryDirectory(prefix="jcode-e2e-update-") as scratch:
        _, probe_env = environment(Path(scratch))
        old = version_of(binary, probe_env)
    assert old != expect, f"{binary} already reports {expect}. Build it with JCODE_DESKTOP_VERSION=<older>"
    print(f"Updating Jcode Desktop {old} -> live {expect}")
    results = {}
    failed = []
    for name in args.scenario or sorted(SCENARIOS):
        root = Path(tempfile.mkdtemp(prefix=f"jcode-e2e-{name}-"))
        print(f"\n== {name}")
        try:
            results[name] = {"ok": True, **SCENARIOS[name](root, binary, old, expect)}
            print(f"PASS {name}")
        except Exception as error:  # report every scenario, then fail
            results[name] = {"ok": False, "error": str(error)}
            failed.append(name)
            print(f"FAIL {name}: {error}")
        finally:
            subprocess.run(["chmod", "-R", "u+w", str(root)], check=False)
            shutil.rmtree(root, ignore_errors=True)
    evidence = {"from": old, "to": expect, "scenarios": results}
    if args.evidence:
        args.evidence.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence, indent=2))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
