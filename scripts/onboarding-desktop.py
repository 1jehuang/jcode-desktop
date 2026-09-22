#!/usr/bin/env python3
"""Open REAL first-run Desktop in a disposable, credential-free profile.

Alt+Shift+5: python3 /path/to/jcode-desktop/scripts/onboarding-desktop.py
Each invocation opens a new production window, not a mock or screenshot fixture.
Close that window to stop its private runtime/browser and discard its profile.
Sign-in is real. Any credentials you explicitly enter belong only to this profile.
"""
import argparse
import ctypes
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlsplit

REPO = Path(__file__).resolve().parents[1]
SCRIPT = Path(__file__).resolve()
# Deliberately not os.environ.copy(): tokens, runtime overrides, proxy credentials,
# provider homes, SSH agents, DBus/keyrings and fixture switches must not leak in.
DISPLAY_ENV = (
    "DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "LANG", "LC_ALL", "LC_CTYPE",
    "XDG_SESSION_TYPE", "XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID",
    "LIBGL_ALWAYS_SOFTWARE",
)


def private_directory(path):
    meta = path.lstat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_mode & 0o077:
        raise RuntimeError(f"expected an owned private directory (0700): {path}")


def desktop_binary(explicit=None):
    candidates = [Path(explicit)] if explicit else [
        REPO / "target/release/jcode-desktop", REPO / "target/debug/jcode-desktop",
    ]
    candidates = [p.resolve() for p in candidates if p.is_file() and os.access(p, os.X_OK)]
    if not candidates:
        raise RuntimeError("Build Jcode Desktop first (cargo build -p jcode-desktop).")
    # Refine the real flow using the latest build, not a stale preferred release.
    return max(candidates, key=lambda p: p.stat().st_mtime_ns)


def runtime_binary(source, explicit=None):
    if explicit:
        path = Path(explicit).resolve()
        if not path.is_file() or not os.access(path, os.X_OK):
            raise RuntimeError(f"Jcode companion is not executable: {path}")
        return path
    candidates = [Path(source.get("HOME", "")) / ".local/bin/jcode"]
    found = shutil.which("jcode", path=source.get("PATH", "/usr/bin:/bin"))
    if found:
        candidates.append(Path(found))
    # This developer checkout also owns the adjacent runtime. Prefer the latest
    # built production companion so testing a runtime fix needs no global install
    # or restart of the user's current CLI/daemon.
    candidates.extend(REPO.parent / "jcode" / "target" / profile / "jcode"
                      for profile in ("debug", "release", "selfdev"))
    candidates = [p.resolve() for p in candidates if p.is_file() and os.access(p, os.X_OK)]
    if candidates:
        return max(candidates, key=lambda p: p.stat().st_mtime_ns)
    raise RuntimeError("Install the Jcode companion executable before opening real onboarding.")


def isolated_environment(root, source):
    env = {key: source[key] for key in DISPLAY_ENV if source.get(key)}
    # Wayland resolves relative socket names under the ORIGINAL runtime. Only
    # expose that display endpoint, never the user's other runtime sockets.
    wayland = env.get("WAYLAND_DISPLAY")
    if wayland and not Path(wayland).is_absolute():
        original_runtime = source.get("XDG_RUNTIME_DIR")
        if not original_runtime or not Path(original_runtime).is_absolute():
            raise RuntimeError("relative WAYLAND_DISPLAY requires an absolute XDG_RUNTIME_DIR")
        env["WAYLAND_DISPLAY"] = str(Path(original_runtime) / wayland)
    env.update({
        "HOME": str(root / "home"),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "data"),
        "XDG_STATE_HOME": str(root / "state"),
        "XDG_RUNTIME_DIR": str(root / "runtime"),
        "JCODE_HOME": str(root / "jcode"),
        "JCODE_RUNTIME_DIR": str(root / "runtime"),
        "JCODE_SOCKET": str(root / "runtime/jcode.sock"),
        "JCODE_API_SOCKET": str(root / "runtime/jcode-api.sock"),
        "JCODE_DESKTOP_STATE": str(root / "desktop-state.txt"),
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(root / "no-session-bus"),
        "PATH": str(root / "bin") + ":/usr/bin:/bin",
        "SHELL": "/bin/sh",
        "USER": "onboarding",
        "LOGNAME": "onboarding",
        "LANG": env.get("LANG", "C.UTF-8"),
        "JCODE_NO_TELEMETRY": "1",
        "JCODE_WAKE_MODE": "external",
        "BROWSER": str(root / "bin/xdg-open"),
    })
    return env


