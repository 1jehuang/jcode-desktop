"""Offline tests: python3 -m unittest discover -s scripts/cloud_alpha -v.

Only stdlib is used. Fakes enforce strongly consistent reads, conditional state
writes, atomic transactions, and the exact target instance. No AWS calls occur.
"""

import copy
import os
from pathlib import Path
import subprocess
import sys
import unittest
from datetime import datetime, timedelta, timezone
from unittest.mock import patch

try:
    from . import guard
except ImportError:
    import guard

UTC = timezone.utc
ENV = {"INSTANCE_ID": "i-personal", "TABLE_NAME": "runtime-guard"}
BASE = datetime(2026, 9, 18, 6, 0, tzinfo=UTC)


class FakeEC2:
    def __init__(self, launch=BASE, state="running"):
        self.launch, self.state = launch, state
        self.stops = []
        self.describe_error = None
        self.stop_error = None
        self.returned_id = ENV["INSTANCE_ID"]
        self.before_stop = None

    def describe_instances(self, **kwargs):
        assert kwargs == {"InstanceIds": [ENV["INSTANCE_ID"]]}
        if self.describe_error:
            raise self.describe_error
        return {"Reservations": [{"Instances": [{
            "InstanceId": self.returned_id,
            "State": {"Name": self.state}, "LaunchTime": self.launch,
        }]}]}

    def stop_instances(self, **kwargs):
        assert kwargs == {"InstanceIds": [ENV["INSTANCE_ID"]]}
        self.stops.append(kwargs)
        if self.before_stop:
            self.before_stop()
        if self.stop_error:
            raise self.stop_error
        return {}


