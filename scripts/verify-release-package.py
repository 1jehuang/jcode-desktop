#!/usr/bin/env python3
"""Validate release payloads and their binary architecture without executing them."""
import argparse
import pathlib
import re
import struct
import subprocess
import tarfile
import zipfile

BINARIES = {"jcode-desktop", "jcode", "jcode-harness-api-bridge"}
EXPECTED_UNIX = BINARIES | {"jcode.desktop", "jcode.png"}
EXPECTED_WINDOWS = {name + ".exe" for name in BINARIES} | {"Jcode.png", "jcode-desktop.exe.manifest"}
TARGETS = {
    "x86_64-unknown-freebsd": ("ELF", 62, None),
    "x86_64-unknown-linux-gnu": ("ELF", 62, "amd64"),
    "aarch64-unknown-linux-gnu": ("ELF", 183, "arm64"),
    "x86_64-pc-windows-msvc": ("PE", 0x8664, None),
    "aarch64-pc-windows-msvc": ("PE", 0xAA64, None),
}


def filename_target(artifact):
    match = re.fullmatch(r"Jcode-[0-9]+\.[0-9]+\.[0-9]+(?:[.-][0-9A-Za-z.-]+)?-(linux|windows|freebsd)-(x86_64|aarch64|amd64|arm64)(\.tar\.gz|\.deb|\.zip)", artifact.name)
    if not match:
        return None
    platform, arch, suffix = match.groups()
    if platform == "freebsd" and arch == "x86_64" and suffix == ".tar.gz":
        return "x86_64-unknown-freebsd"
    if platform == "linux" and ((suffix == ".tar.gz" and arch in ("x86_64", "aarch64")) or (suffix == ".deb" and arch in ("amd64", "arm64"))):
        return ("x86_64" if arch in ("x86_64", "amd64") else "aarch64") + "-unknown-linux-gnu"
    if platform == "windows" and suffix == ".zip" and arch in ("x86_64", "aarch64"):
        return arch + "-pc-windows-msvc"
    return None


def verify_binary(stream, target):
    kind, machine, _ = TARGETS[target]
    header = stream.read(64)
    if kind == "ELF":
        if len(header) < 64 or header[:6] != b"\x7fELF\x02\x01":
            raise ValueError("expected a 64-bit little-endian ELF header")
        actual = struct.unpack_from("<H", header, 18)[0]
    else:
        if len(header) < 64 or header[:2] != b"MZ":
            raise ValueError("expected a PE DOS header")
        offset = struct.unpack_from("<I", header, 60)[0]
        if offset < 64:
            raise ValueError("invalid PE header offset")
        stream.seek(offset)
        pe = stream.read(26)
        if len(pe) != 26 or pe[:4] != b"PE\0\0" or pe[24:26] != b"\x0b\x02":
            raise ValueError("expected a 64-bit PE header")
        actual = struct.unpack_from("<H", pe, 4)[0]
    if actual != machine:
        raise ValueError(f"architecture mismatch: machine 0x{actual:04x}, expected 0x{machine:04x} for {target}")


def verify_members(members, open_member, target, windows=False):
    expected = EXPECTED_WINDOWS if windows else EXPECTED_UNIX
    binaries = {name + ".exe" for name in BINARIES} if windows else BINARIES
    found = set()
    for name, member in members:
        basename = pathlib.PurePosixPath(name).name
        if basename not in expected:
            continue
        if basename in found:
            raise ValueError(f"duplicate payload: {basename}")
        found.add(basename)
        if basename in binaries:
            with open_member(member) as stream:
                try:
                    verify_binary(stream, target)
                except ValueError as error:
                    raise ValueError(f"{name}: {error}") from error
    missing = expected - found
    if missing:
        raise ValueError(f"missing {', '.join(sorted(missing))}")


def verify_tar(archive, target):
    # Symlinks and directories must not masquerade as bundled files.
    verify_members(((m.name, m) for m in archive if m.isfile()), archive.extractfile, target)


def verify(artifact, target):
    labeled_target = filename_target(artifact)
    if labeled_target and labeled_target != target:
        raise ValueError(f"filename architecture {labeled_target} does not match --target {target}")
    if artifact.name.endswith(".tar.gz"):
        if TARGETS[target][0] != "ELF":
            raise ValueError("Unix archive requires an ELF target")
        with tarfile.open(artifact) as archive:
            verify_tar(archive, target)
    elif artifact.suffix == ".zip":
        if TARGETS[target][0] != "PE":
            raise ValueError("Windows archive requires a Windows target")
        with zipfile.ZipFile(artifact) as archive:
            verify_members(((m.filename, m) for m in archive.infolist() if not m.is_dir()), archive.open, target, windows=True)
    elif artifact.suffix == ".deb":
        if TARGETS[target][2] is None:
            raise ValueError("Debian package requires a Linux target")
        arch = subprocess.check_output(["dpkg-deb", "--field", str(artifact), "Architecture"], text=True).strip()
        if arch != TARGETS[target][2]:
            raise ValueError(f"Debian Architecture {arch!r} does not match {target}")
        # Stream the payload instead of buffering all three release binaries.
        command = ["dpkg-deb", "--fsys-tarfile", str(artifact)]
        with subprocess.Popen(command, stdout=subprocess.PIPE) as process:
            try:
                with process.stdout:
                    with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
                        verify_tar(archive, target)
                    while process.stdout.read(65536):
                        pass
                if process.wait():
                    raise subprocess.CalledProcessError(process.returncode, command)
            finally:
                if process.poll() is None:
                    process.terminate()
    else:
        raise ValueError("expected .tar.gz, .zip, or .deb")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=pathlib.Path)
    parser.add_argument("--target", choices=TARGETS, help="Rust target triple (otherwise inferred from the release filename)")
    args = parser.parse_args()
    target = args.target or filename_target(args.artifact)
    if target is None:
        parser.error("--target is required when the release filename does not identify a supported target")
    try:
        verify(args.artifact, target)
    except (ValueError, OSError, tarfile.TarError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{args.artifact}: {error}\n")
    print(f"verified {args.artifact} ({target})")


if __name__ == "__main__":
    main()
