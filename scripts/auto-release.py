#!/usr/bin/env python3
"""Coalesce main commits into immutable Desktop beta releases, without building.

Requires a full git checkout and authenticated gh. The default is read-only.
GITHUB_TOKEN-created tags do not trigger push workflows, so dispatch explicitly
at the reserved tag. Never retry an observed build automatically.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

BUILD_WORKFLOWS = {
    "macOS beta": "macos-beta.yml",
    "Linux and Windows desktop release": "cross-platform-release.yml",
}
RELEASE_WORKFLOWS = set(BUILD_WORKFLOWS) | {
    "Publish public desktop downloads", "Recover Linux desktop release",
    "macOS public release acceptance",
}
AUTO_MARKER = "Jcode Desktop automatic release v1"
TAG_RE = re.compile(r"desktop-v(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?\Z")
INPUT_ROOTS = ("src", "crates", "assets", "packaging", ".cargo", ".github/workflows")
INPUT_FILES = (
    "Cargo.toml", "Cargo.lock", "build.rs", "scripts/package-linux.sh",
    "scripts/package-macos.sh", "scripts/package-windows.ps1",
    "scripts/fetch-sparkle.sh", "scripts/render-macos-plist.py",
    "scripts/verify-macos-package.sh", "scripts/verify-release-package.py",
    "scripts/prepare-public-release.py", "scripts/publish-public-release.py",
    "scripts/auto-release.py", "scripts/fast-linker", "scripts/rustc-wrapper",
)
QUIET_SECONDS = 30 * 60
MAX_BATCH_SECONDS = 2 * 60 * 60
DISPATCH_GRACE_SECONDS = 10 * 60


def git(*args):
    return subprocess.check_output(["git", *args]).decode().strip()


def version_key(tag):
    match = TAG_RE.fullmatch(tag)
    if not match:
        raise ValueError(f"Invalid release tag: {tag}")
    major, minor, patch, beta = match.groups()
    return (int(major), int(minor), int(patch), int(beta) if beta else float("inf"))


def next_tag(tag):
    major, minor, patch, beta = version_key(tag)
    if beta == float("inf"):
        return f"desktop-v{major}.{minor}.{patch + 1}-beta.1"
    return f"desktop-v{major}.{minor}.{patch}-beta.{beta + 1}"


def release_input(path):
    return path in INPUT_FILES or any(path.startswith(root + "/") for root in INPUT_ROOTS)


def fingerprint(ref):
    records = git("ls-tree", "-r", "-z", ref).split("\0")
    inputs = [record for record in records if record and release_input(record.split("\t", 1)[1])]
    if not inputs:
        raise ValueError(f"No release inputs at {ref}")
    return hashlib.sha256("\0".join(inputs).encode()).hexdigest()


def timestamp(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()


def decide(*, now, latest_tag, latest_at, automatic, runs, active, changed,
           first_change, last_change, interval):
    """Pure release policy. Return a decision without making external changes."""
    observed = {run["name"] for run in runs}
    missing = [file for name, file in BUILD_WORKFLOWS.items() if name not in observed]
    # Repair a crash between reserving a tag and dispatching its builds. Do not
    # duplicate a run that failed/cancelled, or race API eventual consistency.
    if automatic and missing:
        if any(run["status"] == "completed" and run["conclusion"] != "success" for run in runs):
            return {"action": "skip", "reason": "Partial release failed; manual recovery required"}
        if now - latest_at < DISPATCH_GRACE_SECONDS:
            return {"action": "skip", "reason": "Waiting for reserved tag dispatches to become visible"}
        if any(run["head_branch"] != latest_tag for run in active):
            return {"action": "skip", "reason": "Another release is active"}
        return {"action": "dispatch", "tag": latest_tag, "workflows": missing,
                "reason": "Resume missing dispatches for the reserved immutable tag"}
    if active:
        return {"action": "skip", "reason": "Release build, publication, or acceptance is active"}
    if not changed:
        return {"action": "skip", "reason": "Release inputs unchanged since last attempted tag"}
    if now - latest_at < interval:
        return {"action": "skip", "reason": "Release interval has not elapsed"}
    if now - last_change < QUIET_SECONDS and now - first_change < MAX_BATCH_SECONDS:
        return {"action": "skip", "reason": "Batching recent commits (30-minute quiet period, 2-hour maximum)"}
    return {"action": "release", "tag": next_tag(latest_tag),
            "workflows": list(BUILD_WORKFLOWS.values()), "reason": "Changed release inputs are ready"}


class GitHub:
    def __init__(self, repository):
        if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
            raise ValueError("Expected owner/repository")
        self.repository = repository

    def api(self, route, payload=None, pages=False):
        command = ["gh", "api", f"repos/{self.repository}/{route}"]
        if pages:
            command += ["--paginate", "--slurp"]
        if payload is not None:
            command += ["--method", "POST", "--input", "-"]
        result = subprocess.run(command, input=json.dumps(payload) if payload is not None else None,
                                text=True, capture_output=True, check=True)
        return json.loads(result.stdout) if result.stdout.strip() else None

    def tag_runs(self, tag):
        runs = []
        for workflow in BUILD_WORKFLOWS.values():
            pages = self.api(f"actions/workflows/{workflow}/runs?branch={tag}&per_page=100", pages=True)
            runs.extend(run for page in pages for run in page["workflow_runs"])
        return runs

    def active_runs(self):
        runs = []
        for status in ("in_progress", "queued", "waiting", "pending", "requested"):
            pages = self.api(f"actions/runs?status={status}&per_page=100", pages=True)
            runs.extend(run for page in pages for run in page["workflow_runs"]
                        if run["name"] in RELEASE_WORKFLOWS)
        return runs

    def reserve(self, tag, commit, now):
        obj = self.api("git/tags", {
            "tag": tag, "message": f"{AUTO_MARKER}\nSource: {commit}\n",
            "object": commit, "type": "commit",
            "tagger": {"name": "github-actions[bot]", "email": "41898282+github-actions[bot]@users.noreply.github.com",
                       "date": datetime.fromtimestamp(now, timezone.utc).isoformat()},
        })
        # Atomic creation, never update or force-move a tag. A competing manual
        # release causes a safe failure before any dispatch.
        self.api("git/refs", {"ref": f"refs/tags/{tag}", "sha": obj["sha"]})

    def dispatch(self, tag, workflow):
        self.api(f"actions/workflows/{workflow}/dispatches", {
            "ref": tag, "inputs": {"version": tag.removeprefix("desktop-v")},
        })


def validate_runtime_pins(ref):
    pins = []
    for workflow in (*BUILD_WORKFLOWS.values(), "freebsd-release.yml"):
        text = git("show", f"{ref}:.github/workflows/{workflow}")
        match = re.search(r"repository: 1jehuang/jcode\s+ref: ([0-9a-f]{40})\b", text)
        if not match:
            raise ValueError(f"Missing immutable Jcode runtime pin in {workflow}")
        pins.append(match[1])
    if len(set(pins)) != 1:
        raise ValueError("Build workflows must pin the same Jcode runtime commit")


def execute(github, decision, commit, now):
    if decision["action"] == "release":
        github.reserve(decision["tag"], commit, now)
    if decision["action"] in ("release", "dispatch"):
        for workflow in decision["workflows"]:
            github.dispatch(decision["tag"], workflow)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "1jehuang/jcode-desktop"))
    parser.add_argument("--ref", default="HEAD", help="Freshly fetched main commit to consider")
    parser.add_argument("--apply", action="store_true", help="Reserve tag and dispatch builds (default: dry run)")
    parser.add_argument("--interval-hours", type=float, default=6)
    args = parser.parse_args()
    if not 1 <= args.interval_hours <= 168:
        parser.error("interval must be between 1 and 168 hours")
    github = GitHub(args.repo)
    now = time.time()
    commit = git("rev-parse", f"{args.ref}^{{commit}}")
    tags = [tag for tag in git("tag", "--list", "desktop-v*").splitlines() if TAG_RE.fullmatch(tag)]
    if not tags:
        raise ValueError("Bootstrap the first release manually before enabling automatic releases")
    latest_tag = max(tags, key=version_key)
    # Full history is mandatory. Refuse divergence instead of releasing an old
    # branch or inventing a new baseline after a history rewrite.
    subprocess.run(["git", "merge-base", "--is-ancestor", latest_tag, commit], check=True)
    automatic = git("for-each-ref", "--format=%(contents)", f"refs/tags/{latest_tag}").startswith(AUTO_MARKER + "\n")
    latest_at = int(git("for-each-ref", "--format=%(creatordate:unix)", f"refs/tags/{latest_tag}"))
    runs = github.tag_runs(latest_tag)
    latest_at = max([latest_at] + [timestamp(run["created_at"]) for run in runs])
    times = [int(value) for value in git("log", "--format=%ct", f"{latest_tag}..{commit}", "--",
                                        *INPUT_ROOTS, *INPUT_FILES).splitlines()]
    decision = decide(now=now, latest_tag=latest_tag, latest_at=latest_at, automatic=automatic,
                      runs=runs, active=github.active_runs(),
                      changed=fingerprint(latest_tag) != fingerprint(commit),
                      first_change=min(times, default=now), last_change=max(times, default=now),
                      interval=args.interval_hours * 3600)
    if decision["action"] in ("release", "dispatch"):
        validate_runtime_pins(commit if decision["action"] == "release" else decision["tag"])
    report = {**decision, "commit": commit, "baseline": latest_tag, "dry_run": not args.apply}
    print(json.dumps(report, indent=2), flush=True)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as summary:
            summary.write("## Automatic desktop release\n```json\n" + json.dumps(report, indent=2) + "\n```\n")
    if args.apply:
        execute(github, decision, commit, now)


if __name__ == "__main__":
    main()
