#!/usr/bin/env python3
"""Jcode Cloud planning model, not a billing or provisioning implementation.

Offline: python3 scripts/cloud_economics.py [--test]
Read-only price refresh:
  python3 scripts/cloud_economics.py --refresh-prices --output /path/to/prices.json
Pinned USD prices fetched 2026-09-18 using jcode-personal / us-east-1.
"""

import argparse
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from decimal import Decimal as D
import json
from pathlib import Path
import subprocess
import unittest


@dataclass(frozen=True)
class Instance:
    hourly: D
    vcpus: int
    memory_gib: int
    arch: str = "x86_64"
    baseline: D | None = None


INSTANCES = {
    "t3.medium": Instance(D(".0416"), 2, 4, baseline=D(".2")),
    "t3.large": Instance(D(".0832"), 2, 8, baseline=D(".3")),
    "t3a.medium": Instance(D(".0376"), 2, 4, baseline=D(".2")),
    "t3a.large": Instance(D(".0752"), 2, 8, baseline=D(".3")),
    "m7a.medium": Instance(D(".05796"), 1, 4),
    "m7i.large": Instance(D(".1008"), 2, 8),
    "c7i.large": Instance(D(".08925"), 2, 4),
    "m7g.medium": Instance(D(".0408"), 1, 4, "arm64"),
    "m7g.large": Instance(D(".0816"), 2, 8, "arm64"),
    "c7g.medium": Instance(D(".0363"), 1, 2, "arm64"),
    "c7g.large": Instance(D(".0725"), 2, 4, "arm64"),
}
GP3 = D(".08")
SNAPSHOT = D(".05")
EGRESS = D(".09")
IPV4 = D(".005")
CPU_SURPLUS = D(".05")
NAT_HOURLY = D(".045")
NAT_GB = D(".045")


@dataclass(frozen=True)
class Plan:
    instance: str = "m7i.large"
    hours: D = D("50")
    disk_gb: D = D("30")
    snapshot_gb: D = D("30")  # Aggregate retained billed blocks, not snapshot count.
    egress_gb: D = D("5")
    price: D = D("20")
    shared: D = D("1")
    support: D = D("2")
    reserve: D = D("1")
    model_budget: D = D("0")
    payment_percent: D = D(".029")  # Assumption, not a verified processor quote.
    payment_fixed: D = D(".30")

    def __post_init__(self):
        if self.instance not in INSTANCES:
            raise ValueError("unknown instance")
        for value in vars(self).values():
            if isinstance(value, D) and (not value.is_finite() or value < 0):
                raise ValueError("costs and allowances must be finite and nonnegative")
        if self.price == 0:
            raise ValueError("subscription revenue must be positive")


def compute_rate(instance, utilization=D("1"), unlimited=True):
    """Full-load steady state with zero starting credits, not a per-second AWS bill.

    Standard mode has no surplus charge but is throttled after credits run out.
    Idle/stopped time is never assumed to subsidize this worst-case calculation.
    """
    if not D("0") <= utilization <= D("1"):
        raise ValueError("utilization must be between zero and one")
    spec = INSTANCES[instance]
    surcharge = D("0")
    if spec.baseline is not None and unlimited:
        surcharge = max(D("0"), utilization - spec.baseline) * spec.vcpus * CPU_SURPLUS
    return spec.hourly + surcharge


def costs(plan=Plan(), *, unlimited=True):
    return {
        "compute": plan.hours * compute_rate(plan.instance, unlimited=unlimited),
        "ephemeral_ipv4": plan.hours * IPV4,
        "gp3": plan.disk_gb * GP3,
        "snapshots": plan.snapshot_gb * SNAPSHOT,
        "egress": plan.egress_gb * EGRESS,
        "shared_services": plan.shared,
        "payment": plan.price * plan.payment_percent + plan.payment_fixed,
        "support": plan.support,
        "risk_reserve": plan.reserve,
        "included_model_budget": plan.model_budget,
    }


def total(plan=Plan(), **kwargs):
    return sum(costs(plan, **kwargs).values(), D("0"))


def max_hours(plan=Plan(), target_margin=D(".25")):
    if not D("0") <= target_margin < D("1"):
        raise ValueError("margin must be between zero and one")
    fixed = total(replace(plan, hours=D("0")))
    return max(D("0"), (plan.price * (1 - target_margin) - fixed)
               / (compute_rate(plan.instance) + IPV4))


def nat_increment(subscribers, processed_gb, month_hours=D("730")):
    """Shared single NAT + its IPv4, before subtracting per-host IPv4 savings.

    processed_gb includes both download and upload bytes, not only internet egress.
    """
    if subscribers <= 0 or processed_gb < 0 or month_hours < 0:
        raise ValueError("positive subscriber count and nonnegative usage required")
    return month_hours * (NAT_HOURLY + IPV4) / subscribers + processed_gb * NAT_GB


