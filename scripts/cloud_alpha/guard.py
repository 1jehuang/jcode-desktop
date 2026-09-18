"""Single-instance personal EC2 runtime guard (guard.lambda_handler / index.handler).

Configuration: INSTANCE_ID and TABLE_NAME are required. MONTHLY_HOURS defaults
to 50, MAX_BOOT_HOURS to 2. Schedule every minute; reserve concurrency = 1 where
account quota permits. Conditional transactions also fail closed on overlap.
Fractional monthly limits round DOWN to whole minutes (minimum one minute).
IAM: ec2:DescribeInstances (Resource '*'), ec2:StopInstances restricted to the
configured instance ARN, dynamodb:GetItem and dynamodb:PutItem on this table
(the latter authorizes the Put actions in TransactWriteItems), plus Lambda logs.
No start, terminate, provisioning, IAM, or other cloud mutation is performed.

Table: string partition key pk = 'INSTANCE#<id>', string sort key sk.
'STATE' holds version, checked_at, observation_started_at, ec2_state, launch_at.
'MONTH#YYYY-MM' holds consumed_seconds (authoritative), consumed_minutes
(rounded-up cumulative seconds), budget_minutes, depleted, updated_at.
Times/months are UTC. Legacy minute-only rows migrate as minutes * 60.
No TTL: retain state and counters. Protect them from deletion/manual edits.
The operator start helper must strongly read the current month row and reject
consumed_minutes >= its configured budget (or depleted). Missing/unavailable
state must fail closed in that helper, not mean permission to start. No status
file is maintained. The guard independently stops repeated starts at depletion.

Accounting: round each charged observation interval UP to whole seconds,
splitting at UTC month boundaries. Charge the entire gap, including missed
invocations and stop/start transitions. Only two consecutive 'stopped'
observations with identical EC2 LaunchTime prove an idle interval and skip it.
EC2 LaunchTime changes on stop/start, not ordinary reboot: use it for the boot
lease, never the previous invocation time. Thus even stopped-to-stopped hidden
starts are charged. Pending/stopping intervals and uncertain gaps overcount.
Charged gaps begin at the previous EC2 request start and end after the current
response, conservatively overlapping request latency instead of losing races.
Round cumulative seconds up to minutes for display and budget enforcement,
not each individual interval. For example, ten 60.1-second gaps charge 610
seconds (11 displayed minutes), not 20 minutes. Rounding overcounts by less
than one second per charged interval/month segment, plus unknown-gap overlap.
These rules assume accurate EC2 observations, not strongly consistent EC2 APIs.
On initial enrollment, charge an active instance from LaunchTime; a stopped
instance starts tracking now. History before enrollment cannot be reconstructed.
Install before allowing operator starts. Deleting state invalidates accounting.

A transaction commits all affected months and a version-conditional STATE
write together. Errors (including read, parsing, or transaction uncertainty)
request StopInstances for ONLY INSTANCE_ID and are logged and raised. A missed
span over 98 months fails closed rather than partially recording it.

At each invocation, request stop at either exhausted monthly budget or boot
age >= MAX_BOOT_HOURS. With healthy one-minute delivery this adds at most one
minute of detection overshoot, not a guarantee about eventual EventBridge
failures, EC2 stop completion, or API outages. This is a runtime limit, NOT a
hard total-dollar cap or a model-spend guard. EBS and other charges can persist.
"""

import logging
import os
from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation

LOG = logging.getLogger(__name__)
UTC = timezone.utc
ACTIVE_STATES = {"pending", "running", "stopping"}
VALID_STATES = ACTIVE_STATES | {"stopped", "shutting-down", "terminated"}


def utc(value):
    """Require an aware datetime and normalize to UTC."""
    if not isinstance(value, datetime) or value.tzinfo is None:
        raise ValueError("expected timezone-aware datetime")
    return value.astimezone(UTC)


def stamp(value):
    return utc(value).isoformat()


def parse_time(value):
    return utc(datetime.fromisoformat(value))


def month_key(value):
    return utc(value).strftime("%Y-%m")


def next_month(value):
    return datetime(value.year + (value.month == 12),
                    value.month % 12 + 1, 1, tzinfo=UTC)


