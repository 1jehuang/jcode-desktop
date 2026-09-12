#!/usr/bin/env python3
"""Control isolated previews in a self-development Desktop. No third-party modules.

Examples: preview-state.py --list; preview-state.py login-error --pid 6838
          preview-state.py --reset login-error --pid 6838
Previews are transient and deliberately omitted from reload/persisted snapshots.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import stat
import sys
import tempfile


def request(path, payload):
    meta = path.lstat()
    if not stat.S_ISSOCK(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_mode & 0o077:
        raise RuntimeError(f"unsafe preview socket: {path}")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(3)
        client.connect(str(path))
        client.sendall(json.dumps(payload).encode() + b"\n")
        response = bytearray()
        while b"\n" not in response:
            chunk = client.recv(4096)
            if not chunk:
                raise RuntimeError("preview endpoint closed before acknowledging")
            response.extend(chunk)
            if len(response) > 65536:
                raise RuntimeError("preview response too large")
    return json.loads(response.split(b"\n", 1)[0])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("state", nargs="?")
    parser.add_argument("--list", action="store_true", help="list the running UI's state catalog")
    parser.add_argument("--reset", action="store_true", help="reset the newest open preview of STATE")
    parser.add_argument("--pid", type=int, help="target Desktop process, required if multiple are running")
    args = parser.parse_args()
    if args.list and (args.state or args.reset) or not args.list and not args.state:
        parser.error("use --list, STATE, or --reset STATE")
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    directory = (Path(runtime) / "jcode-desktop-preview" if runtime
                 else Path(tempfile.gettempdir()) / f"jcode-desktop-preview-{os.geteuid()}")
    meta = directory.lstat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_mode & 0o077:
        raise RuntimeError("preview directory must be owned by this user with mode 0700")
    if args.pid is not None:
        if args.pid <= 0:
            parser.error("--pid must be positive")
        endpoint = directory / f"{args.pid}.sock"
    else:
        candidates = []
        for path in directory.glob("*.sock"):
            try:
                if request(path, {"command": "list"}).get("ok"):
                    candidates.append(path)
            except (OSError, ValueError, RuntimeError):
                pass
        if not candidates:
            raise RuntimeError("no self-development Desktop endpoint found; start with --hot-reload")
        if len(candidates) != 1:
            raise RuntimeError("multiple Desktop endpoints; specify --pid: " + ", ".join(p.stem for p in candidates))
        endpoint = candidates[0]
    payload = {"command": "list" if args.list else "reset" if args.reset else "open"}
    if args.state:
        payload["state"] = args.state
    result = request(endpoint, payload)
    print(json.dumps(result, indent=2))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({"ok": False, "error": str(error)}), file=sys.stderr)
        sys.exit(1)