def refresh_prices(output):
    queries = {
        "compute": ("AmazonEC2", [("TERM_MATCH", "operatingSystem", "Linux"),
            ("TERM_MATCH", "tenancy", "Shared"), ("TERM_MATCH", "preInstalledSw", "NA"),
            ("TERM_MATCH", "capacitystatus", "Used"),
            ("ANY_OF", "instanceType", ",".join(INSTANCES))]),
        "gp3": ("AmazonEC2", [("TERM_MATCH", "volumeApiName", "gp3")]),
        "snapshot": ("AmazonEC2", [("TERM_MATCH", "usagetype", "EBS:SnapshotUsage")]),
        "cpu": ("AmazonEC2", [("CONTAINS", "usagetype", "CPUCredits")]),
        "nat": ("AmazonEC2", [("CONTAINS", "usagetype", "NatGateway")]),
        "ipv4": ("AmazonVPC", [("CONTAINS", "usagetype", "PublicIPv4")]),
        "egress": ("AWSDataTransfer", [("TERM_MATCH", "fromLocation", "US East (N. Virginia)"),
            ("TERM_MATCH", "toLocation", "External")]),
    }
    result = {"fetched_at": datetime.now(timezone.utc).isoformat(),
              "profile": "jcode-personal", "region": "us-east-1", "queries": {}}
    for name, (service, filters) in queries.items():
        if name != "egress":
            filters = [("TERM_MATCH", "location", "US East (N. Virginia)")] + filters
        filters = [dict(Type=t, Field=k, Value=v) for t, k, v in filters]
        response = subprocess.run([
            "aws", "--profile", "jcode-personal", "--region", "us-east-1",
            "pricing", "get-products", "--service-code", service,
            "--filters", json.dumps(filters), "--output", "json", "--no-cli-pager",
        ], capture_output=True, text=True, check=True, timeout=60)
        products = [json.loads(p) for p in json.loads(response.stdout)["PriceList"]]
        if not products:
            raise RuntimeError(f"No price products returned for {name}")
        result["queries"][name] = {"service": service, "filters": filters, "products": products}
    # Deliberately does not update pinned prices or any subscription automatically.
    Path(output).write_text(json.dumps(result, indent=2) + "\n")
    print(f"Saved official price evidence to {output}. Review before changing pinned rates.")


class EconomicsTests(unittest.TestCase):
    def test_primary_full_allowance(self):
        self.assertEqual(total(), D("14.5200"))
        self.assertEqual((D("20") - total()) / 20, D(".274"))

    def test_stopped_storage_is_not_free(self):
        c = costs(replace(Plan(), hours=D("0")))
        self.assertEqual(c["compute"] + c["ephemeral_ipv4"], 0)
        self.assertEqual(c["gp3"] + c["snapshots"], D("3.90"))

    def test_t3a_large_full_load(self):
        self.assertEqual(compute_rate("t3a.large"), D(".1452"))
        self.assertEqual(total(replace(Plan(), instance="t3a.large")), D("16.7400"))

    def test_t3a_medium_full_load(self):
        self.assertEqual(compute_rate("t3a.medium"), D(".1176"))

    def test_standard_and_baseline_no_surcharge(self):
        self.assertEqual(compute_rate("t3a.large", unlimited=False), D(".0752"))
        self.assertEqual(compute_rate("t3a.large", utilization=D(".3")), D(".0752"))
        self.assertEqual(compute_rate("t3a.large", utilization=D("0")), D(".0752"))

    def test_nonburstable_rate_independent_of_cpu(self):
        for u in (D("0"), D(".5"), D("1")):
            self.assertEqual(compute_rate("m7i.large", utilization=u), D(".1008"))

    def test_model_budget_is_real_cost(self):
        self.assertEqual(total(replace(Plan(), model_budget=D("2"))) - total(), D("2"))

    def test_margin_ceiling(self):
        ceiling = max_hours()
        self.assertGreater(ceiling, D("54.53"))
        self.assertLess(ceiling, D("54.54"))
        self.assertEqual(total(replace(Plan(), hours=ceiling)), D("15"))
        self.assertLess(max_hours(replace(Plan(), model_budget=D("2"))), D("36"))

    def test_nat_small_scale(self):
        self.assertEqual(nat_increment(1, D("10")), D("36.95"))
        self.assertEqual(nat_increment(100, D("10")), D(".815"))

    def test_no_free_tier_or_promotional_credit(self):
        self.assertEqual(costs()["egress"], D(".45"))
        self.assertEqual(costs()["compute"], D("5.0400"))

    def test_invalid_inputs(self):
        for amount in (D("-1"), D("NaN"), D("Infinity")):
            with self.assertRaises(ValueError):
                Plan(hours=amount)
        with self.assertRaises(ValueError):
            Plan(price=D("0"))
        with self.assertRaises(ValueError):
            compute_rate("m7i.large", utilization=D("1.1"))
        with self.assertRaises(ValueError):
            nat_increment(0, D("10"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true")
    parser.add_argument("--refresh-prices", action="store_true")
    parser.add_argument("--output", help="explicit output path for fetched official evidence")
    args = parser.parse_args()
    if args.test:
        unittest.main(argv=[__file__], exit=True)
    elif args.refresh_prices:
        if not args.output:
            parser.error("--refresh-prices requires --output")
        refresh_prices(args.output)
    else:
        print("Pinned prices: 2026-09-18, USD, us-east-1, Linux shared on-demand")
        print("Full 50h allowance, full CPU, no starting credits or discounts:")
        print("instance       base/h  fullCPU/h  compute50h  all-in   margin")
        for name, spec in INSTANCES.items():
            plan = replace(Plan(), instance=name)
            c = total(plan)
            print(f"{name:14} {spec.hourly:.5f}  {compute_rate(name):.5f}"
                  f"    {costs(plan)['compute']:7.3f}   {c:6.3f}   {(20-c)/20:.2%}")
        print("\nPrimary itemization:")
        for key, value in costs().items():
            print(f"  {key}: ${value:.4f}")
        print(f"25% margin hour ceiling: {max_hours():.4f}")
        for hours in (D("20"), D("50"), D("100"), D("730"), D("744")):
            c = total(replace(Plan(), hours=hours))
            print(f"Primary {hours}h, other quotas full: ${c:.4f}, margin {(20-c)/20:.2%}")


if __name__ == "__main__":
    main()
