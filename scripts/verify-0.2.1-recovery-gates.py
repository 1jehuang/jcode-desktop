#!/usr/bin/env python3
"""Read-only combined native evidence for the 0.2.1 recovery publication.

Both recovery workflows MUST succeed completely. The original strict native
verifier is intentionally unchanged. This gate never publishes anything.
Default: one observation, exit 2 pending. Poll with --timeout-minutes 180.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import time

_spec = importlib.util.spec_from_file_location(
    "original_native_gate", Path(__file__).with_name("verify-0.2.1-native-gates.py"))
BASE = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(BASE)
require, GateError = BASE.require, BASE.GateError
REPOSITORY, SOURCE_SHA, TAG = BASE.REPOSITORY, BASE.SHA, BASE.TAG
ATTEMPT = 1
FREEBSD_RUN = 35508120526
FREEBSD_SHA = "1121a4e998f86c699ea5273b32f9b66b57359039"
RECOVERY_BRANCH = "release/desktop-0.2.0"
WINDOWS_RUN = 35505943861
WINDOWS_SHA = "7461bb2ce1faeba5e5891881d90b3cc5160e1b1a"
WINDOWS_JOBS = (
    "recover-windows (windows-2025, x86_64, X64, x86_64-pc-windows-msvc, SHA256SUMS-windows)",
    # GitHub truncates the actual API job name. Pair it with the exact job ID.
    "recover-windows (windows-11-arm, aarch64, Arm64, aarch64-pc-windows-msvc, SHA256SUMS-windows-aarc...",
)
WINDOWS_JOB_IDS = dict(zip(WINDOWS_JOBS, (106065666657, 106065667634)))
ORIGINAL_WINDOWS_IDS = {BASE.MATRIX_JOBS[2]: 106060890244, BASE.MATRIX_JOBS[3]: 106060890160}
CHECKOUT = "actions/checkout@11d5960a326750d5838078e36cf38b85af677262"
CACHE = "Swatinem/rust-cache@49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c"
ORIGINAL_WINDOWS_STEPS = {
    1: "Set up job", 2: f"Run {CHECKOUT}", 3: f"Run {CHECKOUT}",
    4: "Validate release target and publication contracts", 5: "Configure MSVC build environment",
    6: "Run dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c", 7: f"Run {CACHE}",
    8: "Install Linux build dependencies", 9: "Build and package Linux", 10: "Verify Linux package",
    11: "Preserve packaged Linux artifacts before smoke", 12: "Smoke-test Linux package on X11",
    13: "Smoke-test Linux package on native Wayland", 14: "Print Linux smoke diagnostics",
    15: "Upload Linux smoke diagnostics", 16: "Build and package Windows",
    17: "Verify Windows package", 18: "Smoke-test Windows package",
    19: "Run actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02",
    20: "Upload verified platform packages to private draft",
    38: f"Post Run {CACHE}", 39: f"Post Run {CHECKOUT}", 40: f"Post Run {CHECKOUT}",
}
WINDOWS_REQUIRED = (
    "Require unpublished stable draft", "Verify immutable sources",
    "Validate release target and publication contracts", "Require native Windows runner",
    "Configure MSVC build environment", "Test Windows transport after MSVC environment setup",
    "Build and package Windows", "Verify Windows package", "Smoke-test Windows package",
    "Verify sources remain unchanged", "Upload only native-verified missing Windows assets",
)
FREEBSD_REQUIRED = (
    "Require unpublished stable draft", "Verify immutable source and recovery tooling",
    "Test recovery smoke fixture", "Build, package, and smoke-test inside FreeBSD",
    "Upload only native-verified missing FreeBSD assets",
)
LINUX_REQUIRED = ("Build and package Linux", "Verify Linux package",
                  "Smoke-test Linux package on X11", "Smoke-test Linux package on native Wayland",
                  "Upload verified platform packages to private draft")
MAC_REQUIRED = ("Validate release credentials", "Build universal app, DMG, and ZIP",
                "Verify app bundle and DMG install path", "Sign automatic update and generate appcast",
                "Upload verified macOS packages to private draft")


def contracts():
    require(type(WINDOWS_RUN) is int and WINDOWS_RUN > 0 and
            isinstance(WINDOWS_SHA, str) and len(WINDOWS_SHA) == 40 and
            all(c in "0123456789abcdef" for c in WINDOWS_SHA),
            "Windows recovery run identity has not been reviewed and pinned")
    require(len({BASE.MAC_RUN, BASE.CROSS_RUN, FREEBSD_RUN, WINDOWS_RUN}) == 4,
            "recovery run identities must be distinct")
    return (
        (BASE.MAC_RUN, SOURCE_SHA, TAG, "macos-beta.yml", "success", {"build": ("success",)}),
        (BASE.CROSS_RUN, SOURCE_SHA, TAG, "cross-platform-release.yml", "failure", {
            "prepare": ("success",), **{name: ("success",) for name in BASE.MATRIX_JOBS[:2]},
            **{name: ("success", "failure") for name in ORIGINAL_WINDOWS_IDS},
            BASE.FREEBSD_JOB: ("failure",), "publish": ("skipped",),
        }),
        (FREEBSD_RUN, FREEBSD_SHA, RECOVERY_BRANCH, "freebsd-0.2.1-recovery.yml", "success",
         {"recover-freebsd": ("success",)}),
        (WINDOWS_RUN, WINDOWS_SHA, RECOVERY_BRANCH, "windows-0.2.1-recovery.yml", "success",
         {name: ("success",) for name in WINDOWS_JOBS}),
    )


def validate_run(run, contract):
    run_id, sha, branch, workflow, conclusion, _ = contract
    require(isinstance(run, dict), "malformed run object")
    for key, value in {"id": run_id, "run_attempt": ATTEMPT, "head_sha": sha,
                       "head_branch": branch, "path": f".github/workflows/{workflow}"}.items():
        require(type(run.get(key)) is type(value) and run.get(key) == value,
                f"run {run_id}: incorrect {key}")
    for key in ("repository", "head_repository"):
        require(isinstance(run.get(key), dict) and run[key].get("full_name") == REPOSITORY,
                f"run {run_id}: incorrect {key}")
    events = {"workflow_dispatch"} if run_id in {FREEBSD_RUN, WINDOWS_RUN} else {"push", "workflow_dispatch"}
    require(run.get("event") in events, f"run {run_id}: incorrect event")
    require(run.get("status") in BASE.PENDING | {"completed"}, f"run {run_id}: unknown status")
    if run["status"] == "completed":
        require(run.get("conclusion") == conclusion, f"run {run_id}: unexpected conclusion")
        return True
    require(run.get("conclusion") is None, f"run {run_id}: premature conclusion")
    return False


def step_rows(job):
    steps = job.get("steps")
    require(isinstance(steps, list) and steps, f"missing steps: {job['name']}")
    numbers = set()
    for step in steps:
        require(isinstance(step, dict), "malformed step")
        number = step.get("number")
        require(type(number) is int and number > 0 and number not in numbers, "duplicate/invalid step number")
        numbers.add(number)
        require(isinstance(step.get("name"), str), "malformed step name")
        require(step.get("status") in BASE.PENDING | {"completed"}, "unknown step status")
    return steps


def required_success_steps(job, required):
    steps = step_rows(job)
    for name in required:
        matches = [s for s in steps if s["name"] == name]
        require(len(matches) == 1, f"missing/duplicate required step: {name}")
        step = matches[0]
        require(step.get("status") == "completed" and step.get("conclusion") == "success",
                f"required step did not succeed: {name}")


def original_windows_steps(job):
    """Only the pinned transport step may fail, never build/verify/native smoke."""
    steps = step_rows(job)
    seen = set()
    for step in steps:
        number, name = step["number"], step["name"]
        if name == "Complete job":
            require(number > 40 and "complete" not in seen, "unexpected completion step")
            seen.add("complete")
            expected = "success"
        else:
            require(ORIGINAL_WINDOWS_STEPS.get(number) == name,
                    f"unexpected original Windows step: {number}: {name}")
            seen.add(number)
            expected = "skipped" if number in {*range(8, 16), 19} else "success"
            if number == 20:
                expected = job["conclusion"]
        require(step["status"] == "completed" and step.get("conclusion") == expected,
                f"original Windows failure is not transport-only: {number}: {name}")
    require(set(ORIGINAL_WINDOWS_STEPS) <= seen, "incomplete original Windows step evidence")


def validate_jobs(pages, run, contract):
    run_id, sha, branch, _, _, expected = contract
    require(isinstance(pages, list) and pages, "missing job pages")
    jobs, total = [], None
    for page in pages:
        require(isinstance(page, dict) and isinstance(page.get("jobs"), list), "malformed jobs page")
        count = page.get("total_count")
        require(type(count) is int and count >= 0, "invalid job total")
        total = count if total is None else total
        require(total == count, "job count changed during pagination")
        jobs.extend(page["jobs"])
    require(len(jobs) == total, "incomplete jobs pagination")
    seen, ids, ready = set(), set(), True
    for job in jobs:
        require(isinstance(job, dict), "malformed job")
        name = job.get("name")
        require(isinstance(name, str) and name in expected and name not in seen,
                "unknown or duplicate job")
        seen.add(name)
        job_id = job.get("id")
        require(type(job_id) is int and job_id > 0 and job_id not in ids, "invalid/duplicate job ID")
        ids.add(job_id)
        require(type(job.get("run_id")) is int and job["run_id"] == run_id and
                type(job.get("run_attempt")) is int and job["run_attempt"] == ATTEMPT and
                job.get("head_sha") == sha and job.get("head_branch") == branch,
                f"job identity mismatch: {name}")
        if run_id == WINDOWS_RUN:
            require(job_id == WINDOWS_JOB_IDS[name], "not the pinned Windows recovery job")
        if run_id == BASE.CROSS_RUN:
            pinned_id = ORIGINAL_WINDOWS_IDS.get(name)
            if name == BASE.FREEBSD_JOB:
                pinned_id = BASE.FREEBSD_JOB_ID
            if pinned_id:
                require(job_id == pinned_id, "not the exact original failure job")
        state = job.get("status")
        require(state in BASE.PENDING | {"completed"}, f"unknown job status: {name}")
        if state != "completed":
            require(job.get("conclusion") is None, f"premature job conclusion: {name}")
            ready = False
            continue
        require(job.get("conclusion") in expected[name], f"unexpected job conclusion: {name}")
        if run_id == BASE.CROSS_RUN and name in ORIGINAL_WINDOWS_IDS:
            original_windows_steps(job)
        elif run_id == BASE.CROSS_RUN and name in BASE.MATRIX_JOBS[:2]:
            required_success_steps(job, LINUX_REQUIRED)
        elif run_id == BASE.MAC_RUN:
            required_success_steps(job, MAC_REQUIRED)
        elif run_id == FREEBSD_RUN:
            required_success_steps(job, FREEBSD_REQUIRED)
        elif run_id == WINDOWS_RUN:
            required_success_steps(job, WINDOWS_REQUIRED)
    if run["status"] == "completed":
        require(seen == set(expected), f"completed run {run_id} missing expected jobs")
        require(ready, "completed run contains pending jobs")
    return ready and seen == set(expected), jobs


def observe(deadline=None):
    ready, evidence = True, []
    for contract in contracts():
        run_id = contract[0]
        endpoint = f"repos/{REPOSITORY}/actions/runs/{run_id}"
        before = BASE.gh_json(endpoint, deadline)
        before_ready = validate_run(before, contract)
        pages = BASE.gh_json(f"{endpoint}/attempts/{ATTEMPT}/jobs?per_page=100", deadline, paginate=True)
        jobs_ready, jobs = validate_jobs(pages, before, contract)
        after = BASE.gh_json(endpoint, deadline)
        after_ready = validate_run(after, contract)
        stable = all(before.get(k) == after.get(k) for k in ("status", "conclusion", "run_attempt"))
        ready = ready and before_ready and jobs_ready and after_ready and stable
        evidence.append({"run_id": run_id, "attempt": ATTEMPT, "head_sha": contract[1],
                         "status": after["status"], "conclusion": after.get("conclusion"),
                         "jobs": [{"id": j["id"], "name": j["name"], "status": j["status"],
                                   "conclusion": j.get("conclusion"), "steps": j.get("steps", [])}
                                  for j in jobs]})
    print(json.dumps({"ready": ready, "source_sha": SOURCE_SHA, "tag": TAG, "runs": evidence}), flush=True)
    return ready


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout-minutes", type=lambda v: BASE.bounded_number(v, 180), default=0)
    parser.add_argument("--poll-seconds", type=lambda v: BASE.bounded_number(v, 300, False), default=30)
    args = parser.parse_args(argv)
    deadline = time.monotonic() + args.timeout_minutes * 60 if args.timeout_minutes else None
    try:
        while True:
            if observe(deadline):
                print("Combined native recovery gates passed. Complete-asset and publication gates remain required.")
                return 0
            if deadline is None:
                print("Combined native recovery gates pending", file=sys.stderr)
                return 2
            remaining = deadline - time.monotonic()
            require(remaining > 0, "combined gate polling deadline exceeded")
            time.sleep(min(args.poll_seconds, remaining))
    except (GateError, OSError, subprocess.SubprocessError, json.JSONDecodeError, TypeError, KeyError) as error:
        print(f"Combined gate refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
