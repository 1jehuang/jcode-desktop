#!/usr/bin/env python3
"""Explicit, bounded personal-alpha tar backups. Python stdlib only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shlex
import stat
import subprocess
import sys
import tarfile
import time
import uuid

DEFAULT_MAX = 1024 ** 3
MAX_MEMBERS = 100000
ROOTS = ("workspaces", ".jcode/sessions")
# Applied to EVERY path component, on export and verification.
PRIVATE = re.compile(
    r"(^\.env|(^|[._-])env($|[._-])|environment|credential|secret|"
    r"(^|[._-])(auth|oauth|token|password|passwd|keyring)([._-]|$)|"
    r"^id_(rsa|dsa|ecdsa|ed25519)|\.(pem|key|p12|pfx|keystore)$|"
    r"^\.(ssh|aws|azure|config|gnupg|git|kube|docker|claude|codex|gemini|copilot|npmrc|pypirc|netrc)$|"
    r"^(node_modules|venv|\.venv|__pycache__)$)", re.I)
SECRET = re.compile(
    rb"-----BEGIN [^-\r\n]*PRIVATE KEY-----|"
    rb"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b|\bsk-(?:ant-)?[A-Za-z0-9_-]{20,}|"
    rb"\bgh[pousr]_[A-Za-z0-9]{20,}|"
    rb"(?i:bearer\s+[a-z0-9._~-]{16,}|"
    rb"(?:api[_-]?key|access[_-]?token|client[_-]?secret|password)\s*[\"']?\s*[:=]\s*[\"']?[^\s\"']{8,})")


class BackupError(Exception):
    pass


def allowed(name):
    return not any(PRIVATE.search(part) for part in name.split("/"))


def validate_name(name):
    parts = name.split("/")
    if (not name or "\\" in name or any(ord(c) < 32 for c in name)
            or any(p in ("", ".", "..") for p in parts)
            or not any(name == root or name.startswith(root + "/") for root in ROOTS)
            or not allowed(name)):
        raise BackupError("Unsafe or excluded archive member")


def scan_stream(stream, size, output=None):
    left, tail = size, b""
    while left:
        chunk = stream.read(min(left, 65536))
        if not chunk:
            raise BackupError("Truncated file")
        if SECRET.search(tail + chunk):
            raise BackupError("Recognizable credential content detected; export refused")
        tail = (tail + chunk)[-4096:]
        if output is not None:
            output.write(chunk)
        left -= len(chunk)


def remote_export(max_bytes):
    """Walk with directory FDs and O_NOFOLLOW, never dereference links."""
    total = count = 0
    flags = os.O_RDONLY | os.O_NOFOLLOW
    home = os.open(Path.home(), flags | os.O_DIRECTORY)
    try:
        with tarfile.open(fileobj=sys.stdout.buffer, mode="w|", format=tarfile.USTAR_FORMAT) as archive:
            def walk(parent, component, name):
                nonlocal total, count
                if not allowed(name):
                    return
                validate_name(name)
                info = os.stat(component, dir_fd=parent, follow_symlinks=False)
                if not (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)):
                    raise BackupError("Links or special files in export roots; remove or relocate them first")
                fd = os.open(component, flags | (os.O_DIRECTORY if stat.S_ISDIR(info.st_mode) else os.O_NONBLOCK), dir_fd=parent)
                try:
                    actual = os.fstat(fd)
                    if (actual.st_dev, actual.st_ino, actual.st_mode) != (info.st_dev, info.st_ino, info.st_mode):
                        raise BackupError("Source changed during export")
                    if stat.S_ISREG(actual.st_mode) and actual.st_nlink != 1:
                        raise BackupError("Hard-linked file in export roots")
                    count += 1
                    total += 512 + ((actual.st_size + 511) // 512 * 512 if stat.S_ISREG(actual.st_mode) else 0)
                    if count > MAX_MEMBERS or total + 10240 > max_bytes:
                        raise BackupError("Export exceeds size/member cap")
                    entry = tarfile.TarInfo(name)
                    entry.mode = 0o700 if stat.S_ISDIR(actual.st_mode) else 0o600
                    entry.mtime = int(actual.st_mtime)
                    if stat.S_ISDIR(actual.st_mode):
                        entry.type = tarfile.DIRTYPE
                        archive.addfile(entry)
                        for child in sorted(os.listdir(fd)):
                            walk(fd, child, name + "/" + child)
                    else:
                        entry.size = actual.st_size
                        with os.fdopen(os.dup(fd), "rb") as source:
                            scan_stream(source, entry.size)
                            source.seek(0)
                            archive.addfile(entry, source)
                        after = os.fstat(fd)
                        if (after.st_size, after.st_mtime_ns, after.st_ctime_ns) != (actual.st_size, actual.st_mtime_ns, actual.st_ctime_ns):
                            raise BackupError("Source changed during export")
                finally:
                    os.close(fd)
            walk(home, "workspaces", "workspaces")
            jcode = os.open(".jcode", flags | os.O_DIRECTORY, dir_fd=home)
            try:
                walk(jcode, "sessions", ".jcode/sessions")
            finally:
                os.close(jcode)
    finally:
        os.close(home)


def private_directory(path, create=False):
    path = Path(os.path.abspath(path))
    # Refuse symlink ancestors as well as a symlink leaf.
    for part in (*reversed(path.parents), path):
        if part.is_symlink():
            raise BackupError("Symlink directory path refused")
    if create and not path.exists():
        path.mkdir(mode=0o700)  # Parent must already exist.
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise BackupError("Backup directory must be owned by you with mode 0700")
    return path


def open_regular(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    stream = os.fdopen(fd, "rb")
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        stream.close()
        raise BackupError("Expected non-linked regular file")
    return stream


def inspect_archive(stream, max_bytes):
    size = os.fstat(stream.fileno()).st_size
    if size > max_bytes or size < 1024:
        raise BackupError("Archive exceeds cap or is incomplete")
    stream.seek(0)
    digest = hashlib.file_digest(stream, "sha256").hexdigest()
    stream.seek(0)
    # Inspect raw headers first so extension records cannot allocate unbounded memory
    # or rewrite member names before our validation sees them.
    offset = 0
    while True:
        header = stream.read(512)
        if len(header) != 512:
            raise BackupError("Truncated tar header")
        if header == b"\0" * 512:
            break
        member = tarfile.TarInfo.frombuf(header, "utf-8", "strict")
        if member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE):
            raise BackupError("Only plain files and directories are accepted")
        validate_name(member.name)
        if member.size < 0:
            raise BackupError("Negative member size")
        offset += 512 + ((member.size + 511) // 512) * 512
        if offset > size:
            raise BackupError("Member exceeds archive bounds")
        stream.seek(offset)
    stream.seek(0)
    seen, files, directories, payload = set(), set(), set(), 0
    # No compression, tarfile.extract/extractall, links, devices or inherited permissions.
    with tarfile.open(fileobj=stream, mode="r:") as archive:
        for member in archive:
            validate_name(member.name)
            if (member.name in seen or len(seen) >= MAX_MEMBERS
                    or member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE)
                    or member.pax_headers or member.linkname or member.size < 0
                    or (member.isdir() and member.size != 0)):
                raise BackupError("Invalid archive member type or metadata")
            if any(parent in files for parent in str_parents(member.name)):
                raise BackupError("File used as directory")
            if member.isfile() and member.name in directories:
                raise BackupError("Directory replaced by file")
            seen.add(member.name)
            directories.update(str_parents(member.name))
            payload += member.size
            if payload > max_bytes or member.offset_data + member.size > size:
                raise BackupError("Payload exceeds cap or archive bounds")
            if member.isfile():
                files.add(member.name)
                scan_stream(archive.extractfile(member), member.size)
        end = archive.offset
    if not all(root in seen for root in ROOTS) or any(root in files for root in ROOTS):
        raise BackupError("Both export root directories are required")
    stream.seek(end)
    trailing = 0
    while chunk := stream.read(65536):
        if chunk.strip(b"\0"):
            raise BackupError("Nonzero data after archive end")
        trailing += len(chunk)
    if trailing < 1024:
        raise BackupError("Missing tar end markers")
    return {"format": 1, "sha256": digest, "archive_bytes": size, "payload_bytes": payload, "members": len(seen)}


def str_parents(name):
    parts = name.split("/")
    return ("/".join(parts[:i]) for i in range(1, len(parts)))


def backup(host, directory, max_bytes=DEFAULT_MAX, timeout=300):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.@-]*", host):
        raise BackupError("Use a plain SSH host alias or user@hostname, not SSH options")
    directory = private_directory(directory, create=True)
    bundle = directory / ("backup-" + time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + uuid.uuid4().hex)
    bundle.mkdir(mode=0o700)
    # Incomplete bundles are deliberately retained. No automatic deletion.
    archive_path = bundle / "archive.tar"
    script = open(__file__, "rb")
    command = ["ssh", "-T", "-oBatchMode=yes", "-oStrictHostKeyChecking=yes", "-oConnectTimeout=15",
               host, "python3 - --remote-export " + shlex.quote(str(max_bytes))]
    try:
        process = subprocess.Popen(command, stdin=script, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    finally:
        script.close()
    try:
        deadline = time.monotonic() + timeout
        with os.fdopen(os.open(archive_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as target:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                received = 0
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or not selector.select(remaining):
                        raise BackupError("SSH export timed out")
                    chunk = os.read(process.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    received += len(chunk)
                    if received > max_bytes:
                        raise BackupError("SSH archive exceeds byte cap")
                    target.write(chunk)
            target.flush()
            os.fsync(target.fileno())
        if process.wait(timeout=max(0.01, deadline - time.monotonic())) != 0:
            raise BackupError("SSH export failed; ensure ~/workspaces and ~/.jcode/sessions both exist as real directories, sources are readable and within limits, and SSH authentication succeeds. Incomplete bundle retained without manifest.")
        with open_regular(archive_path) as stream:
            manifest = inspect_archive(stream, max_bytes)
        with os.fdopen(os.open(bundle / "manifest.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
            json.dump(manifest, output, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        return bundle
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()
        process.stdout.close()


def verify(bundle, restore_to=None, max_bytes=DEFAULT_MAX):
    bundle = private_directory(bundle)
    with open_regular(bundle / "manifest.json") as metadata:
        raw = metadata.read(65537)
        if len(raw) > 65536:
            raise BackupError("Manifest too large")
        manifest = json.loads(raw)
    with open_regular(bundle / "archive.tar") as source:
        observed = inspect_archive(source, max_bytes)
        if manifest != observed:
            raise BackupError("Checksum or manifest mismatch")
        if restore_to is not None:
            destination = Path(os.path.abspath(restore_to))
            private_directory(destination.parent)
            destination.mkdir(mode=0o700)  # Must be new, even an empty existing directory is refused.
            source.seek(0)
            recovered = 0
            with tarfile.open(fileobj=source, mode="r:") as archive:
                for member in archive:
                    validate_name(member.name)
                    if member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE) or member.linkname:
                        raise BackupError("Archive changed during recovery")
                    recovered += member.size
                    if member.size < 0 or recovered > max_bytes:
                        raise BackupError("Recovery exceeds byte cap")
                    target = destination / member.name
                    for parent in str_parents(member.name):
                        (destination / parent).mkdir(mode=0o700, exist_ok=True)
                    if member.isdir():
                        target.mkdir(mode=0o700, exist_ok=True)
                    else:
                        with os.fdopen(os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), "wb") as output:
                            scan_stream(archive.extractfile(member), member.size, output)
            # Recheck the same open archive before calling the recovery successful.
            if inspect_archive(source, max_bytes) != observed:
                raise BackupError("Archive changed during recovery; do not use partial destination")
    return observed


def positive(value):
    result = int(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("Must be positive")
    return result


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--remote-export":
        remote_export(positive(sys.argv[2]))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    export = commands.add_parser("backup", help="Explicit SSH export to a new local private bundle")
    export.add_argument("--host", required=True)
    export.add_argument("--backup-dir", required=True, type=Path)
    export.add_argument("--timeout", type=positive, default=300)
    check = commands.add_parser("verify", help="Validate a bundle and optionally recover to a NEW local directory")
    check.add_argument("bundle", type=Path)
    check.add_argument("--restore-to", type=Path)
    for command in (export, check):
        command.add_argument("--max-bytes", type=positive, default=DEFAULT_MAX)
    args = parser.parse_args()
    try:
        if args.command == "backup":
            print(backup(args.host, args.backup_dir, args.max_bytes, args.timeout))
        else:
            print(json.dumps(verify(args.bundle, args.restore_to, args.max_bytes), indent=2))
    except (BackupError, OSError, ValueError, tarfile.TarError, subprocess.SubprocessError) as error:
        print("Backup refused: " + str(error), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
