#!/usr/bin/env python3
"""Fail-closed, read-only original native gates for desktop-v0.2.1 recovery.

Default is one observation (exit 2 if pending). --timeout-minutes 180 polls
fresh GitHub JSON for at most three hours. Exit 0 authorizes only these original
native gates, NOT recovery success, asset integrity, or release publication.
The caller must separately require the successful FreeBSD recovery dependency.
"""
import argparse
import json
import math
import subprocess
import sys
import time

REPOSITORY = "1jehuang/jcode-desktop"
SHA = "4c5495f85a03671b20a00a108f10e6dbe041584d"
TAG = "desktop-v0.2.1"
MAC_RUN = 35504110073
CROSS_RUN = 35504110181
ATTEMPT = 1
FREEBSD_JOB_ID = 106060890220
FREEBSD_JOB = "build-freebsd / Build and smoke-test FreeBSD x86_64"
MATRIX_JOBS = (
    "build (ubuntu-24.04, linux, x86_64, x86_64-unknown-linux-gnu, SHA256SUMS-linux)",
    "build (ubuntu-24.04-arm, linux, aarch64, aarch64-unknown-linux-gnu, SHA256SUMS-linux-aarch64)",
    "build (windows-2025, windows, x86_64, x86_64-pc-windows-msvc, SHA256SUMS-windows)",
    "build (windows-11-arm, windows, aarch64, aarch64-pc-windows-msvc, SHA256SUMS-windows-aarch64)",
)
WORKFLOWS = {
    MAC_RUN: ".github/workflows/macos-beta.yml",
    CROSS_RUN: ".github/workflows/cross-platform-release.yml",
}
PENDING = {"queued", "in_progress", "waiting", "pending", "requested"}


class GateError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise GateError(message)


def validate_run(run, run_id):
    require(isinstance(run, dict), "run must be an object")
    expected = {
        "id": run_id, "run_attempt": ATTEMPT, "head_sha": SHA,
        "head_branch": TAG, "path": WORKFLOWS[run_id],
    }
    for key, value in expected.items():
        require(type(run.get(key)) is type(value) and run.get(key) == value,
                f"run {run_id}: incorrect {key}")
    for key in ("repository", "head_repository"):
        require(isinstance(run.get(key), dict) and
                run[key].get("full_name") == REPOSITORY,
                f"run {run_id}: incorrect {key}")
    require(run.get("event") in {"push", "workflow_dispatch"},
            f"run {run_id}: untrusted event")
    state = run.get("status")
    require(state in PENDING | {"completed"}, f"run {run_id}: unknown status")
    if state != "completed":
        require(run.get("conclusion") is None, f"run {run_id}: premature conclusion")
        return False
    conclusion = "success" if run_id == MAC_RUN else "failure"
    require(run.get("conclusion") == conclusion,
            f"run {run_id}: expected final conclusion {conclusion}")
    return True


def validate_jobs(pages, run, run_id):
    require(isinstance(pages, list) and bool(pages), "missing jobs pages")
    jobs = []
    total = None
    for page in pages:
        require(isinstance(page, dict) and isinstance(page.get("jobs"), list),
                "malformed jobs page")
        count = page.get("total_count")
        require(type(count) is int and count >= 0, "invalid jobs total")
        if total is None:
            total = count
        require(count == total, "jobs changed during pagination")
        jobs.extend(page["jobs"])
    require(len(jobs) == total, "incomplete jobs pagination")
    expected = {"build": "success"} if run_id == MAC_RUN else {
        "prepare": "success", **{name: "success" for name in MATRIX_JOBS},
        FREEBSD_JOB: "failure", "publish": "skipped",
    }
    seen, ids = set(), set()
    ready = True
    for job in jobs:
        require(isinstance(job, dict), "malformed job")
        name = job.get("name")
        require(isinstance(name, str) and name in expected, "unexpected job name")
        require(name not in seen, f"duplicate job: {name}")
        seen.add(name)
        job_id = job.get("id")
        require(type(job_id) is int and job_id > 0 and job_id not in ids,
                f"invalid/duplicate job ID: {name}")
        ids.add(job_id)
        require(type(job.get("run_id")) is int and job.get("run_id") == run_id
                and type(job.get("run_attempt")) is int and job.get("run_attempt") == ATTEMPT
                and job.get("head_sha") == SHA and job.get("head_branch") == TAG,
                f"job identity mismatch: {name}")
        if name == FREEBSD_JOB:
            require(job_id == FREEBSD_JOB_ID, "not the original known FreeBSD failure")
        state = job.get("status")
        require(state in PENDING | {"completed"}, f"unknown job status: {name}")
        if state == "completed":
            require(job.get("conclusion") == expected[name],
                    f"unexpected job conclusion: {name}: {job.get('conclusion')}")
        else:
            require(job.get("conclusion") is None, f"premature job conclusion: {name}")
            ready = False
    if run["status"] == "completed":
        require(seen == set(expected), f"completed run missing jobs: {sorted(set(expected) - seen)}")
        require(ready, "completed run contains unfinished jobs")
    return ready and seen == set(expected)


