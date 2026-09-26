#!/usr/bin/env python3
"""Announce verified public Desktop downloads, recording dedupe on the source release.

Only the public publisher dispatches this workflow automatically, after website
hash verification. Manual retries repeat anonymous manifest and byte checks.
A webhook success followed by marker failure requires operator reconciliation:
GitHub and Discord cannot provide an atomic exactly-once transaction.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import urllib.parse
import urllib.request

SOURCE_REPOSITORY = "1jehuang/jcode-desktop"
PUBLIC_REPOSITORY = "1jehuang/jcode-desktop-releases"
WEBSITE = "https://jcode.sh/desktop"
DISCORD_LIMIT = 2000
MARKER_PREFIX = "jcode-desktop-discord-announced"
SPEC = importlib.util.spec_from_file_location("prepare_discord", Path(__file__).with_name("prepare-public-release.py"))
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)
NOTES_SPEC = importlib.util.spec_from_file_location("discord_release_notes", Path(__file__).with_name("release_notes.py"))
NOTES = importlib.util.module_from_spec(NOTES_SPEC)
NOTES_SPEC.loader.exec_module(NOTES)


def announcement_marker(tag):
    return f"<!-- {MARKER_PREFIX}:{tag} -->"


def already_announced(body, tag):
    return announcement_marker(tag) in body


def release_notes_for_discord(body):
    body = re.sub(r"<!--.*?-->", "", body, flags=re.DOTALL)
    # Release bodies contain the complete changelog, including headings,
    # pending-publication instructions and the previous update. Only current
    # feature bullets belong in an announcement whose title/links we generate.
    lines = []
    in_bullet = False
    for line in body.splitlines():
        if re.match(r"#{1,6}\s+(?:Previous (?:releases?|updates?)|Older releases|History)\b", line, re.I):
            break
        if re.match(r"^\s*[-*+]\s+", line):
            lines.append(line)
            in_bullet = True
        elif in_bullet and line.startswith(("  ", "\t")) and line.strip():
            lines.append(line)
        else:
            in_bullet = False
    return "\n".join(lines).strip().replace("@", "@\u200b")


def sanitize(text):
    return re.sub(r"<!--.*?-->", "", text, flags=re.DOTALL).replace("@", "@\u200b").strip()


def format_message(*, tag, body, url, notes=None):
    """Discord post for a tag. `notes` come from release_notes.notes_for_tag.

    Curated CHANGELOG.md notes (or commit subjects) are preferred. The source
    release body is only a fallback, since build workflows create it with a
    fixed placeholder rather than the changelog.
    """
    title = f"## Jcode Desktop {tag.removeprefix('desktop-v')}"
    links = f"\n\nDownload: <{WEBSITE}>\nPublic binaries: <{url}>"
    budget = DISCORD_LIMIT - len(title) - len(links) - 1
    text = ""
    if notes and notes.get("sections"):
        # Sanitize before measuring so mention escapes cannot overflow the limit.
        sanitized = {"headline": sanitize(notes.get("headline", "")),
                     "sections": [(name, [sanitize(i) for i in items]) for name, items in notes["sections"]]}
        text = NOTES.to_markdown(sanitized, limit=budget)
    if not text:
        text = release_notes_for_discord(body) or "New Desktop release available."
    if len(text) > budget:
        text = text[:budget - 1].rstrip() + "…"
    return f"{title}\n{text}{links}"


def request_json(url, *, token=None, method="GET", payload=None):
    headers = {"User-Agent": "jcode-desktop-release-bot/1.0", "Accept": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if payload is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, method=method, headers=headers,
                                     data=None if payload is None else json.dumps(payload).encode())
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def fetch_release(*, repository, tag, token=None):
    return request_json(f"https://api.github.com/repos/{repository}/releases/tags/{urllib.parse.quote(tag, safe='')}", token=token)


def validate_release(release, repository, tag):
    if (release.get("tag_name") != tag or release.get("draft") is not False
            or not release.get("published_at") or type(release.get("id")) is not int
            or release.get("html_url") != f"https://github.com/{repository}/releases/tag/{tag}"
            or release.get("prerelease") is not ("-beta." in tag)):
        raise ValueError("Release identity or publication metadata mismatch")


def validate_manifest(manifest, tag):
    checksums = PREPARE.expected_assets(tag)
    names = set(checksums) | {"appcast.xml"}
    for packages in checksums.values():
        names.update(packages)
    assets = manifest.get("assets", [])
    if (manifest.get("tag_name") != tag or manifest.get("draft") is not False
            or manifest.get("prerelease") is not ("-beta." in tag)
            or manifest.get("name") != f"Jcode Desktop {tag.removeprefix('desktop-v')}"
            or len(assets) != len(names) or {a.get("name") for a in assets} != names):
        raise ValueError("Invalid public manifest metadata")
    for asset in assets:
        if (asset.get("browser_download_url") != f"{WEBSITE}/releases/{tag}/{asset['name']}"
                or type(asset.get("size")) is not int or asset["size"] <= 0
                or not re.fullmatch(r"[0-9a-f]{64}", asset.get("sha256", ""))):
            raise ValueError("Invalid public asset metadata")


def verify_website_asset(asset):
    digest, size = hashlib.sha256(), 0
    request = urllib.request.Request(asset["browser_download_url"], headers={"User-Agent": "JcodeReleaseVerifier/1.0"})
    with urllib.request.urlopen(request, timeout=120) as response:
        while chunk := response.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    if digest.hexdigest() != asset["sha256"] or size != asset["size"]:
        raise ValueError("Website asset integrity mismatch")


def verify_publication(tag):
    public = fetch_release(repository=PUBLIC_REPOSITORY, tag=tag)
    validate_release(public, PUBLIC_REPOSITORY, tag)
    live = request_json(f"{WEBSITE}/latest.json")
    live_tag = live.get("tag_name", "")
    validate_manifest(live, live_tag)
    if live_tag != tag:
        if "-beta." in tag and "-beta." not in live_tag:
            stable = fetch_release(repository=PUBLIC_REPOSITORY, tag=live_tag)
            validate_release(stable, PUBLIC_REPOSITORY, live_tag)
            print("Stable Desktop release has superseded this beta; skipping")
            return None
        raise ValueError("Website does not advertise the requested release")
    immutable = request_json(f"https://github.com/{PUBLIC_REPOSITORY}/releases/download/{tag}/latest.json")
    if live != immutable:
        raise ValueError("Live and immutable public manifests differ")
    public_names = {a["name"] for a in public.get("assets", []) if a.get("state") == "uploaded"}
    if not ({a["name"] for a in live["assets"]} | {"latest.json"}) <= public_names:
        raise ValueError("Public release is incomplete")
    for asset in live["assets"]:
        verify_website_asset(asset)
    # Fail closed if promotion changed while the package verification ran.
    if request_json(f"{WEBSITE}/latest.json") != live:
        raise ValueError("Website changed during verification")
    return public


def post_to_discord(*, webhook_url, content):
    try:
        parts = urllib.parse.urlsplit(webhook_url)
        if parts.scheme != "https" or parts.hostname not in {"discord.com", "discordapp.com"} or not parts.path.startswith("/api/webhooks/"):
            raise ValueError("Invalid webhook")
        query = [(k, v) for k, v in urllib.parse.parse_qsl(parts.query) if k != "wait"] + [("wait", "true")]
        url = urllib.parse.urlunsplit((parts.scheme, parts.netloc, parts.path, urllib.parse.urlencode(query), ""))
        result = request_json(url, method="POST", payload={"content": content, "allowed_mentions": {"parse": []}})
        if not result.get("id"):
            raise ValueError("Missing Discord acknowledgement")
        return result
    except Exception:
        # Transport exception strings and HTTP response bodies can contain the
        # secret webhook URL. Never expose them, including via chained traceback.
        raise RuntimeError("Discord webhook failed (details suppressed)") from None


def mark_release_announced(*, repository, release, tag, token):
    validate_release(release, repository, tag)
    body = release.get("body") or ""
    if already_announced(body, tag):
        return
    request_json(f"https://api.github.com/repos/{repository}/releases/{release['id']}", token=token,
                 method="PATCH", payload={"body": body + f"\n\n{announcement_marker(tag)}\n"})


def announce_release(*, repository, tag, token, webhook_url):
    PREPARE.expected_assets(tag)
    if repository != SOURCE_REPOSITORY:
        raise ValueError("Unexpected source repository")
    source = fetch_release(repository=repository, tag=tag, token=token)
    validate_release(source, repository, tag)
    if already_announced(source.get("body") or "", tag):
        print("Desktop Discord announcement already recorded; skipping")
        return None
    public = verify_publication(tag)
    if public is None:
        return None
    notes = NOTES.notes_for_tag(tag)
    if notes["source"] == "none":
        print("::warning::No CHANGELOG.md section or commits found for this tag; falling back to the release body.")
    message = post_to_discord(webhook_url=webhook_url, content=format_message(
        tag=tag, body=source.get("body") or "", url=public["html_url"], notes=notes))
    try:
        latest = fetch_release(repository=repository, tag=tag, token=token)
        if latest["id"] != source["id"]:
            raise ValueError("Source release replaced")
        mark_release_announced(repository=repository, release=latest, tag=tag, token=token)
    except Exception:
        print("::warning::Discord post succeeded but dedupe marker failed. Do not rerun until the source marker is reconciled manually.", file=sys.stderr)
    return str(message["id"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"))
    args = parser.parse_args()
    try:
        token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        webhook = os.environ.get("DISCORD_RELEASE_WEBHOOK")
        if not token or not webhook:
            raise ValueError("Missing announcement credentials")
        message_id = announce_release(repository=args.repository, tag=args.tag, token=token, webhook_url=webhook)
        if message_id is not None:
            # IDs are Discord snowflakes. Never echo arbitrary response text.
            if not re.fullmatch(r"[0-9]+", message_id):
                print("::warning::Discord post succeeded but returned a nonstandard message ID; details suppressed.")
            else:
                print(f"Posted {args.tag} to Discord as message {message_id}")
        return 0
    except Exception:
        print("::error::Desktop announcement failed. Check release identity, public/live metadata, hashes, and configured credentials. Error details suppressed to protect secrets.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
