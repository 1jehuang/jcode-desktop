#!/usr/bin/env python3
"""Release an explicit Desktop version and wait through public acceptance.

Read-only by default. --apply authorizes immutable tagging and missing build
workflow dispatches, not retries, tag moves, bypasses, or release-asset edits.
"""
import argparse
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release_policy", ROOT / "scripts/auto-release.py")
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)
spec = importlib.util.spec_from_file_location("prepare_release", ROOT / "scripts/prepare-public-release.py")
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)
spec = importlib.util.spec_from_file_location("release_notes", ROOT / "scripts/release_notes.py")
notes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notes)
BUILD = tuple(policy.BUILD_WORKFLOWS.values())
DOWNSTREAM = {
    "publish-public-release.yml": "Publish",
    "macos-public-release-verify.yml": "Accept",
    "discord-release.yml": "Announce",
}
MANIFESTS = ("Cargo.toml", "crates/jcode-desktop-api/Cargo.toml",
             "crates/jcode-desktop-motion/Cargo.toml", "crates/jcode-desktop-ui/Cargo.toml")


def validate_versions(sha, version):
    for path in MANIFESTS:
        actual = tomllib.loads(policy.git("show", f"{sha}:{path}"))["package"]["version"]
        if actual != version:
            raise ValueError(f"{path} version {actual} does not match requested {version}")


def verify_website(tag):
    prerelease = "-beta." in tag
    # Stable completion means the actual current download channel advanced.
    # Betas intentionally do not displace an existing stable channel.
    url = (f"{prepare.PUBLIC_BASE}/{tag}/latest.json" if prerelease
           else "https://jcode.sh/desktop/latest.json")
    request = urllib.request.Request(url, headers={"User-Agent": "JcodeReleaseOrchestrator/1.0",
                                                   "Cache-Control": "no-cache"})
    with urllib.request.urlopen(request, timeout=30) as response:
        payload = response.read(2 * 1024 * 1024 + 1)
    if len(payload) > 2 * 1024 * 1024:
        raise ValueError("Website release manifest exceeds size limit")
    manifest = json.loads(payload)
    if (manifest.get("tag_name") != tag or manifest.get("draft") is not False
            or manifest.get("prerelease") is not prerelease):
        raise ValueError(f"Website channel has not promoted the requested release: {tag}")
    checksums = prepare.expected_assets(tag)
    expected = set(checksums) | {"appcast.xml"}
    expected.update(name for names in checksums.values() for name in names)
    assets = manifest.get("assets", [])
    names = [a.get("name") for a in assets]
    if len(names) != len(expected) or set(names) != expected:
        raise ValueError("Website release asset set is incomplete, duplicated or unexpected")
    for asset in assets:
        if (type(asset.get("size")) is not int or asset["size"] <= 0
                or not re.fullmatch(r"[a-f0-9]{64}", asset.get("sha256", ""))
                or asset.get("browser_download_url") != f"{prepare.PUBLIC_BASE}/{tag}/{asset['name']}"):
            raise ValueError(f"Invalid website release asset: {asset.get('name')}")


def allowed_versions(tags):
    """Next legal versions: one SemVer step past the latest stable, or its next beta.

    See docs/release-orchestration.md "Choosing the version" for which step to use.
    """
    keys = {policy.version_key(t) for t in tags if policy.TAG_RE.fullmatch(t)}
    stable = max((k[:3] for k in keys if k[3] == float("inf")), default=None)
    if stable is None:
        return None
    major, minor, patch = stable
    allowed = set()
    for base in ((major, minor, patch + 1), (major, minor + 1, 0), (major + 1, 0, 0)):
        text = ".".join(map(str, base))
        allowed.add(text)
        betas = [k[3] for k in keys if k[:3] == base and k[3] != float("inf")]
        if (*base, float("inf")) not in keys:
            allowed.add(f"{text}-beta.{max(betas, default=0) + 1}")
    return {v for v in allowed if policy.version_key("desktop-v" + v) not in keys}


