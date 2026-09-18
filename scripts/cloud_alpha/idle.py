#!/usr/bin/env python3
"""Conservative AL2023 host idle shutdown, stdlib only, run as root every minute.

Install this file root:root 0755 at /usr/local/sbin/jcode-cloud-idle.
Bootstrap owns the units, with the following contract:
  [Service] Type=oneshot; User=root
  ExecStart=/usr/bin/python3 /usr/local/sbin/jcode-cloud-idle
  TimeoutStartSec=45
  [Timer] OnBootSec=60; OnUnitActiveSec=60; AccuracySec=5
Use normal host /proc and /tmp (NOT PrivateTmp=yes). Enable the timer, not a
long-running service. --check prints the same inspection/decision JSON but
NEVER changes idle state or requests shutdown. State is root-only under /run,
uses boot ID plus monotonic time, and resets after a missed observation >90s.

Alpha policy: any connected SSH transport (even an unused forwarded channel),
any ec2-user process, any Jcode process/descendant, running background record,
or unknown observation keeps the host on. The final product target is genuine
agent/tool/user activity, not mere SSH connectivity. Disk persistence preserves
files, NOT processes. An independent two-hour lease bounds fail-safe costs.

IMPORTANT runtime contract, inspected in ../jcode:
* harness-api-server/src/translate.rs list_sessions synthesizes status 'idle'
  for unattached persisted records, 'attached' for the attached session. Neither
  proves live inactivity. Live session_status events use 'running'/'idle', but
  an unattached listing does not supply these. Swarm snapshots are also stale.
* api --stdio starts a daemon if absent. We never invoke it on an idle host.
  When a daemon already exists, a bounded runuser probe sends hello then
  list_sessions WITHOUT create/attach, for diagnostics only. Even an empty
  successful response is NOT an authoritative snapshot. A live daemon therefore
  fails safe today. Do not whitelist it until runtime offers a complete, fresh,
  passive activity snapshot including queued turns and background work.
* jcode-base/src/background/model.rs stores *.status.json in
  std::env::temp_dir()/jcode-bg-tasks (normally /tmp), NOT ~/.jcode/bg-tasks.
  jcode-background-types defines running/completed/failed/superseded. Never
  infer completion from a missing PID or stale mtime. Custom TMPDIR is discovered
  from live Jcode environments. Detached ec2-user work is independently counted.
"""

import argparse
import fcntl
import json
import math
import os
from pathlib import Path
import pwd
import selectors
import signal
import socket
import subprocess
import time

USER = "ec2-user"
HOME = Path("/home/ec2-user")
STATE_DIR = Path("/run/jcode-cloud-idle")
IDLE_SECONDS = 30 * 60
MAX_GAP = 90
MAX_BYTES = 4 * 1024 * 1024
TERMINAL_TASKS = {"completed", "failed", "superseded"}


def policy(observation):
    """Pure policy: only complete positive evidence of inactivity is idle."""
    if not isinstance(observation, dict):
        return {"idle": False, "reasons": ["unknown observation"]}
    errors = observation.get("errors")
    reasons = list(errors) if isinstance(errors, list) else ["unknown inspection errors"]
    reasons = [str(reason) for reason in reasons]
    if type(observation.get("ssh")) is not bool:
        reasons.append("unknown SSH state")
    for field in ("work_pids", "jcode_pids"):
        if not isinstance(observation.get(field), list):
            reasons.append("unknown " + field)
    for field, description in (("ssh", "connected SSH"),
                               ("work_pids", "live user/Jcode/descendant processes")):
        if field not in observation:
            reasons.append("missing " + field)
        elif observation[field]:
            reasons.append(description)
    tasks = observation.get("tasks")
    if not isinstance(tasks, list):
        reasons.append("unknown background task inventory")
    else:
        for task in tasks:
            if (not isinstance(task, dict) or not isinstance(task.get("status"), str)
                    or task.get("status") not in TERMINAL_TASKS):
                reasons.append("running or unknown background task")
            elif task.get("pid_alive") is not False:
                reasons.append("background task PID alive or unknown")
    # Never trust list_sessions' synthetic 'idle' as live agent state.
    if observation.get("jcode_pids"):
        reasons.append("live Jcode: API listing is not authoritative activity")
    return {"idle": not reasons, "reasons": sorted(set(reasons))}