def prepare_profile(root, source, companion):
    private_directory(root)
    (root / ".onboarding-profile").write_text("jcode-real-onboarding-v1\n")
    for name in ("home", "config", "cache", "data", "state", "runtime", "jcode", "bin", "browser"):
        (root / name).mkdir(mode=0o700)
    # Share executables only, never ~/.jcode, account files, sessions or settings.
    (root / "bin/jcode").symlink_to(companion)
    env = isolated_environment(root, source)
    # GPUI's production open_url uses xdg-open first. Never route authentication
    # to the user's normal browser, its cookies, or the session portal.
    opener = root / "bin/xdg-open"
    opener.write_text(
        "#!/usr/bin/python3\nimport os, sys\n"
        f"os.execv('/usr/bin/python3', ['/usr/bin/python3', {str(SCRIPT)!r}, "
        f"'--open-url', {str(root)!r}] + sys.argv[1:])\n"
    )
    opener.chmod(0o700)
    return env


def open_private_url(root, url):
    private_directory(root)
    if urlsplit(url).scheme not in ("https", "http"):
        raise RuntimeError("The onboarding browser only opens HTTP(S) links.")
    # --no-remote prevents Firefox from forwarding to an already-running profile.
    # A fresh profile also keeps real SSO sessions/cookies out of this rehearsal.
    firefox = shutil.which("firefox", path="/usr/bin:/bin")
    if not firefox:
        raise RuntimeError("Firefox is required for isolated onboarding sign-in. Install it and retry.")
    # Reopening must not collide with Firefox's locked --no-remote profile or
    # forward to the user's browser. Every requested window is independently fresh.
    profile = tempfile.mkdtemp(prefix="window-", dir=root / "browser")
    with (root / "browser.log").open("ab") as log:
        subprocess.Popen(
            [firefox, "--no-remote", "--profile", profile, "--new-window", url],
            stdin=subprocess.DEVNULL, stdout=log, stderr=log,
        )


def enable_subreaper():
    # Adopt detached daemon/browser grandchildren when their parents exit. This
    # lets cleanup stop only OUR children, not scan or signal the user's daemons.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "could not supervise private runtime children")


def direct_children():
    path = Path(f"/proc/self/task/{os.getpid()}/children")
    return [int(pid) for pid in path.read_text().split()]


def stop_children():
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline:
        while True:
            try:
                if os.waitpid(-1, os.WNOHANG)[0] == 0:
                    break
            except ChildProcessError:
                return True
        children = direct_children()
        for pid in children:
            # Pin the child identity against PID reuse and recheck parenthood.
            try:
                fd = os.pidfd_open(pid)
                try:
                    if pid in direct_children():
                        sig = signal.SIGTERM if time.monotonic() < deadline - 4 else signal.SIGKILL
                        signal.pidfd_send_signal(fd, sig)
                finally:
                    os.close(fd)
            except ProcessLookupError:
                pass
        time.sleep(0.05)
    return False