def remote_tags():
    """Read-only, current view of release tags on origin."""
    output = policy.git("ls-remote", "--tags", "--refs", "origin", "desktop-v*")
    return [line.split("refs/tags/", 1)[1] for line in output.splitlines() if "refs/tags/" in line]


def validate_next_version(sha, version, tags):
    allowed = allowed_versions(tags)
    if allowed is not None and version not in allowed:
        raise ValueError(f"{version} is not a valid next Desktop version. Choose one of: "
                         + ", ".join(sorted(allowed, key=lambda v: policy.version_key('desktop-v' + v)))
                         + ". See docs/release-orchestration.md (Choosing the version).")
    if "-beta." not in version:
        changelog = policy.git("show", f"{sha}:CHANGELOG.md")
        if not notes.changelog_section(changelog, version):
            raise ValueError(f"CHANGELOG.md has no '### Jcode Desktop {version}' section with bullets. "
                             "Stable releases need curated notes for GitHub, Discord and the updates panel.")


def tag_for(version):
    tag = "desktop-v" + version
    if not policy.TAG_RE.fullmatch(tag):
        raise ValueError("Expected VERSION such as 0.3.0 or 0.3.0-beta.1")
    return tag


def matching_run(runs, tag, sha, title=None, not_before=None):
    """Select newest matching attempt, never fall back to an older success."""
    matches = [r for r in runs if r["event"] in ("push", "workflow_dispatch", "workflow_run")
               and ((r["head_branch"] == tag and r["head_sha"] == sha) if title is None
                    else r.get("display_title") == f"{title} {tag}")
               and (not not_before or (r.get("run_started_at") or r.get("created_at", "")) >= not_before)]
    return max(matches, key=lambda r: (r["id"], r.get("run_attempt", 1)), default=None)


def run_state(run):
    if run is None:
        return "missing"
    if run["status"] != "completed":
        return "pending"
    if run["conclusion"] != "success":
        raise RuntimeError(f"Release gate failed: {run.get('html_url', run['id'])} ({run['conclusion']}). "
                           "Inspect and explicitly rerun the failed gate, then resume this command.")
    return "success"


def read_runs(github, workflow, tag=None):
    route = f"actions/workflows/{workflow}/runs?per_page=100"
    if tag:
        route += f"&branch={tag}"
    return [r for page in github.api(route, pages=True) for r in page["workflow_runs"]]


def resolve_tag(github, tag):
    # Listing refs avoids treating arbitrary authentication/network errors as 404.
    refs = github.api(f"git/matching-refs/tags/{tag}")
    exact = [r for r in refs if r["ref"] == f"refs/tags/{tag}"]
    if not exact:
        return None
    obj = exact[0]["object"]
    for _ in range(8):
        if obj["type"] == "commit":
            return obj["sha"]
        if obj["type"] != "tag":
            break
        obj = github.api(f"git/tags/{obj['sha']}")["object"]
    raise ValueError("Release tag does not resolve to a commit")


