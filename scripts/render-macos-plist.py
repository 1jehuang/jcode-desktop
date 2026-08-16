#!/usr/bin/env python3
"""Render and validate release-sensitive macOS bundle metadata."""

from __future__ import annotations

import argparse
import base64
import binascii
import plistlib
import re
from pathlib import Path
from urllib.parse import urlsplit

SEMVER = re.compile(
    r"^(?:desktop-v)?(?P<core>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\."
    r"(?P<patch>0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)


def marketing_version(version: str) -> str:
    match = SEMVER.fullmatch(version)
    if match is None:
        raise ValueError(f"version is not valid SemVer: {version!r}")
    return ".".join((match.group("core"), match.group("minor"), match.group("patch")))


def validate_build(build: str) -> str:
    if re.fullmatch(r"[1-9]\d*", build) is None:
        raise ValueError("build number must be a positive integer")
    return build


def validate_public_key(public_key: str) -> str:
    try:
        decoded = base64.b64decode(public_key, validate=True)
    except (binascii.Error, ValueError) as error:
        raise ValueError("Sparkle public key must be valid base64") from error
    if len(decoded) != 32:
        raise ValueError("Sparkle public key must decode to a 32-byte Ed25519 key")
    return public_key


def validate_feed_url(feed_url: str) -> str:
    parsed = urlsplit(feed_url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise ValueError("update feed must be an HTTPS URL without embedded credentials")
    return feed_url


def render(args: argparse.Namespace) -> None:
    with args.template.open("rb") as source:
        info = plistlib.load(source)

    info["CFBundleShortVersionString"] = marketing_version(args.version)
    info["CFBundleVersion"] = validate_build(args.build)

    have_update_config = bool(args.public_key or args.feed_url)
    if have_update_config and not (args.public_key and args.feed_url):
        raise ValueError("Sparkle public key and feed URL must be supplied together")
    if args.require_updates and not have_update_config:
        raise ValueError("secure update configuration is required for this build")

    if have_update_config:
        info["SUPublicEDKey"] = validate_public_key(args.public_key)
        info["SUFeedURL"] = validate_feed_url(args.feed_url)
        info["SUEnableAutomaticChecks"] = True
        info["SUAutomaticallyUpdate"] = True
        info["SUScheduledCheckInterval"] = 86400
    else:
        for key in (
            "SUPublicEDKey",
            "SUFeedURL",
            "SUEnableAutomaticChecks",
            "SUAutomaticallyUpdate",
            "SUScheduledCheckInterval",
        ):
            info.pop(key, None)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("wb") as destination:
        plistlib.dump(info, destination, sort_keys=False)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--template", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build", required=True)
    parser.add_argument("--public-key", default="")
    parser.add_argument("--feed-url", default="")
    parser.add_argument("--require-updates", action="store_true")
    return parser.parse_args()


if __name__ == "__main__":
    try:
        render(parse_args())
    except ValueError as error:
        raise SystemExit(f"error: {error}") from error