def gh_json(endpoint, deadline=None, paginate=False):
    remaining = 60 if deadline is None else min(60, deadline - time.monotonic())
    require(remaining > 0, "native gate polling deadline exceeded")
    command = ["gh", "api", "--hostname", "github.com", endpoint]
    if paginate:
        command += ["--paginate", "--slurp"]
    result = subprocess.run(command, check=True, capture_output=True, text=True,
                            timeout=remaining)
    return json.loads(result.stdout)


def observe(deadline=None):
    """Inspect BOTH runs even when one is pending, so other failures fail now."""
    ready = True
    summaries = []
    for run_id in (MAC_RUN, CROSS_RUN):
        endpoint = f"repos/{REPOSITORY}/actions/runs/{run_id}"
        before = gh_json(endpoint, deadline)
        run_ready = validate_run(before, run_id)
        pages = gh_json(f"{endpoint}/attempts/{ATTEMPT}/jobs?per_page=100",
                        deadline, paginate=True)
        jobs_ready = validate_jobs(pages, before, run_id)
        # Detect reruns or changes while jobs were fetched. Never combine attempts.
        after = gh_json(endpoint, deadline)
        after_ready = validate_run(after, run_id)
        unchanged = all(before.get(key) == after.get(key)
                        for key in ("status", "conclusion", "run_attempt", "head_sha"))
        if after_ready and unchanged:
            validate_jobs(pages, after, run_id)
        ready = ready and run_ready and jobs_ready and after_ready and unchanged
        summaries.append({"run_id": run_id, "attempt": ATTEMPT,
                          "status": after["status"], "conclusion": after["conclusion"]})
    print(json.dumps({"repository": REPOSITORY, "sha": SHA, "tag": TAG,
                      "ready": ready, "runs": summaries}), flush=True)
    return ready


def bounded_number(value, maximum, allow_zero=True):
    number = float(value)
    if not math.isfinite(number) or number > maximum or number < 0 or (not allow_zero and number == 0):
        raise argparse.ArgumentTypeError(f"must be {'0..' if allow_zero else '>0 and <='}{maximum}")
    return number


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout-minutes", type=lambda s: bounded_number(s, 180), default=0)
    parser.add_argument("--poll-seconds", type=lambda s: bounded_number(s, 300, False), default=30)
    args = parser.parse_args(argv)
    deadline = time.monotonic() + args.timeout_minutes * 60 if args.timeout_minutes else None
    try:
        while True:
            if observe(deadline):
                print("Original native gates passed. Recovery and complete-asset gates remain required.")
                return 0
            if deadline is None:
                print("Native gates pending", file=sys.stderr)
                return 2
            remaining = deadline - time.monotonic()
            require(remaining > 0, "native gate polling deadline exceeded")
            time.sleep(min(args.poll_seconds, remaining))
    except (GateError, OSError, subprocess.SubprocessError, json.JSONDecodeError,
            TypeError, KeyError) as error:
        print(f"Native gate refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