@contextmanager
def dispatch_journal(repository, tag, sha):
    """POSIX advisory lock and durable dispatch intent outside the worktree."""
    import fcntl
    directory = Path(policy.git("rev-parse", "--git-common-dir")) / "desktop-releases"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    key = hashlib.sha256(f"{repository}/{tag}/{sha}".encode()).hexdigest()
    path = directory / f"{key}.json"
    with (directory / f"{key}.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError("Another local orchestrator owns this release") from error
        state = json.loads(path.read_text()) if path.exists() else []
        if not isinstance(state, list) or any(w not in BUILD for w in state):
            raise ValueError(f"Invalid dispatch journal: {path}")
        dispatched = set(state)
        def record(workflow):
            dispatched.add(workflow)
            temporary = path.with_suffix(".pending")
            with temporary.open("w") as output:
                json.dump(sorted(dispatched), output)
                output.flush()
                os.fsync(output.fileno())
            temporary.replace(path)
        yield dispatched, record


def monitor(github, tag, sha, timeout, poll, grace=90, dispatched=None, record=None):
    deadline = time.monotonic() + timeout
    dispatch_after = time.monotonic() + grace
    dispatched = set() if dispatched is None else dispatched
    last = None
    while time.monotonic() < deadline:
        states = {}
        build_starts = []
        for workflow in BUILD:
            run = matching_run(read_runs(github, workflow, tag), tag, sha)
            states[workflow] = run_state(run)
            if run:
                build_starts.append(run.get("run_started_at") or run.get("created_at", ""))
            if run is None and workflow not in dispatched and time.monotonic() >= dispatch_after:
                # A PAT-created tag may trigger push builds. Discover those first.
                # Record locally before dispatch. Ambiguous API failures abort,
                # never blindly repeat a request within this invocation.
                dispatched.add(workflow)
                if record:
                    record(workflow)
                github.dispatch(tag, workflow)
                states[workflow] = "dispatched"
        if all(states[w] == "success" for w in BUILD):
            not_before = max(build_starts, default="")
            for workflow, title in DOWNSTREAM.items():
                run = matching_run(read_runs(github, workflow), tag, sha, title, not_before)
                states[workflow] = run_state(run)
                if workflow == "publish-public-release.yml":
                    if states[workflow] != "success":
                        break
                    not_before = run.get("run_started_at") or run.get("created_at", not_before)
        if states != last:
            print(json.dumps({"tag": tag, "gates": states}), flush=True)
            last = states
        if len(states) == len(BUILD) + len(DOWNSTREAM) and all(s == "success" for s in states.values()):
            return
        time.sleep(poll)
    raise TimeoutError("Timed out waiting for release gates. No runs were cancelled. Resume the same command.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("--repo", default="1jehuang/jcode-desktop")
    parser.add_argument("--ref", default="origin/main", help="Source commit for new tags. Existing tags resume their immutable source.")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--timeout-minutes", type=int, default=240)
    parser.add_argument("--poll-seconds", type=int, default=20)
    args = parser.parse_args(argv)
    if args.timeout_minutes < 1 or args.poll_seconds < 1:
        parser.error("Timeout and polling interval must be positive")
    tag = tag_for(args.version)
    github = policy.GitHub(args.repo)
    existing = resolve_tag(github, tag)
    sha = existing or policy.git("rev-parse", f"{args.ref}^{{commit}}")
    policy.validate_runtime_pins(sha)
    if not existing:
        validate_versions(sha, args.version)
        validate_next_version(sha, args.version, remote_tags())
        remote = github.api("commits/main")["sha"]
        if sha != remote:
            raise ValueError("New release must match current remote main. Fetch and review main first.")
        active = github.active_runs()
        if active:
            raise ValueError("Another release is active. Wait rather than racing publication.")
    print(json.dumps({"tag": tag, "sha": sha, "resume": bool(existing), "dry_run": not args.apply}), flush=True)
    if not args.apply:
        return 0
    if os.name != "posix":
        raise ValueError("Run release orchestration on Linux or macOS (POSIX locking required)")
    if not existing:
        if policy.git("status", "--porcelain"):
            raise ValueError("Refusing new release from a dirty checkout")
        if policy.git("rev-parse", "HEAD") != sha:
            raise ValueError("Check out the reviewed release source before running preflight tests")
        subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"], cwd=ROOT, check=True)
        subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "scripts",
                        "-p", "test_prepare_freebsd_gpui.py", "-v"], cwd=ROOT, check=True)
        # Lightweight tag creation is atomic. Existing refs cause a safe error.
        # Both GITHUB_TOKEN and PAT are supported by monitor's discovery window.
        github.api("git/refs", {"ref": f"refs/tags/{tag}", "sha": sha})
    with dispatch_journal(args.repo, tag, sha) as (dispatched, record):
        monitor(github, tag, sha, args.timeout_minutes * 60, args.poll_seconds,
                dispatched=dispatched, record=record)
    verify_website(tag)
    print(f"{tag}: builds, public downloads, macOS acceptance, and announcement all succeeded.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
