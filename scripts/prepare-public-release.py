#!/usr/bin/env python3
"""Validate a complete desktop release before publishing binaries, never source.

The input directory contains assets downloaded from the private GitHub release.
No network access or credentials are needed for this validation step.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import xml.etree.ElementTree as ET

PUBLIC_BASE = "https://jcode.sh/desktop/releases"
SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
TAG = re.compile(r"desktop-v(\d+\.\d+\.\d+(?:-beta\.\d+)?)\Z")


def expected_assets(tag: str) -> dict[str, list[str]]:
    match = TAG.fullmatch(tag)
    if not match:
        raise ValueError("invalid desktop release tag")
    version = match[1]
    return {
        "SHA256SUMS": [
            "Jcode-macOS-universal.dmg",
            f"Jcode-{version}-macOS-universal.zip",
        ],
        "SHA256SUMS-linux": [
            f"Jcode-{version}-linux-x86_64.tar.gz",
            f"Jcode-{version}-linux-amd64.deb",
        ],
        "SHA256SUMS-windows": [f"Jcode-{version}-windows-x86_64.zip"],
    }


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def validate(directory: Path, tag: str) -> dict:
    checksums = expected_assets(tag)
    required = set(checksums) | {"appcast.xml"}
    hashes = {}
    for checksum, names in checksums.items():
        entries = {}
        for line in (directory / checksum).read_text().splitlines():
            match = re.fullmatch(r"([a-fA-F0-9]{64})\s+\*?(.+)", line)
            if not match:
                raise ValueError(f"malformed checksum in {checksum}")
            # Historical checksum files included build-machine absolute paths.
            # Only use the basename for matching, never open the supplied path.
            name = match[2].replace("\\", "/").rsplit("/", 1)[-1]
            if name in entries or name not in names:
                raise ValueError(f"unexpected or duplicate checksum asset: {name}")
            entries[name] = match[1].lower()
        if set(entries) != set(names):
            raise ValueError(f"incomplete checksum file: {checksum}")
        for name in names:
            path = directory / name
            if path.is_symlink() or not path.is_file() or not path.stat().st_size:
                raise ValueError(f"missing or unsafe release artifact: {name}")
            if digest(path) != entries[name]:
                raise ValueError(f"checksum mismatch: {name}")
        hashes.update(entries)
        required.update(names)

    for name in required:
        if (directory / name).is_symlink():
            raise ValueError(f"symlink is not a release artifact: {name}")
    enclosure = ET.parse(directory / "appcast.xml").findall("./channel/item/enclosure")
    if len(enclosure) != 1:
        raise ValueError("release appcast must have exactly one enclosure")
    archive = f"Jcode-{tag.removeprefix('desktop-v')}-macOS-universal.zip"
    item = enclosure[0]
    if item.get("url") != f"{PUBLIC_BASE}/{tag}/{archive}":
        raise ValueError("appcast must point to this release's public archive")
    signature = base64.b64decode(item.get(f"{{{SPARKLE}}}edSignature", ""), validate=True)
    if len(signature) != 64:
        raise ValueError("appcast requires an Ed25519 signature")
    if item.get("length") != str((directory / archive).stat().st_size):
        raise ValueError("appcast archive length mismatch")
    # Signature authenticity is checked by Sparkle against the embedded public
    # key on installation. Here we check structure and preserve the signed bytes.
    return {
        "tag_name": tag,
        "name": f"Jcode Desktop {tag.removeprefix('desktop-v')}",
        "prerelease": "-" in tag.removeprefix("desktop-v"),
        "draft": False,
        "assets": [
            {
                "name": name,
                "size": (directory / name).stat().st_size,
                "sha256": hashes.get(name) or digest(directory / name),
                "browser_download_url": f"{PUBLIC_BASE}/{tag}/{name}",
            }
            for name in sorted(required)
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("tag")
    parser.add_argument("--manifest", type=Path)
    args = parser.parse_args()
    manifest = validate(args.directory, args.tag)
    content = json.dumps(manifest, indent=2) + "\n"
    if args.manifest:
        args.manifest.write_text(content)
    else:
        print(content, end="")


if __name__ == "__main__":
    main()