def host_ready(root, pid):
    path = root / "runtime/jcode-desktop.sock"
    try:
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(0.2)
            client.connect(str(path))
            import struct
            peer_pid, peer_uid, _ = struct.unpack("3i", client.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            if peer_pid != pid or peer_uid != os.getuid():
                raise RuntimeError("private Desktop socket belongs to an unexpected process")
        # This file is emitted by production render, not just socket creation.
        return (root / "desktop-state.txt").is_file()
    except (FileNotFoundError, ConnectionRefusedError, socket.timeout):
        return False


def supervise(root, binary, timeout):
    private_directory(root)
    # Internal CLI mode is deliberately fail-closed. Even an accidental manual
    # invocation must never turn an arbitrary private directory into disposable data.
    if (not root.name.startswith("jcode-onboarding-")
            or os.environ.get("HOME") != str(root / "home")
            or os.environ.get("JCODE_HOME") != str(root / "jcode")
            or (root / ".onboarding-profile").read_text() != "jcode-real-onboarding-v1\n"):
        raise RuntimeError("refusing to supervise or remove a non-onboarding profile")
    enable_subreaper()
    stopping = False

    def stop(_signum, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        with (root / "desktop.log").open("ab") as log:
            child = subprocess.Popen(
                [str(binary), "--no-hot-reload"], cwd=root / "home",
                stdin=subprocess.DEVNULL, stdout=log, stderr=log,
            )
        deadline = time.monotonic() + timeout
        while not stopping and child.poll() is None:
            if host_ready(root, child.pid):
                print(json.dumps({"ok": True, "mode": "real-first-run", "pid": child.pid,
                                  "supervisor_pid": os.getpid(), "profile": str(root)}), flush=True)
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("production Desktop did not render before startup timeout")
            time.sleep(0.05)
        else:
            raise RuntimeError(f"production Desktop exited before rendering (exit {child.poll()})")
        while not stopping and child.poll() is None:
            time.sleep(0.2)
        return 0
    except Exception as error:
        print(json.dumps({"ok": False, "error": str(error), "profile": str(root)}), flush=True)
        return 1
    finally:
        # Remove only the mkdtemp profile owned by this invocation, and only
        # after all writers stop. Never remove any configured real user path.
        if stop_children():
            shutil.rmtree(root)


def open_onboarding(timeout=30.0, binary=None, source=None, jcode_binary=None):
    if not math.isfinite(timeout) or not 0 < timeout <= 120:
        raise ValueError("timeout must be greater than 0 and at most 120 seconds")
    if not sys.platform.startswith("linux") or not hasattr(os, "pidfd_open"):
        raise RuntimeError("isolated onboarding currently requires Linux with pidfd support")
    source = dict(os.environ if source is None else source)
    parent = Path(source.get("XDG_RUNTIME_DIR", ""))
    if not parent.is_absolute():
        raise RuntimeError("XDG_RUNTIME_DIR must be an absolute private runtime directory")
    private_directory(parent)
    binary = desktop_binary(binary)
    companion = runtime_binary(source, jcode_binary)
    sibling = binary.parent / "jcode"
    if sibling.is_file() and os.access(sibling, os.X_OK):
        # Production Desktop intentionally prefers a bundled sibling executable.
        # Do not falsely claim an explicit override wins over that resolution.
        if jcode_binary and sibling.resolve() != companion:
            raise RuntimeError("Desktop's bundled sibling jcode overrides --jcode-binary. Use a matching bundle.")
        companion = sibling.resolve()
    root = Path(tempfile.mkdtemp(prefix="jcode-onboarding-", dir=parent))
    worker = None
    try:
        env = prepare_profile(root, source, companion)
        worker = subprocess.Popen(
            [sys.executable, str(SCRIPT), "--supervise", str(root), "--binary", str(binary),
             "--timeout", str(timeout)],
            env=env, cwd=root / "home", stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True, text=True,
        )
        import select
        readable, _, _ = select.select([worker.stdout], [], [], timeout + 3)
        if not readable:
            raise RuntimeError("isolated onboarding supervisor timed out")
        result = json.loads(worker.stdout.readline())
        worker.stdout.close()
        if result.get("ok") is not True:
            raise RuntimeError(result.get("error", "isolated onboarding failed"))
        # Keep/reap the detached supervisor while a library caller remains alive.
        # CLI callers may exit immediately, at which point init adopts it.
        threading.Thread(target=worker.wait, daemon=True).start()
        return result
    except BaseException:
        if worker is not None and worker.poll() is None:
            worker.terminate()
            try:
                worker.wait(timeout=10)
            except subprocess.TimeoutExpired:
                # Leave the private directory intact if descendants might write.
                pass
        elif worker is None:
            shutil.rmtree(root)
        raise


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--binary", type=Path, help="production Desktop binary (default: newest local build)")
    parser.add_argument("--jcode-binary", type=Path, help="explicit production Jcode companion for source development")
    parser.add_argument("--supervise", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--open-url", nargs=2, metavar=("PROFILE", "URL"), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    try:
        if args.open_url:
            # Always return success to GPUI's opener chain. A private-browser
            # failure must NOT fall back to gio/portals/the real browser.
            try:
                open_private_url(Path(args.open_url[0]), args.open_url[1])
            except Exception as error:
                print(f"Private onboarding browser: {error}", file=sys.stderr)
            return 0
        if args.supervise:
            return supervise(args.supervise, args.binary, args.timeout)
        print(json.dumps(open_onboarding(args.timeout, args.binary, jcode_binary=args.jcode_binary)))
        return 0
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({"ok": False, "error": str(error)}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
