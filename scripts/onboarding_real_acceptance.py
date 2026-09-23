#!/usr/bin/env python3
"""Exercise actual first-run Desktop on private Xvfb, never fixture mode.

Only Welcome's Skip and the beta dismissal are activated. No sign-in request,
email, browser or model submission is initiated by this acceptance workflow.
Public update/catalog startup traffic is not disabled or claimed absent.
"""
import argparse
from contextlib import contextmanager
import importlib.util
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import time
import tomllib

from default_directory_acceptance import NativeUI
from model_picker_acceptance import normalized, phrase_bounds

REPO = Path(__file__).resolve().parents[1]


@contextmanager
def private_display_root():
    root = Path(tempfile.mkdtemp(prefix="jco-", dir="/run/user/" + str(os.getuid())))
    try:
        yield root
    finally:
        # A timed-out launcher can intentionally retain live writers. Preserve
        # their whole private parent rather than deleting it on harness failure.
        if list((root / "runtime").glob("jcode-onboarding-*")):
            print("Retained private sandbox after incomplete cleanup: " + str(root), flush=True)
        else:
            shutil.rmtree(root)


class ProductionUI(NativeUI):
    def __init__(self, output, env, root):
        assert not any(key.startswith("JCODE_DESKTOP_SCREENSHOT") for key in env)
        assert env["HOME"] == str(root / "home")
        assert env["XDG_RUNTIME_DIR"] == str(root / "runtime")
        assert env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY")
        self.output, self.env, self.root = output, env, root

    def capture(self, label):
        from PIL import Image
        time.sleep(.35)
        path = self.artifact(label + ".png")
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=self.env, cwd=self.root, check=True, timeout=15)
        state = self.root / "desktop-state.txt"
        if state.is_file():
            shutil.copyfile(state, self.artifact(label + "-state.txt"))
        with Image.open(path) as image:
            return image.convert("RGB")

    def text(self, label, phrase):
        def check(image):
            words = self.words(image, (0, 0, image.width, image.height), label, psm=11)
            phrase_bounds(words, phrase)
            return words
        return self.wait_frame(label, check)


def wait_for(check, message, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(.1)
    raise AssertionError(message)


def process_environment(pid):
    return dict(item.decode().split("=", 1) for item in
                Path(f"/proc/{pid}/environ").read_bytes().split(b"\0") if item)


def peer(path):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(2)
        connection.connect(str(path))
        return struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))


