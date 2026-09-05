#!/usr/bin/env python3
"""Publish validated binaries to the public distribution repository using gh.

Run prepare-public-release.py first. GH_TOKEN must be able to write releases in
PUBLIC_REPOSITORY. No git push, source checkout, or source archive is published.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import urllib.request

SPEC = importlib.util.spec_from_file_location("prepare", Path(__file__).with_name("prepare-public-release.py"))
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)
PUBLIC_REPOSITORY = "1jehuang/jcode-desktop-releases"
CHANNEL = "desktop-latest"


def gh(*args, check=True):
    result = subprocess.run(["gh", *args], capture_output=True, text=True)
    if check and result.returncode:
        raise RuntimeError(result.stderr)
    return result


def release(tag):
    result = gh("release", "view", tag, "--repo", PUBLIC_REPOSITORY, "--json", "isDraft,assets", check=False)
    if result.returncode:
        # Only treat a genuinely absent release as creation permission. Network
        # and authentication failures must not accidentally overwrite metadata.
        if "release not found" not in result.stderr.lower() and "not found" not in result.stderr.lower():
            raise RuntimeError(result.stderr)
        return None
    return json.loads(result.stdout)


def version_key(tag):
    PREPARE.expected_assets(tag)
    core, _, beta = tag.removeprefix("desktop-v").partition("-beta.")
    return (*map(int, core.split(".")), int(beta) if beta else float("inf"))


def verify_download(url, expected_hash, expected_size):
    # Deliberately no GitHub token, cookies, or Authorization header.
    digest = hashlib.sha256()
    size = 0
    with urllib.request.urlopen(url, timeout=120) as response:
        while chunk := response.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    if size != expected_size or digest.hexdigest() != expected_hash:
        raise ValueError(f"anonymous download failed integrity check: {url}")
    print(f"Verified anonymous download: {url} ({size} bytes)", flush=True)


def publish(directory, tag):
    manifest = PREPARE.validate(directory, tag)
    manifest_path = directory / "latest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    current = release(tag)
    files = [directory / asset["name"] for asset in manifest["assets"]] + [manifest_path]
    if current and not current["isDraft"]:
        with tempfile.TemporaryDirectory() as temporary:
            gh("release", "download", tag, "--repo", PUBLIC_REPOSITORY, "--pattern", "latest.json", "--dir", temporary)
            if json.loads((Path(temporary) / "latest.json").read_text()) != manifest:
                raise ValueError("refusing to modify an already published version")
    else:
        if not current:
            gh("release", "create", tag, "--repo", PUBLIC_REPOSITORY,
               "--target", "main", "--draft", "--prerelease", "--title", manifest["name"],
               "--notes", "Download at https://jcode.sh/desktop. Includes macOS (signed and notarized), Linux x86_64, and Windows x86_64 packages. Older Mac betas using the private update URL need a one-time DMG reinstall. Application source is not published here.")
        gh("release", "upload", tag, "--repo", PUBLIC_REPOSITORY, "--clobber", *map(str, files))
        gh("release", "edit", tag, "--repo", PUBLIC_REPOSITORY, "--draft=false")

    # Publish only complete versions. Check exact public bytes before promoting
    # the website or updater to this release.
    for asset in manifest["assets"]:
        url = f"https://github.com/{PUBLIC_REPOSITORY}/releases/download/{tag}/{asset['name']}"
        verify_download(url, asset["sha256"], asset["size"])

    channel = release(CHANNEL)
    if channel:
        with tempfile.TemporaryDirectory() as temporary:
            result = gh("release", "download", CHANNEL, "--repo", PUBLIC_REPOSITORY,
                        "--pattern", "latest.json", "--dir", temporary, check=False)
            if result.returncode:
                if any(a["name"] == "latest.json" for a in channel["assets"]):
                    raise RuntimeError("cannot read current channel manifest")
            else:
                previous = json.loads((Path(temporary) / "latest.json").read_text())
                if version_key(previous["tag_name"]) > version_key(tag):
                    print("Newer version already published. Leaving current channel unchanged.")
                    return
    else:
        gh("release", "create", CHANNEL, "--repo", PUBLIC_REPOSITORY, "--target", "main",
           "--prerelease", "--title", "Desktop download metadata",
           "--notes", "Machine-readable download metadata and signed update feed. Install from https://jcode.sh/desktop.")
    gh("release", "upload", CHANNEL, "--repo", PUBLIC_REPOSITORY, "--clobber",
       str(directory / "appcast.xml"), str(manifest_path))
    print(f"Published and promoted {tag}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("tag")
    args = parser.parse_args()
    publish(args.directory, args.tag)
