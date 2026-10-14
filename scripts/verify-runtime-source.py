#!/usr/bin/env python3
"""Fail fast on Desktop/runtime source incompatibility, without building or networking.

This checks dependency manifests, requested features, and known API boundaries.
It does not replace the native compilation, package, or smoke-test release gates.
Use --revision to inspect committed Git objects without reading dirty runtime work.
"""
import argparse
from pathlib import Path
import re
import subprocess
import tomllib


API_REQUIREMENTS = {
    "crates/jcode-harness-api/src/events.rs": (r"pub\s+user_connected\s*:\s*bool",),
    "crates/jcode-sdk/src/client.rs": (
        r"pub\s+supports_user_response\s*:\s*bool",
        r"pub\s+fn\s+runtime_command\s*\(",
    ),
    "crates/jcode-sdk/src/ssh.rs": (r"pub\s+supports_user_response\s*:\s*bool",),
    "crates/jcode-sdk/src/collections.rs": (r"pub\s+fn\s+create_with_id\s*\(",),
    "crates/jcode-harness-api/src/runtime_commands.rs": (r"pub\s+enum\s+RuntimeCommand\b",),
    "crates/jcode-voice/src/lib.rs": (
        r"pub\s+mod\s+dictation\s*;",
        r"pub\s+mod\s+global\s*;",
        r"pub\s+use\s+streaming_capture\s*::",
    ),
}


def dependency_tables(value):
    for key, table in value.items():
        if not isinstance(table, dict):
            continue
        if key in ("dependencies", "dev-dependencies", "build-dependencies"):
            yield table
        else:
            yield from dependency_tables(table)


def runtime_dependencies(desktop):
    """Derive requirements from every Desktop manifest, including target tables."""
    desktop = Path(desktop).resolve()
    nominal_runtime = desktop.parent / "jcode"
    requirements = []
    for manifest in [desktop / "Cargo.toml", *sorted((desktop / "crates").rglob("Cargo.toml"))]:
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for table in dependency_tables(data):
            for alias, dependency in table.items():
                if not isinstance(dependency, dict) or "path" not in dependency:
                    continue
                path = (manifest.parent / dependency["path"]).resolve()
                if not path.is_relative_to(nominal_runtime):
                    continue
                requirements.append(((path.relative_to(nominal_runtime) / "Cargo.toml").as_posix(),
                                     dependency.get("package", alias),
                                     dependency.get("features", [])))
    if not requirements:
        raise ValueError("No Jcode path dependencies found in Desktop manifests")
    return requirements


def runtime_reader(runtime, revision=None):
    runtime = Path(runtime).resolve()
    if revision is not None and not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("Runtime revision must be an immutable 40-character commit")

    def read(path):
        try:
            if revision is not None:
                return subprocess.check_output(["git", "-C", str(runtime), "show",
                                                f"{revision}:{path}"], stderr=subprocess.PIPE).decode("utf-8")
            return (runtime / path).read_text(encoding="utf-8")
        except (OSError, subprocess.CalledProcessError) as error:
            raise ValueError(f"Missing required runtime source: {path}") from error
    return read


def verify(desktop, read):
    dependencies = runtime_dependencies(desktop)
    manifests = {}
    for path, package, features in dependencies:
        if path not in manifests:
            manifests[path] = tomllib.loads(read(path))
        manifest = manifests[path]
        actual = manifest.get("package", {}).get("name")
        if actual != package:
            raise ValueError(f"Runtime package mismatch in {path}: expected {package}, got {actual}")
        provided = set(manifest.get("features", {}))
        for table in dependency_tables(manifest):
            provided.update(name for name, dep in table.items()
                            if isinstance(dep, dict) and dep.get("optional"))
        for feature in features:
            if feature not in provided:
                raise ValueError(f"Missing runtime feature {package}/{feature} in {path}")
    # Packaging also builds the bundled CLI and bridge from the runtime workspace.
    read("Cargo.toml")
    read("crates/jcode-build-meta/Cargo.toml")
    for path, patterns in API_REQUIREMENTS.items():
        source = read(path)
        for pattern in patterns:
            if not re.search(pattern, source):
                raise ValueError(f"Missing required runtime API in {path}: {pattern}")
    return sorted(manifests)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--desktop", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--revision", help="Inspect exact committed objects, ignoring dirty work")
    parser.add_argument("--expected-commit", help="Require a clean checkout at this immutable commit")
    args = parser.parse_args(argv)
    try:
        if args.expected_commit:
            if not re.fullmatch(r"[0-9a-f]{40}", args.expected_commit):
                raise ValueError("Expected runtime commit must be immutable")
            head = subprocess.check_output(["git", "-C", str(args.runtime), "rev-parse", "HEAD"], text=True).strip()
            if head != args.expected_commit:
                raise ValueError(f"Runtime checkout mismatch: expected {args.expected_commit}, got {head}")
            subprocess.run(["git", "-C", str(args.runtime), "diff", "--exit-code", "HEAD", "--"], check=True)
            if args.revision and args.revision != args.expected_commit:
                raise ValueError("Revision and expected checkout commit must agree")
        manifests = verify(args.desktop, runtime_reader(args.runtime, args.revision))
    except (ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Runtime source compatibility failed: {error}\n")
    print(f"Runtime source preflight passed: {len(manifests)} dependency manifests and known Desktop APIs")
    for path in manifests:
        print(f"  {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