def run(output, binary=None, no_build=False, size="1440x1000", jcode_binary=None):
    output = Path(output).resolve()
    if output.exists():
        raise RuntimeError(f"refusing to overwrite {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    for tool in ("Xvfb", "openbox", "xdotool", "import", "tesseract"):
        if not shutil.which(tool):
            raise RuntimeError(f"missing native acceptance dependency: {tool}")
    width, height = map(int, size.split("x"))
    if not 640 <= width <= 7680 or not 480 <= height <= 4320:
        raise ValueError("invalid private display dimensions")
    binary = Path(binary or REPO / "target/debug/jcode-desktop").resolve()
    if not no_build:
        command = ["cargo", "build", "-p", "jcode-desktop"]
        if binary.parent.name == "release":
            command.append("--release")
        subprocess.run(command, cwd=REPO, check=True)
    spec = importlib.util.spec_from_file_location("onboarding_launcher", REPO / "scripts/onboarding-desktop.py")
    launcher = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(launcher)
    report = {"mode": "real-production-first-run", "binary": str(binary), "checks": {},
              "auth_network": "No sign-in, browser, email or submission control activated",
              "public_startup_network": "Not blocked or measured"}
    result = None
    processes = []
    original_environment = dict(os.environ)
    # Short runtime root avoids AF_UNIX's pathname limit, independent of checkout path.
    with private_display_root() as root:
        root.chmod(0o700)
        for name in ("home", "runtime", "config", "cache", "data", "state"):
            (root / name).mkdir(mode=0o700)
        env = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"),
               "XDG_RUNTIME_DIR": str(root / "runtime"), "XDG_CONFIG_HOME": str(root / "config"),
               "XDG_CACHE_HOME": str(root / "cache"), "XDG_DATA_HOME": str(root / "data"),
               "XDG_STATE_HOME": str(root / "state"), "LANG": "C.UTF-8",
               "LIBGL_ALWAYS_SOFTWARE": "1", "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(root / "no-bus")}
        config = root / "openbox.xml"
        config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        sentinel = root / "personal-sentinel"
        sentinel.write_text("DO NOT READ OR CHANGE: personal sentinel\n")
        sentinel_before = sentinel.read_bytes()
        read_fd, write_fd = os.pipe()
        try:
            with (root / "display.log").open("w") as log:
                xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                                         f"{width}x{height}x24", "-nolisten", "tcp"],
                                        pass_fds=(write_fd,), env=env, stdout=log, stderr=log)
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise RuntimeError("private Xvfb startup timed out")
                display = os.read(read_fd, 64).decode().strip()
                assert display.isdigit(), "Xvfb failed to allocate display"
                env["DISPLAY"] = ":" + display
                processes.append(subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(config)],
                                                  env=env, stdout=log, stderr=log))
                time.sleep(.5)
                assert processes[-1].poll() is None, "private Openbox failed"
                # Supply poisoned parent values, but retain installed companion lookup.
                source = dict(env, HOME=original_environment.get("HOME", ""),
                              PATH=original_environment.get("PATH", "/usr/bin:/bin"),
                              JCODE_API_KEY="onboarding-sentinel-not-a-real-key",
                              ANTHROPIC_API_KEY="onboarding-sentinel-not-a-real-key",
                              CLAUDE_CODE_OAUTH_TOKEN="onboarding-sentinel-not-a-real-key",
                              JCODE_HOME=str(sentinel), JCODE_DESKTOP_CONFIG=str(sentinel),
                              JCODE_SOCKET=str(sentinel), JCODE_API_SOCKET=str(sentinel),
                              SSH_AUTH_SOCK=str(sentinel), BROWSER="personal-browser-sentinel")
                source_before = dict(source)
                result = launcher.open_onboarding(timeout=60, binary=binary, source=source,
                                                   jcode_binary=jcode_binary)
                profile = Path(result["profile"])
                child_env = process_environment(result["pid"])
                ui = ProductionUI(output, child_env, profile)
                assert child_env["JCODE_HOME"] == str(profile / "jcode")
                assert child_env["JCODE_SOCKET"] == str(profile / "runtime/jcode.sock")
                assert child_env["JCODE_API_SOCKET"] == str(profile / "runtime/jcode-api.sock")
                forbidden = ("JCODE_API_KEY", "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "SSH_AUTH_SOCK", "JCODE_DESKTOP_CONFIG")
                assert not any(key in child_env for key in forbidden), "credential/environment leak"
                assert "personal-browser-sentinel" not in child_env.values()
                report["checks"]["allowlisted_environment"] = True
                print("JCODE_CHECKPOINT " + json.dumps({"message": "Production host rendered with isolated environment"}), flush=True)
                endpoints = {}
                runtime_logs = profile / "jcode/logs"
                def runtime_diagnostic():
                    return "\n".join(path.read_text(errors="replace") for path in runtime_logs.glob("*.log"))
                wait_for(lambda: (profile / "runtime/jcode-api.sock").exists(),
                         "production runtime did not publish its private API socket", 60)
                for name in ("jcode-desktop.sock", "jcode.sock", "jcode-api.sock"):
                    path = profile / "runtime" / name
                    assert path.exists(), f"missing private runtime endpoint: {name}"
                    pid, uid, _ = peer(path)
                    assert uid == os.getuid()
                    assert process_environment(pid)["JCODE_HOME"] == str(profile / "jcode")
                    assert not any(key in process_environment(pid) for key in forbidden)
                    endpoints[name] = pid
                ui.artifact("runtime.log").write_text(runtime_diagnostic())
                assert endpoints["jcode-desktop.sock"] == result["pid"]
                report["endpoints"] = endpoints
                report["checks"]["owned_private_runtime_endpoints"] = True
                welcome = ui.text("welcome", "Welcome to Jcode")
                phrase_bounds(welcome, "Skip for now")
                text = normalized(" ".join(word["text"] for word in welcome))
                assert "simulat" not in text and "rehearse" not in text
                shutil.copyfile(ui.artifact("welcome.png"), output)
                report["checks"]["real_welcome_without_simulator"] = True
                ui.click(phrase_bounds(welcome, "Skip for now"))
                beta = ui.text("beta", "Jcode Desktop is in beta testing")
                phrase_bounds(beta, "Got it")
                ui.native("key", "--clearmodifiers", "Escape")
                ui.text("workspace", "chat")
                saved = tomllib.loads((profile / "jcode/config.toml").read_text())
                shared_config = saved.get("desktop", saved)
                assert shared_config["workspace"]["account_sign_in_handled"] is True
                ui.artifact("saved-config.toml").write_text((profile / "jcode/config.toml").read_text())
                report["checks"]["native_skip_beta_workspace_and_private_persistence"] = True
                ui.native("mousemove", 132, 30)
                def accounts_menu(image):
                    return phrase_bounds(ui.words(image, (0, 52, 264, 340), "accounts-menu", psm=11), "accounts")
                ui.click(ui.wait_frame("accounts-menu", accounts_menu))
                ui.native("mousemove", 700, 600)
                def accounts_empty(image):
                    words = ui.words(image, (0, 80, 264, image.height), "accounts-status", psm=11)
                    phrase_bounds(words, "Not connected")
                    phrase_bounds(words, "Not configured")
                    actual = normalized(" ".join(word["text"] for word in words))
                    assert "signedin" not in actual.replace("notsignedin", "")
                    return words
                ui.wait_frame("accounts-empty", accounts_empty)
                report["checks"]["real_accounts_unconfigured"] = True
                assert not (profile / "browser.log").exists(), "workflow unexpectedly launched browser"
                assert not list((profile / "jcode").rglob("*auth.json")), "unexpected auth persistence"
                assert not (profile / "jcode/config/jcode/jcode.env").exists(), "unexpected hosted account credential"
                report["checks"]["no_browser_or_persisted_credentials"] = True
                assert sentinel.read_bytes() == sentinel_before
                assert source == source_before and dict(os.environ) == original_environment
                report["checks"]["parent_sentinels_and_environment_unchanged"] = True
                # WM_DELETE_WINDOW through private Openbox, not a signal or windowkill.
                ui.native("key", "--clearmodifiers", "alt+F4")
                wait_for(lambda: not profile.exists(), "normal close did not clean private profile", 20)
                for pid in endpoints.values():
                    assert not Path(f"/proc/{pid}").exists(), f"private process {pid} survived cleanup"
                report["checks"]["normal_close_stops_runtime_and_removes_profile"] = True
                report["passed"] = True
        except BaseException as error:
            report["passed"] = False
            report["error"] = repr(error)
            if result:
                profile = Path(result["profile"])
                for relative in ("desktop.log", "state/jcode-desktop/jcode-desktop.log", "desktop-state.txt"):
                    path = profile / relative
                    if path.is_file():
                        shutil.copyfile(path, output.with_name(output.stem + "-" + path.name))
            raise
        finally:
            if result and Path(result["profile"]).exists():
                try:
                    os.kill(result["supervisor_pid"], signal.SIGTERM)
                    wait_for(lambda: not Path(result["profile"]).exists(), "failure cleanup incomplete", 15)
                except (ProcessLookupError, AssertionError):
                    pass
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
            output.with_suffix(".onboarding.json").write_text(json.dumps(report, indent=2) + "\n")
    print("Real first-run native acceptance passed: " + str(output), flush=True)
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=REPO / "target/debug/jcode-desktop")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--jcode-binary", type=Path, help="explicit production Jcode companion executable")
    parser.add_argument("--size", default="1440x1000")
    args = parser.parse_args()
    run(args.output, args.binary, args.no_build, args.size, args.jcode_binary)