def advance(previous, decision, boot_id, now):
    """Pure timer transition. Missing/corrupt state, reboot or gaps restart grace."""
    since = now
    if decision["idle"] and isinstance(previous, dict):
        last = previous.get("checked_at")
        old_since = previous.get("idle_since")
        numbers = (last, old_since)
        if (previous.get("boot_id") == boot_id
                and all(type(v) in (int, float) and math.isfinite(v) for v in numbers)
                and 0 <= old_since <= last <= now and now - last <= MAX_GAP):
            since = old_since
    state = {"boot_id": boot_id, "checked_at": now,
             "idle_since": since if decision["idle"] else None}
    elapsed = now - since if decision["idle"] else 0
    return state, elapsed >= IDLE_SECONDS, elapsed


def read_json(path):
    with path.open("rb") as stream:
        data = stream.read(MAX_BYTES + 1)
    if len(data) > MAX_BYTES:
        raise ValueError("oversized JSON: " + str(path))
    return json.loads(data)


def processes(proc=Path("/proc")):
    """One proc snapshot. Vanished PIDs exited, all other read errors propagate."""
    rows = {}
    for directory in proc.iterdir():
        if not directory.name.isdigit():
            continue
        try:
            status = directory.joinpath("status").read_text()
            fields = dict(line.split(":", 1) for line in status.splitlines() if ":" in line)
            raw = directory.joinpath("cmdline").read_bytes()
            rows[int(directory.name)] = {
                "uid": int(fields["Uid"].split()[0]),
                "ppid": int(fields["PPid"]),
                "name": fields["Name"].strip(),
                "argv": [p.decode(errors="replace") for p in raw.split(b"\0") if p],
                "state": fields["State"].strip().split()[0],
            }
        except (FileNotFoundError, ProcessLookupError):
            continue
    return rows


def work_processes(rows, uid):
    jcode = {pid for pid, row in rows.items()
             if row["state"] != "Z" and (row["name"].startswith("jcode")
                 or (row["argv"] and Path(row["argv"][0]).name.startswith("jcode")))}
    work = jcode | {pid for pid, row in rows.items()
                    if row["uid"] == uid and row["state"] != "Z"}
    # Include root tools launched by sudo as well as ec2-user descendants.
    while True:
        descendants = {pid for pid, row in rows.items()
                       if row["ppid"] in work and row["state"] != "Z"}
        expanded = work | descendants
        if expanded == work:
            return sorted(work), sorted(jcode)
        work = expanded


def ssh_connected(rows, proc=Path("/proc")):
    # SSH sessions may have no PTY (desktop API, forwarding, scp). 'who' is not
    # sufficient. Root sshd children also cover a non-default listening port.
    if any(row["name"].startswith("sshd") and row["argv"]
           and ("sshd:" in " ".join(row["argv"]))
           and "[listener]" not in " ".join(row["argv"])
           for row in rows.values()):
        return True
    for name in ("tcp", "tcp6"):
        path = proc / "net" / name
        if name == "tcp6" and not path.exists():
            continue  # IPv6 can be disabled on the host.
        for line in path.read_text().splitlines()[1:]:
            fields = line.split()
            if len(fields) < 4:
                raise ValueError("malformed /proc/net/" + name)
            if fields[3] == "01" and int(fields[1].rsplit(":", 1)[1], 16) == 22:
                return True
    return False


