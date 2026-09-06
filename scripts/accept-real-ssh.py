#!/usr/bin/env python3
"""Real Desktop -> system SSH -> native daemon acceptance, with no model calls.

Uses two private daemons, temporary SSH keys/config, and Xvfb. No screenshot
fixture or mocked bridge is used. Requires a fresh CLI supporting api --stdio.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shlex
import signal
import socket
import subprocess
import tempfile
import time
from urllib.parse import unquote

from screenshot import isolated_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--desktop", type=Path, default=Path("target/debug/jcode-desktop"))
    args = parser.parse_args()
    cli, desktop, output = args.cli.resolve(), args.desktop.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    processes, logs = [], []
    with tempfile.TemporaryDirectory(prefix="desktop-real-ssh-", dir=os.environ.get("JCODE_SCRATCH_DIR", str(output))) as tmp:
        root = Path(tmp)
        envs = {}
        for name in ("local", "remote"):
            base = root / name
            env = isolated_env(base)
            env.pop("JCODE_DESKTOP_SCREENSHOT")
            for key in ("HOME", "XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "JCODE_HOME"):
                Path(env[key]).mkdir(parents=True, exist_ok=True)
            os.chmod(env["XDG_RUNTIME_DIR"], 0o700)
            env.update(JCODE_RUNTIME_DIR=env["XDG_RUNTIME_DIR"], JCODE_SOCKET=str(base / "runtime/jcode.sock"), JCODE_NO_TELEMETRY="1", JCODE_DEFERRED_AUTH_BOOTSTRAP="1", JCODE_WAKE_MODE="external")
            envs[name] = env
        env = envs["local"]
        bindir = root / "bin"
        bindir.mkdir()
        (bindir / "jcode").symlink_to(cli)
        env["PATH"] = str(bindir) + ":/usr/bin:/bin"
        remote_bin = Path(envs["remote"]["HOME"]) / ".local/bin"
        remote_bin.mkdir(parents=True)
        (remote_bin / "jcode").symlink_to(cli)

        def launch(label, command, environment):
            log = open(output / f"{label}.log", "w")
            logs.append(log)
            process = subprocess.Popen(command, env=environment, cwd=root, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            processes.append(process)
            return process

        def wait(label, predicate, timeout=45):
            deadline = time.monotonic() + timeout
            while True:
                value = predicate()
                if value:
                    print(f"CHECK {label}: passed", flush=True)
                    return value
                if time.monotonic() > deadline:
                    raise AssertionError(f"Timeout: {label}. See {output}")
                time.sleep(.1)

        def alive_socket(path, daemon):
            if daemon.poll() is not None:
                raise AssertionError("Private daemon exited")
            s = socket.socket(socket.AF_UNIX)
            try:
                s.connect(path)
                return True
            except OSError:
                return False
            finally:
                s.close()

        def command(*argv):
            return subprocess.check_output(argv, env=env, text=True, stderr=subprocess.DEVNULL, timeout=30)

        def capture(label):
            path = output / f"{label}.png"
            command("import", "-window", "root", str(path))
            words = []
            for line in command("tesseract", str(path), "stdout", "tsv").splitlines()[1:]:
                fields = line.split("\t", 11)
                if len(fields) == 12 and fields[11].strip():
                    words.append(dict(text=fields[11].strip(), x=int(fields[6])+int(fields[8])//2, y=int(fields[7])+int(fields[9])//2))
            return words

        def click(words, text, after=0, index=0):
            previous = None
            def candidates():
                nonlocal words, previous
                words = capture("waiting-for-control")
                if any(w["text"].startswith("Connecting") and w["x"] < 275 for w in words):
                    previous = None
                    return None
                matches = sorted((w for w in words if w["text"] == text and w["x"] < 275 and w["y"] > after and (text != "Connect" or 45 <= w["x"] <= 65)), key=lambda w: w["y"])
                if len(matches) <= index:
                    return None
                coordinate = (matches[index]["x"], matches[index]["y"])
                if coordinate == previous:
                    return matches
                previous = coordinate
                return None
            w = wait(f"rendered stable {text} control {index}", candidates)[index]
            command("xdotool", "mousemove", str(w["x"]), str(w["y"]), "click", "1")
            time.sleep(.5)

        def navigation():
            try:
                for line in Path(env["JCODE_DESKTOP_STATE"]).read_text().splitlines():
                    if line.startswith("navigation="):
                        return json.loads(line[len("navigation="):])
            except (OSError, ValueError):
                pass
            return {}

        def panels():
            return [p for row in navigation().get("rows", []) for p in row["panels"]]

        def ids():
            return [p["session"] for p in panels()]

        def seed(environment, session_id):
            # Empty native sessions are provisional. Persist context without
            # requesting inference before testing full-process restoration.
            p = subprocess.Popen([str(cli), "--no-update", "api", "--stdio"], env=environment, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            processes.append(p)
            frames = [dict(v=1, id=1, req="hello", min_version=1, max_version=1, client="desktop-real-ssh-acceptance"), dict(v=1, id=2, req="attach_session", session_id=session_id), dict(v=1, id=3, req="send_message", session_id=session_id, content="Acceptance context only, no model response requested", no_reply=True)]
            try:
                for frame in frames:
                    p.stdin.write((json.dumps(frame)+"\n").encode())
                    p.stdin.flush()
                    deadline = time.monotonic()+20
                    while True:
                        assert time.monotonic() < deadline, "API seed timeout"
                        assert select.select([p.stdout], [], [], max(.1, deadline-time.monotonic()))[0], "API seed timeout"
                        reply = json.loads(p.stdout.readline())
                        if reply.get("reply_to") == frame["id"]:
                            assert reply["ev"] in ("hello_ok", "attached", "ok"), reply
                            break
            finally:
                p.stdin.close()
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()

        try:
            help_text = subprocess.check_output([str(cli), "--no-update", "api", "--help"], env=env, text=True, timeout=20)
            assert "--stdio" in help_text
            for name, environment in envs.items():
                daemon = launch(name+"-daemon", [str(cli), "--no-update", "serve"], environment)
                wait(name+" daemon", lambda e=environment, d=daemon: alive_socket(e["JCODE_SOCKET"], d))
            for key in ("host_key", "client_key"):
                subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root/key)], check=True, timeout=20)
            user = subprocess.check_output(["id", "-un"], text=True).strip()
            with socket.socket() as s:
                s.bind(("127.0.0.1", 0))
                port = s.getsockname()[1]
            remote_env = envs["remote"]
            remote_script = root / "remote.sh"
            exports = " ".join(f"{k}={shlex.quote(remote_env[k])}" for k in ("HOME", "JCODE_HOME", "JCODE_SOCKET", "JCODE_RUNTIME_DIR", "XDG_RUNTIME_DIR", "JCODE_NO_TELEMETRY", "JCODE_WAKE_MODE"))
            remote_script.write_text(f'export {exports}\ncd "$HOME" || exit 1\nexec /bin/sh -c "$SSH_ORIGINAL_COMMAND"\n')
            server_config = root / "sshd_config"
            server_config.write_text(f"Port {port}\nListenAddress 127.0.0.1\nHostKey {root/'host_key'}\nPidFile {root/'sshd.pid'}\nAuthorizedKeysFile {root/'client_key.pub'}\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nUseDNS no\nPermitUserRC no\nAllowUsers {user}\nForceCommand /bin/sh {remote_script}\n")
            sshd = launch("sshd", ["/usr/bin/sshd", "-D", "-e", "-f", str(server_config)], env)
            def listening():
                assert sshd.poll() is None, "Private sshd exited"
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=.2):
                        return True
                except OSError:
                    return False
            wait("sshd", listening)
            known = root / "known_hosts"
            known.write_text("jcode-test "+(root/"host_key.pub").read_text())
            ssh_config = root / "ssh_config"
            ssh_config.write_text(f"Host jcode-test\n HostName 127.0.0.1\n Port {port}\n User {user}\n HostKeyAlias jcode-test\n IdentityFile {root/'client_key'}\n IdentitiesOnly yes\n IdentityAgent none\n UserKnownHostsFile {known}\n GlobalKnownHostsFile /dev/null\n")
            # Preserve the production SDK invocation and all its options. Only
            # SSH configuration selection is isolated by this transparent wrapper.
            (bindir/"ssh").write_text(f'#!/bin/sh\nexec /usr/bin/ssh -F {shlex.quote(str(ssh_config))} "$@"\n')
            (bindir/"ssh").chmod(0o700)
            config = root / "desktop.toml"
            config.write_text('[workspace]\ndefault_remote_host="jcode-test"\nremote_hosts=["jcode-test"]\n')
            env["JCODE_DESKTOP_CONFIG"] = str(config)
            read_fd, write_fd = os.pipe()
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x1000x24", "-nolisten", "tcp"], pass_fds=(write_fd,), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            processes.append(xvfb)
            os.close(write_fd)
            assert select.select([read_fd], [], [], 10)[0]
            env["DISPLAY"] = ":"+os.read(read_fd, 80).decode().strip()
            os.close(read_fd)
            wm = root / "openbox.xml"
            wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
            launch("openbox", ["openbox", "--sm-disable", "--config-file", str(wm)], env)
            drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
            env["VK_DRIVER_FILES"] = str(drivers[0])
            app = launch("desktop-initial", [str(desktop), "--no-hot-reload"], env)
            wait("remote startup promoted to real session", lambda: len(ids()) == 1 and ids()[0].startswith("ssh://jcode-test/"))
            first = ids()[0]
            capture("real-remote-startup")
            command("xdotool", "search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync")
            command("xdotool", "type", "--clearmodifiers", "/rename Remote alpha")
            command("xdotool", "key", "Return")
            wait("remote rename persisted", lambda: list(Path(remote_env["JCODE_HOME"]).rglob(unquote(first.split("/", 3)[3])+".json")))
            command("xdotool", "key", "--clearmodifiers", "super+n")
            wait("new panel uses remote default", lambda: len(ids()) == 2 and all(i.startswith("ssh://jcode-test/") for i in ids()))
            assert len(set(ids())) == 2
            remote_ids = ids()
            command("xdotool", "type", "--clearmodifiers", "/rename Remote beta")
            command("xdotool", "key", "Return")
            second = next(i for i in remote_ids if i != first)
            wait("second remote rename persisted", lambda: list(Path(remote_env["JCODE_HOME"]).rglob(unquote(second.split("/", 3)[3])+".json")))
            for session in remote_ids:
                seed(remote_env, unquote(session.split("/", 3)[3]))
            capture("two-real-remote-panels")
            time.sleep(1)  # Allow the normal crash-recovery checkpoint to flush.
            app.terminate()
            app.wait(timeout=10)
            Path(env["JCODE_DESKTOP_STATE"]).unlink(missing_ok=True)
            app = launch("desktop-restored", [str(desktop), "--no-hot-reload"], env)
            wait("remote identities restored", lambda: set(ids()) == set(remote_ids))
            diagnostic = Path(env["XDG_STATE_HOME"]) / "jcode-desktop/jcode-desktop.log"
            wait("SSH reattachment after process restart", lambda: diagnostic.exists() and all(diagnostic.read_text().count(f"session {sid} connected") >= 2 for sid in remote_ids))
            command("xdotool", "search", "--sync", "--onlyvisible", "--class", "^jcode-desktop$", "windowactivate", "--sync")
            command("xdotool", "key", "--clearmodifiers", "super+n")
            wait("restored default creates another remote panel", lambda: len(ids()) == 3 and all(i.startswith("ssh://jcode-test/") for i in ids()))
            capture("restored-default")
            command("xdotool", "mousemove", "130", "66", "click", "1")
            wait("native Machines picker opens", lambda: "sidebar=Machines" in Path(env["JCODE_DESKTOP_STATE"]).read_text())
            words = capture("real-machines-picker")
            # State dumps precede rasterization. click() waits for the control.
            # The isolated picker has local and remote cards in that order.
            # Host punctuation is unreliable in OCR, but the buttons are stable.
            click(words, "Connect", index=1)
            wait("picker Connect opens a real remote session", lambda: len(ids()) == 4 and all(i.startswith("ssh://jcode-test/") for i in ids()))
            click(capture("remote-connected-via-picker"), "Connect")
            wait("one-off local panel beside remote panels", lambda: len(ids()) == 5 and sum(not i.startswith("ssh://") for i in ids()) == 1 and all(not i.startswith("startup://") for i in ids()))
            click(capture("mixed-machines"), "Set")
            command("xdotool", "key", "--clearmodifiers", "super+n")
            wait("switching default back creates local panel", lambda: len(ids()) == 6 and sum(not i.startswith("ssh://") for i in ids()) == 2 and all(not i.startswith("startup://") for i in ids()))
            words = capture("local-default-with-remote-panels")
            # Local now shows Default, leaving the remote card as the only Set.
            click(words, "Set")
            command("xdotool", "key", "--clearmodifiers", "super+n")
            wait("setting remote default in UI creates a real remote panel", lambda: len(ids()) == 7 and sum(i.startswith("ssh://jcode-test/") for i in ids()) == 5)
            capture("remote-default-restored-in-ui")
            report = dict(real_desktop_bridge=True, real_openssh=True, picker_remote_connect=True, set_remote_default_in_ui=True, remote_native_rename=True, independent_local_and_remote_daemons=True, remote_startup=first, unique_remote_default_panels=True, restored_remote_session_ids=remote_ids, remote_default_after_restart=True, one_off_local_override=True, switch_default_back_local=True, final_panel_ids=ids(), model_calls=0)
            (output/"report.json").write_text(json.dumps(report, indent=2)+"\n")
            print(json.dumps(report, indent=2), flush=True)
        finally:
            for p in reversed(processes):
                if p.poll() is None:
                    try:
                        if os.getpgid(p.pid) == p.pid:
                            os.killpg(p.pid, signal.SIGTERM)
                        else:
                            p.terminate()
                        p.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        if os.getpgid(p.pid) == p.pid:
                            os.killpg(p.pid, signal.SIGKILL)
                        else:
                            p.kill()
                        p.wait()
            for log in logs:
                log.close()
            for name, environment in envs.items():
                diagnostic = Path(environment["XDG_STATE_HOME"]) / "jcode-desktop/jcode-desktop.log"
                if diagnostic.exists():
                    (output/f"{name}-desktop-diagnostics.log").write_text(diagnostic.read_text())
            state = Path(env["JCODE_DESKTOP_STATE"])
            if state.exists():
                (output/"final-state.txt").write_text(state.read_text())


if __name__ == "__main__":
    main()
