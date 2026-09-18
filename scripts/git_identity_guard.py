#!/usr/bin/env python3
"""Reject invented Jcode Git identities without impersonating other contributors.

Install: git config --local core.hooksPath .githooks
prepare-commit-msg also runs with commit --no-verify. pre-push checks actual
commit objects, covering commit-tree, imported commits, and author overrides.
"""

import re
import subprocess
import sys


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def check(identity, context):
    match = re.fullmatch(r"(.*?) <([^<>]+)>(?: .*?)?", identity)
    if not match:
        raise ValueError(f"Cannot validate {context}: {identity!r}")
    name, email = match.groups()
    if name.strip().casefold() == "jcode" or email.split("@", 1)[0].casefold() == "jcode":
        raise ValueError(
            f"Refusing {context}: {identity}. Use the user's configured Git "
            "identity, not an invented agent identity. Remove user.name/user.email "
            "command overrides and GIT_AUTHOR_*/GIT_COMMITTER_* overrides."
        )


def check_push(lines):
    checked = set()
    for line in lines:
        local_ref, local_oid, remote_ref, remote_oid = line.split()
        if set(local_oid) == {"0"}:
            continue  # Ref deletion introduces no commits.
        # Tags may point to trees/blobs rather than commits.
        kind = git("cat-file", "-t", f"{local_oid}^{{}}")
        if kind != "commit":
            continue
        revisions = [local_oid]
        if set(remote_oid) != {"0"}:
            # A remote tip need not exist locally. In that case scan all history
            # reachable from the pushed tip rather than failing open.
            known = subprocess.run(
                ["git", "cat-file", "-e", f"{remote_oid}^{{commit}}"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            ).returncode == 0
            if known:
                revisions.append(f"^{remote_oid}")
        for oid in git("rev-list", *revisions).splitlines():
            if oid in checked:
                continue
            checked.add(oid)
            author, committer = git("show", "-s", "--format=%an <%ae>%n%cn <%ce>", oid).splitlines()
            check(author, f"author of {oid[:12]} pushed to {remote_ref}")
            check(committer, f"committer of {oid[:12]} pushed to {remote_ref}")


def main():
    try:
        if sys.argv[1:] == ["commit"]:
            check(git("var", "GIT_AUTHOR_IDENT"), "commit author")
            check(git("var", "GIT_COMMITTER_IDENT"), "commit committer")
        elif sys.argv[1:] == ["push"]:
            check_push(sys.stdin)
        else:
            raise ValueError("Usage: git_identity_guard.py {commit|push}")
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"Git identity guard: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