def background_tasks(rows, jcode_pids, home=HOME):
    directories = {Path("/tmp/jcode-bg-tasks"), Path("/var/tmp/jcode-bg-tasks"),
                   home / ".jcode/bg-tasks"}
    for pid in jcode_pids:
        try:
            environment = Path("/proc", str(pid), "environ").read_bytes()
        except (FileNotFoundError, ProcessLookupError):
            continue
        for item in environment.split(b"\0"):
            if item.startswith(b"TMPDIR="):
                value = os.fsdecode(item.split(b"=", 1)[1])
                if not value or not Path(value).is_absolute():
                    raise ValueError("unknown Jcode TMPDIR")
                directories.add(Path(value) / "jcode-bg-tasks")
    tasks = []
    for directory in sorted(directories):
        try:
            paths = list(directory.iterdir())
        except FileNotFoundError:
            continue
        for path in paths:
            if not path.name.endswith(".status.json"):
                continue
            task = read_json(path)
            if not isinstance(task, dict):
                raise ValueError("malformed background record: " + str(path))
            pid = task.get("pid")
            if pid is not None and (type(pid) is not int or pid <= 0):
                raise ValueError("malformed background PID")
            tasks.append({"status": task.get("status"), "pid": pid,
                          "pid_alive": pid in rows and rows[pid]["state"] != "Z"})
    return tasks


def runtime_environment(rows, jcode_pids, uid):
    """Match harness-api/src/sockets.rs using the live daemon environment."""
    candidates = {}
    for pid in jcode_pids:
        if rows[pid]["uid"] != uid:
            continue
        try:
            raw = Path("/proc", str(pid), "environ").read_bytes()
        except (FileNotFoundError, ProcessLookupError):
            continue
        env = dict(os.fsdecode(item).split("=", 1) for item in raw.split(b"\0") if b"=" in item)
        discriminator = "".join(c for c in env.get("UID", env.get("USER", "user"))
                                if c.isascii() and (c.isalnum() or c in "-_"))[:64] or "user"
        temporary = env.get("TMPDIR", "/tmp")
        runtime = env.get("JCODE_RUNTIME_DIR", env.get("XDG_RUNTIME_DIR",
                             str(Path(temporary) / ("jcode-" + discriminator))))
        endpoint = env.get("JCODE_SOCKET", str(Path(runtime) / "jcode.sock"))
        candidates[endpoint] = {"JCODE_SOCKET": endpoint, "JCODE_RUNTIME_DIR": runtime,
                                "TMPDIR": temporary}
    if len(candidates) != 1:
        raise ValueError("no unique live ec2-user runtime endpoint")
    return next(iter(candidates.values()))


def api_sessions(home=HOME, timeout=5, runtime_env=None):
    """Bounded diagnostic only. Never create or attach an agent session."""
    # Verify the existing native endpoint before invoking CLI, which otherwise
    # auto-starts the daemon. A concurrent daemon exit still fails safe.
    if runtime_env is None:
        raise ValueError("API probe requires a discovered live runtime")
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(1)
        connection.connect(runtime_env["JCODE_SOCKET"])
    command = ["/usr/sbin/runuser", "-u", USER, "--", "/usr/bin/env",
               "-i", "HOME=" + str(home), "USER=" + USER, "LOGNAME=" + USER,
               "PATH=/usr/local/bin:/usr/bin:/bin", "JCODE_NON_INTERACTIVE=1",
               *(key + "=" + value for key, value in runtime_env.items()),
               str(home / ".local/bin/jcode"), "api", "--stdio"]
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, start_new_session=True)
    deadline = time.monotonic() + timeout
    data = bytearray()
    total = 0
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        try:
            def send(frame):
                process.stdin.write((json.dumps(frame) + "\n").encode())
                process.stdin.flush()

            send({"v": 1, "id": 1, "req": "hello", "min_version": 1,
                  "max_version": 1, "client": "cloud-idle-inspection"})
            hello = False
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise TimeoutError("API inspection timed out")
                chunk = os.read(process.stdout.fileno(), 65536)
                if not chunk:
                    raise ValueError("API EOF before session inventory")
                data.extend(chunk)
                total += len(chunk)
                if total > MAX_BYTES:
                    raise ValueError("oversized API response")
                while b"\n" in data:
                    line, _, rest = data.partition(b"\n")
                    data = bytearray(rest)
                    frame = json.loads(line)
                    if frame.get("v") != 1 or frame.get("ev") == "error":
                        raise ValueError("API error or unsupported version")
                    if frame.get("reply_to") == 1 and frame.get("ev") == "hello_ok":
                        hello = True
                        send({"v": 1, "id": 2, "req": "list_sessions", "include_archived": True})
                    elif hello and frame.get("reply_to") == 2 and frame.get("ev") == "sessions":
                        sessions = frame.get("sessions")
                        if not isinstance(sessions, list):
                            raise ValueError("invalid session inventory")
                        return {"authoritative": False, "sessions": [
                            {"status": s.get("status"), "swarm_status": s.get("swarm_status")}
                            for s in sessions]}
        finally:
            # EOF lets runuser/PAM close cleanly. Bound cleanup too, and target
            # only our probe group, never the pre-existing daemon/tools.
            try:
                try:
                    process.stdin.close()
                except BrokenPipeError:
                    pass
                try:
                    process.wait(timeout=0.3)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait(timeout=2)
            finally:
                process.stdout.close()


