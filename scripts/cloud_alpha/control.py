#!/usr/bin/env python3
"""Operate one configured personal alpha. Never creates infrastructure or exports credentials."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import time

CONFIG = Path.home() / ".config/jcode/cloud-alpha.json"


def aws(config, service, action, *args, timeout=40):
    command = ["aws", "--profile", config["profile"], "--region", config["region"],
               "--no-cli-pager", "--cli-connect-timeout", "5", "--cli-read-timeout", "20",
               service, action, *args, "--output", "json"]
    out = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    if out.returncode:
        # AWS diagnostics, never credentials, go to the operator.
        raise RuntimeError(out.stderr.strip() or f"{service} {action} failed")
    return json.loads(out.stdout) if out.stdout.strip() else {}


def identity(config):
    result = aws(config, "sts", "get-caller-identity")
    if result["Account"] != config["account_id"]:
        raise RuntimeError("AWS account differs from the recorded alpha account. Refusing operation.")


def instance(config):
    data = aws(config, "ec2", "describe-instances", "--instance-ids", config["instance_id"])
    hosts = [h for r in data["Reservations"] for h in r["Instances"]]
    if len(hosts) != 1 or hosts[0]["InstanceId"] != config["instance_id"]:
        raise RuntimeError("Expected exactly the configured host")
    if not any(t["Key"] == "Project" and t["Value"] == "jcode-cloud-alpha" for t in hosts[0].get("Tags", [])):
        raise RuntimeError("Configured host lacks the alpha ownership tag")
    return hosts[0]


def ledger(config):
    key = {"pk": {"S": "INSTANCE#" + config["instance_id"]},
           "sk": {"S": "MONTH#" + datetime.now(timezone.utc).strftime("%Y-%m")}}
    row = aws(config, "dynamodb", "get-item", "--table-name", config["ledger_table"],
              "--consistent-read", "--key", json.dumps(key)).get("Item")
    if not row:
        raise RuntimeError("Usage guard has not initialized this month. Wait one minute and retry.")
    consumed, budget = int(row["consumed_minutes"]["N"]), int(row["budget_minutes"]["N"])
    if consumed < 0 or budget <= 0:
        raise RuntimeError("Invalid usage counter. Refusing wake.")
    return consumed, budget, row["depleted"]["BOOL"]


def check_guard(config):
    key = {"pk": {"S": "INSTANCE#" + config["instance_id"]}, "sk": {"S": "STATE"}}
    # These are independent, read-only checks. Every result must pass before a
    # wake is permitted. Do not cache the heartbeat or renew the boot lease.
    with ThreadPoolExecutor(max_workers=3) as pool:
        rule_read = pool.submit(aws, config, "events", "describe-rule", "--name", config["guard_rule"])
        fn_read = pool.submit(aws, config, "lambda", "get-function-configuration", "--function-name", config["guard_function"])
        state_read = pool.submit(aws, config, "dynamodb", "get-item", "--table-name", config["ledger_table"],
                                 "--consistent-read", "--key", json.dumps(key))
        rule, fn, state = rule_read.result(), fn_read.result(), state_read.result().get("Item")
    if rule["State"] != "ENABLED" or fn.get("State") != "Active":
        raise RuntimeError("Independent runtime guard is unavailable. Refusing wake.")
    if not state:
        raise RuntimeError("Independent guard has not initialized. Refusing wake.")
    checked = datetime.fromisoformat(state["checked_at"]["S"])
    if checked.tzinfo is None or not 0 <= (datetime.now(timezone.utc) - checked).total_seconds() <= 180:
        raise RuntimeError("Independent guard heartbeat is stale. Refusing wake.")
    return checked


def wake(config, *, verify_identity=False, progress_only=False):
    if verify_identity:
        print("Checking AWS sign-in and cloud safety...", file=sys.stderr, flush=True)
    else:
        print("Checking cloud runtime guard and allowance...", file=sys.stderr, flush=True)
    # Direct callers retain the existing prevalidated-identity API. CLI callers
    # overlap identity with reads, never start/connect until all checks pass.
    with ThreadPoolExecutor(max_workers=4) as pool:
        identity_read = pool.submit(identity, config) if verify_identity else None
        guard_read = pool.submit(check_guard, config)
        ledger_read = pool.submit(ledger, config)
        host_read = pool.submit(instance, config)
        if identity_read is not None:
            identity_read.result()
            print("Checking cloud runtime guard and allowance...", file=sys.stderr, flush=True)
        checked = guard_read.result()
        consumed, budget, depleted = ledger_read.result()
        host = host_read.result()
    # Other reads may take tens of seconds after the guard check completes.
    # Refresh the age calculation before any start or SSH side effect.
    if not 0 <= (datetime.now(timezone.utc) - checked).total_seconds() < 180:
        raise RuntimeError("Independent guard heartbeat is stale. Refusing wake.")
    if depleted or consumed >= budget:
        raise RuntimeError("Cloud runtime allowance exhausted. No automatic overage is enabled.")
    state = host["State"]["Name"]
    if state == "stopping":
        raise RuntimeError("Host is still stopping. Retry once stopped.")
    if state == "stopped":
        print("Starting the shared cloud VM...", file=sys.stderr, flush=True)
        aws(config, "ec2", "start-instances", "--instance-ids", config["instance_id"])
    elif state not in ("running", "pending"):
        raise RuntimeError(f"Cannot wake host in state {state}")
    elif state == "running":
        print("Shared cloud VM is already running. Checking connection...", file=sys.stderr, flush=True)
    else:
        print("Shared cloud VM is starting...", file=sys.stderr, flush=True)
    cold = state in ("stopped", "pending")
    if cold:
        print("Waiting for shared cloud VM to become reachable...", file=sys.stderr, flush=True)
    else:
        print("Waiting for cloud host and private SSM connection...", file=sys.stderr, flush=True)
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        # The proxy already checks identity, ownership and current EC2 state.
        # Actual SSH + bootstrap readiness is stronger than SSM's delayed (or
        # stale-from-the-previous-boot) Online flag. Do not gate it on two more
        # CLI processes and control-plane round trips on every attempt.
        if not cold:
            print("Verifying SSH connection and cloud bootstrap readiness...", file=sys.stderr, flush=True)
        try:
            ready = subprocess.run(
                ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8",
                 "jcode-cloud-alpha", "test -f /opt/jcode-alpha/ready"],
                capture_output=True, timeout=min(20, max(0.1, deadline - time.monotonic()))).returncode == 0
        except subprocess.TimeoutExpired:
            ready = False
        if ready:
            if cold:
                print("Shared cloud VM is running. SSH bootstrap ready.", file=sys.stderr, flush=True)
            output = sys.stderr if progress_only else sys.stdout
            print("Ready. Desktop → Machines → jcode-cloud-alpha → Connect.", file=output, flush=True)
            print("The alpha stops after a maximum two-hour continuous lease, including active work.",
                  file=output, flush=True)
            return
        time.sleep(4)
    raise RuntimeError("Host did not become SSM-ready within four minutes. Check bootstrap via SSM.")


def check_ready(config, now=None):
    """Read-only safety receipt, not proof of SSH/bootstrap readiness.

    A consumer may reuse an established transport only for this exact instance
    and launch time, for at most valid_for_seconds from observed_at (Unix
    seconds). launch_time retains the AWS string. This does not renew the
    lease or authorize a future wake.
    """
    with ThreadPoolExecutor(max_workers=4) as pool:
        identity_read = pool.submit(identity, config)
        guard_read = pool.submit(check_guard, config)
        ledger_read = pool.submit(ledger, config)
        host_read = pool.submit(instance, config)
        identity_read.result()
        checked = guard_read.result()
        consumed, budget, depleted = ledger_read.result()
        host = host_read.result()
    if depleted or consumed >= budget:
        raise RuntimeError("Cloud runtime allowance exhausted. No automatic overage is enabled.")
    state = host["State"]["Name"]
    if state != "running":
        raise RuntimeError(f"Cloud host is not running (state {state}). Readiness check cannot wake it.")
    now = now or datetime.now(timezone.utc)
    launch = datetime.fromisoformat(host["LaunchTime"])
    if launch.tzinfo is None or now.tzinfo is None or launch > now:
        raise RuntimeError("Invalid cloud launch time. Refusing readiness receipt.")
    launch = launch.astimezone(timezone.utc)
    deadline = launch + timedelta(hours=2)
    remaining = int((deadline - now).total_seconds())
    if remaining <= 0:
        raise RuntimeError("Cloud two-hour lease expired. Refusing readiness receipt.")
    # Re-evaluate freshness after every read completes, not just when the
    # heartbeat arrived. No receipt may outlive its guard, lease or allowance.
    guard_age = (now - checked).total_seconds()
    if not 0 <= guard_age < 180:
        raise RuntimeError("Independent guard heartbeat is stale. Refusing readiness receipt.")
    valid_for = int(min(30, remaining, 180 - guard_age, (budget - consumed) * 60))
    if valid_for <= 0:
        raise RuntimeError("Safety proof expires too soon. Refusing readiness receipt.")
    return {"instance_id": config["instance_id"], "state": state,
            "valid_for_seconds": valid_for,
            "launch_time": host["LaunchTime"], "lease_deadline": int(deadline.timestamp()),
            "lease_remaining_seconds": remaining,
            "allowance_remaining_minutes": budget - consumed,
            "observed_at": int(now.timestamp())}


def wake_ready(config):
    """Explicit wake plus a fresh full safety proof after bootstrap succeeds."""
    wake(config, verify_identity=True, progress_only=True)
    print("Confirming cloud safety checks...", file=sys.stderr, flush=True)
    return check_ready(config)


def status(config, now=None):
    """Read-only status. Countdown is advisory, never permission to extend a lease."""
    with ThreadPoolExecutor(max_workers=2) as pool:
        host_read = pool.submit(instance, config)
        ledger_read = pool.submit(ledger, config)
        host = host_read.result()
        used, budget, depleted = ledger_read.result()
    now = now or datetime.now(timezone.utc)
    state = host["State"]["Name"]
    deadline, remaining = None, None
    if state in ("pending", "running", "stopping"):
        launch = datetime.fromisoformat(host["LaunchTime"])
        if launch.tzinfo is None or now.tzinfo is None or launch > now:
            raise RuntimeError("Invalid cloud launch time. Cannot estimate shutdown deadline.")
        deadline = launch + timedelta(hours=2)
        remaining = max(0, int((deadline - now).total_seconds()))
    return {"instance_id": config["instance_id"], "state": state,
            "machine": host["InstanceType"], "used_minutes": used,
            "allowance_minutes": budget, "remaining_minutes": max(0, budget - used),
            "depleted": depleted, "maximum_continuous_hours": 2,
            "lease_deadline": deadline.isoformat() if deadline else None,
            "lease_remaining_seconds": remaining,
            "observed_at": now.isoformat(), "persistent_disk_gb": 30}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["status", "wake", "stop", "ssh", "proxy", "check-ready", "wake-ready"])
    args = parser.parse_args()
    config = json.loads(CONFIG.read_text())
    # Ensure the AWS CLI can find the user-installed Session Manager plugin.
    os.environ["PATH"] = str(Path.home() / ".local/bin") + os.pathsep + os.environ.get("PATH", "")
    if args.action in ("wake", "ssh", "wake-ready"):
        print("Checking AWS sign-in...", file=sys.stderr, flush=True)
    if args.action == "check-ready":
        print(json.dumps(check_ready(config), indent=2))
        return
    if args.action == "wake-ready":
        print(json.dumps(wake_ready(config), indent=2))
        return
    if args.action in ("wake", "ssh"):
        wake(config, verify_identity=True)
        if args.action == "ssh":
            os.execvp("ssh", ["ssh", "jcode-cloud-alpha"])
        return
    identity(config)
    if args.action == "stop":
        instance(config)
        aws(config, "ec2", "stop-instances", "--instance-ids", config["instance_id"])
        print("Stop requested. Saved files remain. Running processes will end.")
    elif args.action == "status":
        print(json.dumps(status(config), indent=2))
    elif args.action == "proxy":
        # SSH's stdout is its wire. Never print diagnostics or status there.
        if instance(config)["State"]["Name"] != "running":
            raise RuntimeError("Cloud host is asleep. Run `jcode-cloud-alpha wake`, then retry the panel.")
        os.execvp("aws", ["aws", "ssm", "start-session", "--profile", config["profile"],
                         "--region", config["region"], "--target", config["instance_id"],
                         "--document-name", "AWS-StartSSHSession", "--parameters", "portNumber=22"])


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"jcode-cloud-alpha: {error}", file=sys.stderr)
        sys.exit(1)