class FakeDDB:
    def __init__(self):
        self.items = {}
        self.read_error = None
        self.write_error = None
        self.after_commit_error = None
        self.before_commit = None
        self.transactions = []

    @staticmethod
    def key(item):
        return item["pk"]["S"], item["sk"]["S"]

    def get_item(self, **kwargs):
        assert kwargs["TableName"] == ENV["TABLE_NAME"]
        assert kwargs["ConsistentRead"] is True
        if self.read_error:
            raise self.read_error
        item = self.items.get(self.key(kwargs["Key"]))
        return {"Item": copy.deepcopy(item)} if item is not None else {}

    def transact_write_items(self, TransactItems):
        if self.before_commit:
            callback, self.before_commit = self.before_commit, None
            callback()
        if self.write_error:
            raise self.write_error
        assert len(TransactItems) <= 100
        candidate = copy.deepcopy(self.items)
        keys = set()
        for action in TransactItems:
            assert set(action) == {"Put"}
            put = action["Put"]
            assert put["TableName"] == ENV["TABLE_NAME"]
            key = self.key(put["Item"])
            assert key not in keys
            keys.add(key)
            if key[1] == "STATE":
                old = self.items.get(key)
                condition = put["ConditionExpression"]
                if condition == "attribute_not_exists(pk)":
                    if old is not None:
                        raise RuntimeError("conditional conflict")
                else:
                    assert condition == "#version = :version"
                    assert put["ExpressionAttributeNames"] == {"#version": "version"}
                    expected = put["ExpressionAttributeValues"][":version"]
                    if old is None or old["version"] != expected:
                        raise RuntimeError("conditional conflict")
            candidate[key] = copy.deepcopy(put["Item"])
        self.items = candidate
        self.transactions.append(copy.deepcopy(TransactItems))
        if self.after_commit_error:
            raise self.after_commit_error
        return {}

    def item(self, sk):
        return guard.decode(self.items[("INSTANCE#i-personal", sk)])


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.ec2, self.ddb = FakeEC2(), FakeDDB()

    def run_at(self, now=BASE, **limits):
        return guard.run_guard(self.ec2, self.ddb, {**ENV, **limits}, now)

    def expect_closed(self, error=Exception, now=BASE, **limits):
        with self.assertLogs(guard.LOG, level="ERROR"):
            with self.assertRaises(error):
                self.run_at(now, **limits)
        self.assertTrue(self.ec2.stops)
        self.assertTrue(all(s == {"InstanceIds": [ENV["INSTANCE_ID"]]}
                            for s in self.ec2.stops))

    def test_defaults_allow_initial_testing(self):
        config = guard.Config.from_env(ENV)
        self.assertEqual(config.budget_minutes, 3000)
        self.assertEqual(config.boot_seconds, 7200)
        result = self.run_at(BASE + timedelta(minutes=1))
        self.assertEqual(result["consumed_minutes"], 1)
        self.assertFalse(result["stop_requested"])

    def test_round_cumulative_seconds_up_to_minutes(self):
        self.run_at(BASE + timedelta(seconds=1))
        self.assertEqual(self.run_at(BASE + timedelta(seconds=62))["consumed_minutes"], 2)
        self.assertEqual(self.run_at(BASE + timedelta(seconds=62))["consumed_minutes"], 2)

    def test_repeated_60_point_1_second_jitter_does_not_double_charge(self):
        self.run_at()
        for count in range(1, 11):
            result = self.run_at(BASE + timedelta(milliseconds=60100 * count))
        self.assertEqual(self.ddb.item("MONTH#2026-09")["consumed_seconds"], 610)
        self.assertEqual(result["consumed_minutes"], 11)
        self.assertFalse(result["stop_requested"])

    def test_legacy_minute_counter_migrates_conservatively(self):
        self.run_at(BASE + timedelta(seconds=1))
        del self.ddb.items[("INSTANCE#i-personal", "MONTH#2026-09")]["consumed_seconds"]
        result = self.run_at(BASE + timedelta(seconds=2))
        self.assertEqual(result["consumed_minutes"], 2)
        self.assertEqual(self.ddb.item("MONTH#2026-09")["consumed_seconds"], 61)

    def test_boot_lease_exact_threshold_and_no_heartbeat_extension(self):
        self.run_at(BASE + timedelta(minutes=119, seconds=59))
        self.assertFalse(self.ec2.stops)
        result = self.run_at(BASE + timedelta(hours=2))
        self.assertEqual(result["reasons"], ["boot_lease"])
        self.assertTrue(result["stop_requested"])

    def test_one_minute_schedule_detects_lease_within_one_minute(self):
        # Deliberately offset the invocation phase from launch.
        self.run_at(BASE + timedelta(hours=1, minutes=59, seconds=37))
        self.assertFalse(self.ec2.stops)
        result = self.run_at(BASE + timedelta(hours=2, seconds=37))
        self.assertTrue(result["stop_requested"])

    def test_one_minute_schedule_detects_monthly_budget(self):
        self.run_at(BASE + timedelta(minutes=58), MONTHLY_HOURS="1")
        self.assertFalse(self.run_at(BASE + timedelta(minutes=59), MONTHLY_HOURS="1")["stop_requested"])
        self.assertTrue(self.run_at(BASE + timedelta(minutes=60), MONTHLY_HOURS="1")["stop_requested"])

    def test_first_observation_already_over_lease(self):
        result = self.run_at(BASE + timedelta(hours=3))
        self.assertEqual(result["consumed_minutes"], 180)
        self.assertIn("boot_lease", result["reasons"])

    def test_missed_invocations_charge_whole_gap(self):
        self.run_at()
        result = self.run_at(BASE + timedelta(hours=4, seconds=1))
        self.assertEqual(result["consumed_minutes"], 241)
        self.assertTrue(result["stop_requested"])

    def test_stopping_transition_is_charged_conservatively(self):
        self.run_at()
        self.ec2.state = "stopped"
        result = self.run_at(BASE + timedelta(minutes=10))
        self.assertEqual(result["consumed_minutes"], 10)
        self.assertEqual(self.run_at(BASE + timedelta(hours=1))["consumed_minutes"], 10)

    def test_stopped_enrollment_does_not_bill_old_idle_history(self):
        self.ec2.state = "stopped"
        self.ec2.launch = BASE - timedelta(days=90)
        self.assertEqual(self.run_at()["consumed_minutes"], 0)
        self.assertEqual(self.run_at(BASE + timedelta(days=1))["consumed_minutes"], 0)
        self.assertFalse(self.ec2.stops)

    def test_stopped_to_running_charges_gap_not_just_latest_launch(self):
        self.ec2.state = "stopped"
        self.run_at()
        self.ec2.state = "running"
        self.ec2.launch = BASE + timedelta(minutes=9)
        result = self.run_at(BASE + timedelta(minutes=10))
        self.assertEqual(result["consumed_minutes"], 10)
        self.assertFalse(result["stop_requested"])

    def test_hidden_start_stop_between_stopped_observations(self):
        self.ec2.state = "stopped"
        self.run_at()
        self.ec2.launch = BASE + timedelta(minutes=3)
        self.assertEqual(self.run_at(BASE + timedelta(minutes=10))["consumed_minutes"], 10)

    def test_request_latency_is_conservatively_included_on_transition(self):
        previous = {"ec2_state": "stopped", "launch_at": guard.stamp(BASE),
                    "observation_started_at": guard.stamp(BASE),
                    "checked_at": guard.stamp(BASE + timedelta(seconds=30))}
        # Start during the previous API request may follow its stopped snapshot.
        charges = guard.accounting(previous, "running", BASE + timedelta(seconds=10),
                                   BASE + timedelta(seconds=90))
        self.assertEqual(charges, {"2026-09": 90})

    def test_running_restart_resets_boot_lease_not_monthly_usage(self):
        self.run_at(BASE + timedelta(minutes=119))
        self.ec2.launch = BASE + timedelta(minutes=120)
        result = self.run_at(BASE + timedelta(minutes=121))
        self.assertEqual(result["consumed_minutes"], 121)
        self.assertFalse(result["stop_requested"])

    def test_depleted_budget_stops_repeated_starts(self):
        result = self.run_at(BASE + timedelta(hours=1), MONTHLY_HOURS="1")
        self.assertEqual(result["reasons"], ["monthly_budget"])
        for minute in (61, 62):
            self.ec2.launch = BASE + timedelta(minutes=minute)
            result = self.run_at(self.ec2.launch, MONTHLY_HOURS="1")
            self.assertTrue(result["stop_requested"])
            self.assertTrue(self.ddb.item("MONTH#2026-09")["depleted"])
        self.assertEqual(len(self.ec2.stops), 3)

    def test_month_boundary_splits_gap_and_does_not_reset_boot(self):
        self.ec2.launch = datetime(2026, 9, 30, 22, tzinfo=UTC)
        self.run_at(datetime(2026, 9, 30, 23, 59, 30, tzinfo=UTC))
        result = self.run_at(datetime(2026, 10, 1, 0, 0, 30, tzinfo=UTC))
        self.assertEqual(self.ddb.item("MONTH#2026-09")["consumed_minutes"], 120)
        self.assertEqual(self.ddb.item("MONTH#2026-10")["consumed_minutes"], 1)
        self.assertIn("boot_lease", result["reasons"])

    def test_new_month_budget_fresh_but_previous_retained(self):
        self.ec2.launch = datetime(2026, 9, 30, 23, tzinfo=UTC)
        self.run_at(datetime(2026, 9, 30, 23, 59, tzinfo=UTC), MONTHLY_HOURS="0.5")
        self.ec2.state = "stopped"
        self.run_at(datetime(2026, 9, 30, 23, 59, 30, tzinfo=UTC), MONTHLY_HOURS="0.5")
        result = self.run_at(datetime(2026, 10, 1, tzinfo=UTC), MONTHLY_HOURS="0.5")
        self.assertEqual(result["consumed_minutes"], 0)
        self.assertFalse(result["depleted"])
        self.assertTrue(self.ddb.item("MONTH#2026-09")["depleted"])

    def test_multiple_month_gap_and_year_rollover(self):
        start = datetime(2025, 12, 31, 23, 59, 30, tzinfo=UTC)
        end = datetime(2026, 2, 1, 0, 0, 1, tzinfo=UTC)
        self.ec2.launch = start
        self.run_at(start)
        self.run_at(end)
        for month, total in {"2025-12": 1, "2026-01": 31 * 1440, "2026-02": 1}.items():
            self.assertEqual(self.ddb.item("MONTH#" + month)["consumed_minutes"], total)

    def test_persist_before_stop(self):
        self.ec2.before_stop = lambda: self.assertTrue(
            self.ddb.item("MONTH#2026-09")["depleted"])
        self.run_at(BASE + timedelta(hours=1), MONTHLY_HOURS="1")

    def test_pending_and_stopping_are_not_free(self):
        for state in ("pending", "stopping"):
            with self.subTest(state=state):
                ec2 = FakeEC2(state=state)
                result = guard.run_guard(ec2, FakeDDB(), ENV, BASE + timedelta(hours=2))
                self.assertEqual(result["consumed_minutes"], 120)
                self.assertTrue(ec2.stops)

    def test_state_read_failure_stops_and_raises(self):
        self.ddb.read_error = OSError("DynamoDB unavailable")
        self.expect_closed(OSError)

    def test_month_read_failure_stops_and_raises(self):
        original = self.ddb.get_item
        def read(**kwargs):
            if kwargs["Key"]["sk"]["S"].startswith("MONTH#"):
                raise OSError("month read failed")
            return original(**kwargs)
        self.ddb.get_item = read
        self.expect_closed(OSError)

    def test_write_failure_keeps_state_and_counters_atomic(self):
        self.run_at()
        before = copy.deepcopy(self.ddb.items)
        self.ddb.write_error = OSError("transaction failed")
        self.expect_closed(OSError, BASE + timedelta(minutes=10))
        self.assertEqual(self.ddb.items, before)
        self.ddb.write_error = None
        self.assertEqual(self.run_at(BASE + timedelta(minutes=11))["consumed_minutes"], 11)

    def test_commit_then_network_failure_retry_does_not_lose_usage(self):
        self.ddb.after_commit_error = OSError("lost response")
        self.expect_closed(OSError, BASE + timedelta(minutes=10))
        self.ddb.after_commit_error = None
        self.assertEqual(self.run_at(BASE + timedelta(minutes=11))["consumed_minutes"], 11)

    def test_conditional_conflict_cannot_overwrite_newer_accounting(self):
        self.run_at()
        self.ddb.before_commit = lambda: self.run_at(BASE + timedelta(minutes=2))
        self.expect_closed(RuntimeError, BASE + timedelta(minutes=1))
        self.assertEqual(self.ddb.item("MONTH#2026-09")["consumed_minutes"], 2)
        self.assertEqual(self.ddb.item("STATE")["checked_at"], guard.stamp(BASE + timedelta(minutes=2)))

    def test_describe_error_or_wrong_instance_fails_closed(self):
        self.ec2.describe_error = OSError("EC2 read failed")
        self.expect_closed(OSError)
        self.ec2.describe_error = None
        self.ec2.returned_id = "i-not-ours"
        self.expect_closed(ValueError)

    def test_fail_closed_stop_error_preserves_original_error(self):
        self.ddb.read_error = OSError("original read failure")
        self.ec2.stop_error = RuntimeError("stop failed")
        with self.assertLogs(guard.LOG, level="ERROR"):
            with self.assertRaisesRegex(OSError, "original read failure"):
                self.run_at()
        self.assertEqual(len(self.ec2.stops), 1)

    def test_policy_stop_error_is_raised_after_persistence(self):
        self.ec2.stop_error = OSError("stop failed")
        self.expect_closed(OSError, BASE + timedelta(hours=2))
        self.assertEqual(self.ddb.item("MONTH#2026-09")["consumed_minutes"], 120)

    def test_corrupt_or_missing_month_counter_fails_closed(self):
        self.run_at()
        key = ("INSTANCE#i-personal", "MONTH#2026-09")
        self.ddb.items[key]["consumed_minutes"] = {"N": "-1"}
        self.expect_closed(ValueError, BASE + timedelta(minutes=1))
        del self.ddb.items[key]
        self.expect_closed(ValueError, BASE + timedelta(minutes=1))

    def test_bad_times_and_unknown_state_fail_closed(self):
        self.run_at()
        self.expect_closed(ValueError, BASE - timedelta(seconds=1))
        self.ec2.launch = BASE + timedelta(seconds=1)
        self.expect_closed(ValueError)
        self.ec2.launch = BASE.replace(tzinfo=None)
        self.expect_closed(ValueError)
        self.ec2.launch = BASE
        self.ec2.state = "unknown"
        self.expect_closed(ValueError)

    def test_invalid_limits_fail_closed(self):
        for name in ("MONTHLY_HOURS", "MAX_BOOT_HOURS"):
            for value in ("0", "-1", "NaN", "Infinity", "oops"):
                with self.subTest(name=name, value=value):
                    self.expect_closed(ValueError, **{name: value})

    def test_missing_table_config_fails_closed(self):
        with self.assertLogs(guard.LOG, level="ERROR"):
            with self.assertRaises(KeyError):
                guard.run_guard(self.ec2, self.ddb, {"INSTANCE_ID": ENV["INSTANCE_ID"]}, BASE)
        self.assertEqual(len(self.ec2.stops), 1)

    def test_fractional_month_limit_rounds_down_conservatively(self):
        self.assertEqual(guard.Config.from_env({**ENV, "MONTHLY_HOURS": "0.025"}).budget_minutes, 1)

    def test_pure_minute_rounding_leap_year_timezone_and_capacity(self):
        start = datetime(2024, 2, 29, 23, 59, 59, 999999, tzinfo=UTC)
        self.assertEqual(guard.charge_minutes(start, start), {})
        self.assertEqual(guard.charge_minutes(start, start + timedelta(microseconds=2)),
                         {"2024-02": 1, "2024-03": 1})
        self.assertEqual(guard.month_key(datetime(2026, 10, 1, 1, tzinfo=timezone(timedelta(hours=2)))), "2026-09")
        with self.assertRaises(ValueError):
            guard.charge_minutes(BASE, BASE + timedelta(days=4000))

    def test_import_without_site_packages_or_boto3(self):
        path = str(Path(guard.__file__).resolve().parent)
        code = f"import sys; sys.path.insert(0, {path!r}); import guard; assert 'boto3' not in sys.modules"
        subprocess.run([sys.executable, "-S", "-c", code], check=True, capture_output=True)

    def test_handler_uses_injected_boto3_and_ignores_event_target(self):
        self.assertIs(guard.handler, guard.lambda_handler)
        class SDK:
            def client(inner, name):
                return {"ec2": self.ec2, "dynamodb": self.ddb}[name]
        with patch.dict(sys.modules, {"boto3": SDK()}), patch.dict(os.environ, ENV):
            # Use a current launch so wall-clock handler test stays within lease.
            self.ec2.launch = datetime.now(UTC)
            result = guard.lambda_handler({"INSTANCE_ID": "i-other"}, None)
        self.assertEqual(result["instance_id"], ENV["INSTANCE_ID"])

    def test_handler_ddb_client_setup_error_fails_closed(self):
        class SDK:
            def client(inner, name):
                if name == "ec2":
                    return self.ec2
                raise OSError("SDK DDB setup failed")
        with patch.dict(sys.modules, {"boto3": SDK()}), patch.dict(os.environ, ENV):
            with self.assertLogs(guard.LOG, level="ERROR"):
                with self.assertRaises(OSError):
                    guard.lambda_handler({}, None)
        self.assertEqual(len(self.ec2.stops), 1)


if __name__ == "__main__":
    unittest.main()
