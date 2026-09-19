#!/usr/bin/env python3
"""Native startup Enter -> real SSH failure -> real local runtime acceptance.

No fixture mode, mocked bridge, provider credentials, or inherited desktop state.
Model generation is intentionally unavailable. The daemon's persisted user messages
are the acceptance boundary for Enter submission, not a generated model response.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shlex
import signal
import socket
import subprocess
import time

from PIL import Image
from screenshot import isolated_env
from model_picker_acceptance import normalized, parse_words, phrase_bounds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--desktop", type=Path, default=Path("target/release/jcode-desktop"))
    parser.add_argument("--expect-silent-startup", action="store_true",
                        help="negative control: Enter retains editor text with no queue or session")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    cli, desktop = args.cli.resolve(strict=True), args.desktop.resolve(strict=True)
    env = isolated_env(root)
    env.pop("JCODE_DESKTOP_SCREENSHOT")
    for name in ("home", "runtime", "config", "cache", "data", "logs", "jcode", "bin"):
        (root / name).mkdir(mode=0o700)
    (root / "bin/jcode").symlink_to(cli)
    env.update(PATH=str(root / "bin") + ":/usr/bin:/bin", JCODE_RUNTIME_DIR=str(root / "runtime"),
               JCODE_SOCKET=str(root / "runtime/daemon.sock"), JCODE_API_SOCKET=str(root / "runtime/api.sock"),
               JCODE_NO_TELEMETRY="1", JCODE_DEFERRED_AUTH_BOOTSTRAP="1", JCODE_WAKE_MODE="external",
               JCODE_DESKTOP_CONFIG=str(root / "desktop.toml"),
               VK_DRIVER_FILES=str(next(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))),
               HTTP_PROXY="http://127.0.0.1:9", HTTPS_PROXY="http://127.0.0.1:9", ALL_PROXY="http://127.0.0.1:9")
    # Reserve a non-listening loopback port, guaranteeing an actual refused SSH
    # connection without using an external machine or somebody else's service.
    refused = socket.socket()
    refused.bind(("127.0.0.1", 0))
    ssh_config = root / "ssh_config"
    ssh_config.write_text(f"Host enter-unavailable\n HostName 127.0.0.1\n Port {refused.getsockname()[1]}\n BatchMode yes\n ConnectTimeout 1\n IdentityAgent none\n UserKnownHostsFile /dev/null\n GlobalKnownHostsFile /dev/null\n")
    ssh = root / "bin/ssh"
    ssh.write_text(f'#!/bin/bash\nexec /usr/bin/ssh -F {shlex.quote(str(ssh_config))} "$@" 2> >(tee -a {shlex.quote(str(root / "ssh-stderr.log"))} >&2)\n')
    ssh.chmod(0o700)
    (root / "desktop.toml").write_text('[appearance]\nlayout_mode="folder_tabs"\ntheme="warm-neutral"\n[workspace]\naccount_sign_in_handled=true\ncoaching_hints=false\ndefault_remote_host="enter-unavailable"\nremote_hosts=["enter-unavailable"]\n')
    (root / "jcode/config.toml").write_text("[features]\nmemory=false\n")
    processes, logs = [], []
    report = {"real_desktop": str(desktop), "real_cli": str(cli), "fixture_mode": False,
              "desktop_sha256": hashlib.file_digest(desktop.open("rb"), "sha256").hexdigest(),
              "mode": "silent-startup-baseline" if args.expect_silent_startup else "startup-recovery",
              "model_generation": "Unavailable by design: isolated HOME without credentials and loopback-only HTTP proxies",
              "checks": {}}
    prompt = "Isolated Enter queue acceptance message"
    followup = "Isolated Enter followup acceptance message"

    def spawn(name, command, **kwargs):
        log = (root / (name + ".log")).open("w")
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=log, start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait(label, predicate, timeout=40):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert all(p.poll() is None for p in processes), "Acceptance child exited"
            result = predicate()
            if result:
                report["checks"][label] = True
                print("PASS " + label, flush=True)
                return result
            time.sleep(.1)
        raise TimeoutError(label)

    def ready(path):
        try:
            with socket.socket(socket.AF_UNIX) as client:
                client.connect(path)
            return True
        except OSError:
            return False

    def native(*args):
        return subprocess.check_output(["xdotool", *map(str, args)], env=env, cwd=root, timeout=15).decode()

    def state():
        try:
            return json.loads(next(line[11:] for line in (root / "state").read_text().splitlines() if line.startswith("navigation=")))
        except (OSError, ValueError, StopIteration):
            return {}

    def panels():
        return [p for row in state().get("rows", []) for p in row["panels"]]

    def capture(label):
        path = root / (label + ".png")
        subprocess.run(["import", "-window", "root", str(path)], env=env, cwd=root, check=True, timeout=15)
        with Image.open(path) as image:
            width, height = image.size
            image.resize((width * 3, height * 3)).save(root / (label + "-ocr.png"))
        tsv = subprocess.check_output(["tesseract", str(root / (label + "-ocr.png")), "stdout", "--psm", "11", "tsv"],
                                      env=env, stderr=subprocess.DEVNULL, timeout=20).decode()
        (root / (label + ".tsv")).write_text(tsv)
        return parse_words(tsv, (0, 0, width, height))

    def visible(label, phrase):
        words = capture(label)
        return words if normalized(phrase) in normalized(" ".join(w["text"] for w in words)) else None

    def saved(session):
        paths = list((root / "jcode").rglob(session + ".json"))
        try:
            return json.loads(paths[0].read_text()) if paths else {}
        except (OSError, ValueError):
            return {}

    def user_count(session, phrase):
        return sum(phrase in json.dumps(message) for message in saved(session).get("messages", [])
                   if message.get("role") == "user")

    read_fd, write_fd = os.pipe()
    try:
        spawn("daemon", [str(cli), "--no-update", "--no-selfdev", "--provider", "jcode", "serve"])
        wait("real daemon socket", lambda: ready(env["JCODE_SOCKET"]))
        spawn("bridge", [str(cli), "--no-update", "--no-selfdev", "api-bridge"])
        wait("real API bridge socket", lambda: ready(env["JCODE_API_SOCKET"]))
        spawn("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0]
        env["DISPLAY"] = ":" + os.read(read_fd, 64).decode().strip()
        wm = root / "openbox.xml"
        wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        spawn("wm", ["openbox", "--sm-disable", "--config-file", str(wm)])
        time.sleep(.5)
        spawn("desktop", [str(desktop), "--no-hot-reload"])
        wait("real pending startup mounted", lambda: any(p["session"] == "startup://draft" for p in panels()))
        native("search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync")
        native("key", "--clearmodifiers", "Escape")
        time.sleep(.3)
        if any(p["session"] == "desktop://changelog" for p in panels()):
            native("key", "--clearmodifiers", "ctrl+shift+w")
        initial = wait("pending editor focused", lambda: next((p for p in panels() if p["session"] == "startup://draft" and state().get("keyboard_panel") == p["slot"]), None))
        if args.expect_silent_startup:
            # Same actual refused SSH target, but the old UI has no inline
            # recovery controls. Inspect native input, not a synthetic state.
            wait("baseline actual SSH refusal", lambda: (root / "ssh-stderr.log").exists()
                 and "Connection refused" in (root / "ssh-stderr.log").read_text())
            native("type", "--clearmodifiers", "--delay", "25", prompt)
            native("key", "--clearmodifiers", "Return")
            time.sleep(2)
            words = wait("baseline Enter leaves prompt visible", lambda: visible("silent-startup", prompt))
            assert normalized("Queued") not in normalized(" ".join(w["text"] for w in words))
            assert normalized("sends when connected") not in normalized(" ".join(w["text"] for w in words))
            native("key", "--clearmodifiers", "ctrl+a", "ctrl+c")
            clipboard = subprocess.check_output(["/usr/bin/python3", "-c",
                "import gi; gi.require_version('Gtk', '3.0'); from gi.repository import Gtk, Gdk; "
                "print(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text() or '', end='')"],
                                                env=env, timeout=5).decode()
            assert clipboard == prompt, repr(clipboard)
            (root / "retained-editor.txt").write_text(clipboard)
            assert all(p["session"].startswith("startup://") for p in panels())
            assert not list((root / "jcode/sessions").glob("session_*.json"))
            report["checks"].update({"baseline editor retains exact prompt after Enter": True,
                                     "baseline no visible queue": True,
                                     "baseline no real local session": True})
            report.update(passed=True, persisted_user_messages=0, final_navigation=state())
            return
        failure = wait("real SSH failure visible inline", lambda: visible("remote-failed", "Retry connection"))
        assert "refused" in normalized(" ".join(w["text"] for w in failure)), failure
        native("type", "--clearmodifiers", "--delay", "25", prompt)
        native("key", "--clearmodifiers", "Return")
        wait("Enter visibly queues pending prompt", lambda: visible("queued", prompt))
        wait("queued send status visible", lambda: visible("queued-status", "sends when connected"))
        wait("pending editor cleared after Enter", lambda: visible("queued-editor", "something"))
        assert all(p["session"].startswith("startup://") for p in panels()), "Unexpected automatic local fallback"
        assert not list((root / "jcode/sessions").glob("session_*.json")), "Unexpected local session before recovery"
        report["checks"]["no automatic local fallback"] = True
        words = capture("choose-local")
        x1, y1, x2, y2 = phrase_bounds(words, "Use this computer")
        native("mousemove", round((x1+x2)/2), round((y1+y2)/2), "click", "1")
        attached = wait("explicit local recovery creates real session", lambda: next((p for p in panels() if p["session"].startswith("session_")), None))
        assert attached["id"] == initial["id"], "Recovery replaced the panel"
        session = attached["session"]
        report["session_id"] = session
        wait("real daemon persists queued prompt once", lambda: user_count(session, prompt) == 1)
        wait("queued prompt echoed in real conversation", lambda: visible("delivered", prompt))
        wait("same real editor focused after attach", lambda: state().get("keyboard_panel") == attached["slot"])
        native("type", "--clearmodifiers", "--delay", "25", followup)
        native("key", "--clearmodifiers", "Return")
        wait("normal Enter visibly echoes followup", lambda: visible("followup", followup))
        wait("normal Enter clears composer", lambda: visible("followup-editor", "something"))
        capture("final")
        report["final_navigation"] = state()
        # Live saves use an append journal. Closing the real panel checkpoints
        # through the daemon's own persistence code, avoiding a partial snapshot
        # read or a custom reimplementation of the journal reader.
        native("key", "--clearmodifiers", "ctrl+shift+w")
        wait("real session closed and checkpointed", lambda: saved(session).get("status") == "Closed")
        wait("normal Enter persists followup once", lambda: user_count(session, followup) == 1)
        time.sleep(2)
        assert user_count(session, prompt) == user_count(session, followup) == 1
        report["checks"]["no duplicate submissions"] = True
        report["persisted_user_messages"] = 2
        report["persisted_session"] = saved(session)
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, error=str(error))
        if env.get("DISPLAY"):
            try:
                capture("failure")
            except Exception:
                pass
        raise
    finally:
        (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        for log in logs:
            log.close()
        refused.close()
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)


if __name__ == "__main__":
    main()