def charge_seconds(start, end):
    """Pure conservative UTC month allocation, with integer microsecond math."""
    start, end = utc(start), utc(end)
    if end < start:
        raise ValueError("clock moved backwards")
    charges = {}
    while start < end:
        if len(charges) >= 98:
            raise ValueError("accounting gap exceeds transaction capacity")
        boundary = min(next_month(start), end)
        elapsed = boundary - start
        micros = ((elapsed.days * 86400 + elapsed.seconds) * 1000000
                  + elapsed.microseconds)
        charges[month_key(start)] = (micros + 999999) // 1000000
        start = boundary
    return charges


def charge_minutes(start, end):
    """Standalone interval display; accounting uses seconds, not this rounding."""
    return {month: (seconds + 59) // 60
            for month, seconds in charge_seconds(start, end).items()}


@dataclass(frozen=True)
class Config:
    instance_id: str
    table_name: str
    budget_minutes: int
    boot_seconds: Decimal

    @classmethod
    def from_env(cls, env):
        instance_id, table_name = env["INSTANCE_ID"], env["TABLE_NAME"]
        if not instance_id or not table_name:
            raise ValueError("INSTANCE_ID and TABLE_NAME must be nonempty")
        try:
            monthly = Decimal(env.get("MONTHLY_HOURS", "50")) * 60
            boot = Decimal(env.get("MAX_BOOT_HOURS", "2")) * 3600
            if not monthly.is_finite() or not boot.is_finite():
                raise ValueError("limits must be finite")
            if monthly < 1 or boot <= 0:
                raise ValueError("monthly limit must be >= 1 minute; boot limit > 0")
            return cls(instance_id, table_name, int(monthly), boot)
        except InvalidOperation as exc:
            raise ValueError("invalid hour limit") from exc


def accounting(previous, state, launch, now):
    """Pure seconds policy: charge unknown gaps, skip only proven idle ones."""
    now, launch = utc(now), utc(launch)
    if state not in VALID_STATES or launch > now:
        raise ValueError("invalid EC2 state or future LaunchTime")
    if previous is None:
        return charge_seconds(launch, now) if state in ACTIVE_STATES else {}
    checked = parse_time(previous["checked_at"])
    started = parse_time(previous["observation_started_at"])
    old_launch = parse_time(previous["launch_at"])
    if started > checked or checked > now or old_launch > checked or launch < old_launch:
        raise ValueError("inconsistent observation timestamps")
    if previous["ec2_state"] not in VALID_STATES:
        raise ValueError("invalid persisted EC2 state")
    if state == previous["ec2_state"] == "stopped" and launch == old_launch:
        return {}
    return charge_seconds(started, now)


def stop_reasons(state, launch, now, consumed_minutes, config):
    """Pure stop decision. A depleted month remains depleted across restarts."""
    reasons = []
    if consumed_minutes >= config.budget_minutes:
        reasons.append("monthly_budget")
    age = utc(now) - utc(launch)
    age_seconds = Decimal(age.days * 86400 + age.seconds) + Decimal(age.microseconds) / 1000000
    if state in ACTIVE_STATES and age_seconds >= config.boot_seconds:
        reasons.append("boot_lease")
    return reasons


def encode(item):
    return {key: ({"BOOL": value} if isinstance(value, bool) else
                  {"N": str(value)} if isinstance(value, int) else {"S": value})
            for key, value in item.items()}


def decode(item):
    result = {}
    for key, value in item.items():
        if set(value) == {"S"}:
            result[key] = value["S"]
        elif set(value) == {"N"}:
            result[key] = int(value["N"])
        elif set(value) == {"BOOL"}:
            result[key] = value["BOOL"]
        else:
            raise ValueError("unexpected DynamoDB attribute")
    return result


def fail_closed(ec2, instance_id):
    LOG.exception("EC2 runtime guard failed; requesting stop for %s", instance_id)
    try:
        ec2.stop_instances(InstanceIds=[instance_id])
    except Exception:
        LOG.exception("Fail-closed stop also failed for %s", instance_id)


