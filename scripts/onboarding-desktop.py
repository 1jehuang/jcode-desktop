#!/usr/bin/env python3
"""Restore/start the main Desktop and reset its sandbox onboarding to Welcome.

Example niri config (replace the script path with your checkout's absolute path):
    binds {
        Alt+Shift+5 { spawn "python3" "/home/jeremy/jcode-desktop/scripts/onboarding-desktop.py"; }
    }

Requires Linux, XDG_RUNTIME_DIR, ~/.local/bin/jcode-desktop, and a main Desktop
with the self-development preview endpoint enabled (normally --hot-reload).
Never scans for previews or targets auxiliary/no-sidebar/panel windows.
Each invocation sends the onboarding command, even if the sandbox is already open.
"""
import argparse
import json
import math
import os
from pathlib import Path
import socket
import stat
import struct
import subprocess
import sys
import time


class UnsafeEndpoint(RuntimeError):
    """A path or connected peer is not safe to control."""


def validate_path(path, directory=False):
    meta = path.lstat()
    correct_type = stat.S_ISDIR(meta.st_mode) if directory else stat.S_ISSOCK(meta.st_mode)
    if not correct_type or meta.st_uid != os.getuid() or meta.st_mode & 0o077:
        kind = "directory (mode 0700)" if directory else "socket (mode 0600)"
        raise UnsafeEndpoint(f"unsafe {kind}, must be owned by this user: {path}")


def remaining(deadline):
    seconds = deadline - time.monotonic()
    if seconds <= 0:
        raise TimeoutError("startup deadline expired")
    return min(seconds, 3.0)


def connect(path, deadline, expected_pid=None):
    validate_path(path)
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.settimeout(remaining(deadline))
        client.connect(str(path))
        pid, uid, _ = struct.unpack(
            "3i", client.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i"))
        )
        if uid != os.getuid() or pid <= 0 or (expected_pid is not None and pid != expected_pid):
            raise UnsafeEndpoint(f"unexpected peer for {path}: pid={pid}, uid={uid}")
        return client, pid
    except BaseException:
        client.close()
        raise


def main_host_pid(runtime, deadline):
    client, pid = connect(runtime / "jcode-desktop.sock", deadline)
    with client:
        client.settimeout(remaining(deadline))
        client.sendall(b"S")
        response = bytearray()
        while len(response) < 3:
            client.settimeout(remaining(deadline))
            chunk = client.recv(3 - len(response))
            if not chunk:
                raise RuntimeError("main Desktop closed before acknowledging Show")
            response.extend(chunk)
        if response != b"ok\n":
            raise RuntimeError(f"main Desktop returned invalid Show acknowledgement: {bytes(response)!r}")
    return pid


def request_onboarding(runtime, pid, deadline):
    directory = runtime / "jcode-desktop-preview"
    validate_path(directory, directory=True)
    # Same lstat/owner/mode and newline/size validation as preview-state.py's
    # request helper, kept local to enforce one overall deadline and peer PID.
    client, _ = connect(directory / f"{pid}.sock", deadline, expected_pid=pid)
    with client:
        client.settimeout(remaining(deadline))
        client.sendall(b'{"command":"onboarding"}\n')
        response = bytearray()
        while b"\n" not in response:
            client.settimeout(remaining(deadline))
            chunk = client.recv(4096)
            if not chunk:
                raise RuntimeError("preview endpoint closed before acknowledging onboarding")
            response.extend(chunk)
            if len(response) > 65536:
                raise RuntimeError("preview response too large")
    result = json.loads(response.split(b"\n", 1)[0])
    if not isinstance(result, dict) or result.get("ok") is not True or result.get("step") != "welcome":
        raise RuntimeError(f"onboarding did not confirm Welcome: {result!r}")
    return result


def open_onboarding(timeout=15.0):
    if not math.isfinite(timeout) or not 0 < timeout <= 120:
        raise ValueError("timeout must be greater than 0 and at most 120 seconds")
    if not hasattr(socket, "SO_PEERCRED"):
        raise RuntimeError("Linux SO_PEERCRED is required to identify the main Desktop host")
    value = os.environ.get("XDG_RUNTIME_DIR")
    if not value or not Path(value).is_absolute():
        raise RuntimeError("XDG_RUNTIME_DIR must be set to an absolute private runtime directory")
    runtime = Path(value)
    validate_path(runtime, directory=True)
    launcher = Path.home() / ".local/bin/jcode-desktop"
    try:
        subprocess.Popen(
            [str(launcher)], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True,
        )
    except OSError as error:
        raise RuntimeError(f"could not launch {launcher}: {error}") from error
    deadline = time.monotonic() + timeout
    last_error = "main Desktop is not ready"
    while time.monotonic() < deadline:
        try:
            pid = main_host_pid(runtime, deadline)
            return request_onboarding(runtime, pid, deadline)
        except UnsafeEndpoint:
            raise
        except (OSError, ValueError, RuntimeError) as error:
            last_error = str(error)
        delay = min(0.1, max(0.0, deadline - time.monotonic()))
        if delay:
            time.sleep(delay)
    raise RuntimeError(
        f"timed out after {timeout:g}s waiting for main Desktop onboarding: {last_error}. "
        "Ensure ~/.local/bin/jcode-desktop starts the main host with --hot-reload "
        "and a UI supporting the onboarding preview command. No auxiliary preview was selected."
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--timeout", type=float, default=15.0,
                        help="maximum startup/request wait in seconds, >0 and <=120 (default: 15)")
    args = parser.parse_args(argv)
    try:
        result = open_onboarding(args.timeout)
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({"ok": False, "error": str(error)}), file=sys.stderr)
        return 1
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    sys.exit(main())