def inspect():
    observation = {"errors": [], "ssh": False, "work_pids": [],
                   "jcode_pids": [], "tasks": [], "api": None}
    try:
        uid = pwd.getpwnam(USER).pw_uid
        rows = processes()
        rows.pop(os.getpid(), None)  # Direct shebang invocation can be named jcode-cloud-idle.
        work, jcode = work_processes(rows, uid)
        observation.update(work_pids=work, jcode_pids=jcode)
        observation["ssh"] = ssh_connected(rows)
        observation["tasks"] = background_tasks(rows, jcode)
        if jcode:
            observation["api"] = api_sessions(runtime_env=runtime_environment(rows, jcode, uid))
    except Exception as error:
        observation["errors"].append(type(error).__name__ + ": " + str(error))
    return observation


def load_state(path):
    try:
        return read_json(path)
    except (OSError, ValueError):
        return None


def save_state(path, state):
    temporary = path.with_suffix(".new")
    with temporary.open("w") as stream:
        json.dump(state, stream)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def evaluate(previous):
    observation = inspect()
    decision = policy(observation)
    boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    state, stop, elapsed = advance(previous, decision, boot_id, time.monotonic())
    return state, {"observation": observation, **decision,
                   "idle_seconds": elapsed, "threshold_seconds": IDLE_SECONDS,
                   "would_poweroff": stop}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="JSON inspection only, no state writes or shutdown")
    args = parser.parse_args(argv)
    path = STATE_DIR / "state.json"
    try:
        if args.check:
            _, result = evaluate(load_state(path))
            result["action"] = "check_only"
        else:
            if os.geteuid() != 0:
                raise PermissionError("timer must run as root")
            os.umask(0o077)
            STATE_DIR.mkdir(mode=0o700, parents=True, exist_ok=True)
            with (STATE_DIR / "lock").open("w") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                try:
                    state, result = evaluate(load_state(path))
                    if result["would_poweroff"]:
                        # Re-scan work after decision, immediately before shutdown.
                        state, result = evaluate(state)
                    save_state(path, state)
                    result["action"] = "keep_running"
                    if result["would_poweroff"]:
                        subprocess.run(["/usr/bin/systemctl", "poweroff"], check=True, timeout=10)
                        result["action"] = "poweroff_requested"
                except Exception:
                    # A failed check must not leave a previously armed grace
                    # interval usable on the very next successful minute.
                    try:
                        save_state(path, {})
                    except OSError:
                        pass
                    raise
        print(json.dumps(result, sort_keys=True))
        return 0
    except Exception as error:
        print(json.dumps({"idle": False, "would_poweroff": False, "action": "keep_running",
                          "errors": [type(error).__name__ + ": " + str(error)]}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