def run_guard(ec2, ddb, env, now=None):
    """Inject clients and time for offline tests. No boto3 import is needed."""
    instance_id = env["INSTANCE_ID"]
    if not instance_id:
        raise ValueError("INSTANCE_ID is required to safely stop the target")
    try:
        config = Config.from_env(env)
        pk = "INSTANCE#" + instance_id

        def read(sk):
            result = ddb.get_item(TableName=config.table_name,
                                  Key=encode({"pk": pk, "sk": sk}),
                                  ConsistentRead=True)
            item = result.get("Item")
            return decode(item) if item else None

        previous = read("STATE")
        version = previous["version"] if previous is not None else 0
        if type(version) is not int or version < 0 or (previous is not None and version == 0):
            raise ValueError("invalid state version")
        # Bracket the observation because its actual snapshot time is unknown.
        observation_started = utc(now if now is not None else datetime.now(UTC))
        response = ec2.describe_instances(InstanceIds=[instance_id])
        now = utc(now if now is not None else datetime.now(UTC))
        instances = [i for r in response["Reservations"] for i in r["Instances"]]
        if len(instances) != 1 or instances[0]["InstanceId"] != instance_id:
            raise ValueError("EC2 response did not identify exactly the configured instance")
        instance = instances[0]
        state, launch = instance["State"]["Name"], utc(instance["LaunchTime"])
        charges = accounting(previous, state, launch, now)
        current_month = month_key(now)
        charges.setdefault(current_month, 0)
        writes, consumed = [], 0
        for month, seconds in sorted(charges.items()):
            sk = "MONTH#" + month
            old = read(sk)
            old_minutes = old["consumed_minutes"] if old is not None else 0
            total_seconds = old.get("consumed_seconds", old_minutes * 60) if old is not None else 0
            if (type(old_minutes) is not int or old_minutes < 0
                    or type(total_seconds) is not int or total_seconds < 0
                    or old_minutes != (total_seconds + 59) // 60):
                raise ValueError("invalid monthly counter")
            # Missing historical counters cannot safely be treated as zero.
            if previous is not None and month == month_key(parse_time(previous["checked_at"])) and old is None:
                raise ValueError("persisted month counter is missing")
            total_seconds += seconds
            total = (total_seconds + 59) // 60
            item = {"pk": pk, "sk": sk, "consumed_seconds": total_seconds,
                    "consumed_minutes": total,
                    "budget_minutes": config.budget_minutes,
                    "depleted": total >= config.budget_minutes,
                    "updated_at": stamp(now)}
            writes.append({"Put": {"TableName": config.table_name, "Item": encode(item)}})
            if month == current_month:
                consumed = total
        state_item = {"pk": pk, "sk": "STATE", "version": version + 1,
                      "checked_at": stamp(now), "ec2_state": state,
                      "observation_started_at": stamp(observation_started),
                      "launch_at": stamp(launch)}
        put = {"TableName": config.table_name, "Item": encode(state_item),
               "ConditionExpression": "#version = :version" if previous else "attribute_not_exists(pk)"}
        if previous:
            put["ExpressionAttributeNames"] = {"#version": "version"}
            put["ExpressionAttributeValues"] = {":version": {"N": str(version)}}
        writes.append({"Put": put})
        ddb.transact_write_items(TransactItems=writes)
        reasons = stop_reasons(state, launch, now, consumed, config)
    except Exception:
        fail_closed(ec2, instance_id)
        raise
    # Persist BEFORE stopping, so retries or a failed stop cannot lose usage.
    if reasons and state in ACTIVE_STATES:
        try:
            ec2.stop_instances(InstanceIds=[instance_id])
        except Exception:
            LOG.exception("Policy stop failed for %s", instance_id)
            raise
    result = {"instance_id": instance_id, "month": current_month,
              "consumed_minutes": consumed, "budget_minutes": config.budget_minutes,
              "depleted": consumed >= config.budget_minutes,
              "stop_requested": bool(reasons and state in ACTIVE_STATES), "reasons": reasons}
    LOG.info("Runtime guard status: %s", result)
    return result


def lambda_handler(event, context):
    import boto3  # Provided by Lambda, deliberately optional for local tests.

    instance_id = os.environ["INSTANCE_ID"]
    if not instance_id:
        raise ValueError("INSTANCE_ID is required")
    ec2 = boto3.client("ec2")
    try:
        ddb = boto3.client("dynamodb")
    except Exception:
        fail_closed(ec2, instance_id)
        raise
    return run_guard(ec2, ddb, os.environ)


# CloudFormation inline Python is emitted as index.py.
handler = lambda_handler
