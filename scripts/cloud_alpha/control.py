#!/usr/bin/env python3
"""Operate one configured personal alpha. Never creates infrastructure or exports credentials."""
import argparse
from datetime import datetime, timezone
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
    rule = aws(config, "events", "describe-rule", "--name", config["guard_rule"])
    fn = aws(config, "lambda", "get-function-configuration", "--function-name", config["guard_function"])
    if rule["State"] != "ENABLED" or fn.get("State") != "Active":
        raise RuntimeError("Independent runtime guard is unavailable. Refusing wake.")
    key = {"pk": {"S": "INSTANCE#" + config["instance_id"]}, "sk": {"S": "STATE"}}
    state = aws(config, "dynamodb", "get-item", "--table-name", config["ledger_table"],
                "--consistent-read", "--key", json.dumps(key)).get("Item")
    if not state:
        raise RuntimeError("Independent guard has not initialized. Refusing wake.")
    checked = datetime.fromisoformat(state["checked_at"]["S"])
    if not 0 <= (datetime.now(timezone.utc) - checked).total_seconds() <= 180:
        raise RuntimeError("Independent guard heartbeat is stale. Refusing wake.")


def wake(config):
    check_guard(config)
    consumed, budget, depleted = ledger(config)
    if depleted or consumed >= budget:
        raise RuntimeError("Cloud runtime allowance exhausted. No automatic overage is enabled.")
    host = instance(config)
    state = host["State"]["Name"]
    if state == "stopping":
        raise RuntimeError("Host is still stopping. Retry once stopped.")
    if state == "stopped":
        aws(config, "ec2", "start-instances", "--instance-ids", config["instance_id"])
    elif state not in ("running", "pending"):
        raise RuntimeError(f"Cannot wake host in state {state}")
    print("Waiting for cloud host and private SSM connection...", file=sys.stderr)
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        info = aws(config, "ssm", "describe-instance-information", "--filters",
                   f"Key=InstanceIds,Values={config['instance_id']}")
        if (any(i["PingStatus"] == "Online" for i in info["InstanceInformationList"])
                and instance(config)["State"]["Name"] == "running"):
            # SSM can briefly report the previous boot as Online. Verify the real
            # SSH path and bootstrap marker before advertising readiness.
            try:
                ready = subprocess.run(
                    ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8",
                     "jcode-cloud-alpha", "test -f /opt/jcode-alpha/ready"],
                    capture_output=True, timeout=20).returncode == 0
            except subprocess.TimeoutExpired:
                ready = False
            if ready:
                print("Ready. Desktop → Machines → jcode-cloud-alpha → Connect.")
                print("The alpha stops after a maximum two-hour continuous lease, including active work.")
                return
        time.sleep(4)
    raise RuntimeError("Host did not become SSM-ready within four minutes. Check bootstrap via SSM.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["status", "wake", "stop", "ssh", "proxy"])
    args = parser.parse_args()
    config = json.loads(CONFIG.read_text())
    # Ensure the AWS CLI can find the user-installed Session Manager plugin.
    os.environ["PATH"] = str(Path.home() / ".local/bin") + os.pathsep + os.environ.get("PATH", "")
    identity(config)
    if args.action == "wake":
        wake(config)
    elif args.action == "stop":
        instance(config)
        aws(config, "ec2", "stop-instances", "--instance-ids", config["instance_id"])
        print("Stop requested. Saved files remain. Running processes will end.")
    elif args.action == "status":
        host = instance(config)
        used, budget, depleted = ledger(config)
        print(json.dumps({"instance_id": config["instance_id"], "state": host["State"]["Name"],
                          "machine": host["InstanceType"], "used_minutes": used,
                          "allowance_minutes": budget, "depleted": depleted,
                          "maximum_continuous_hours": 2, "persistent_disk_gb": 30}, indent=2))
    elif args.action == "ssh":
        wake(config)
        os.execvp("ssh", ["ssh", "jcode-cloud-alpha"])
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
