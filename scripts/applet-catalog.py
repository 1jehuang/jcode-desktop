#!/usr/bin/env python3
"""Pack, check, publish and install Jcode Desktop applets from the catalog.

Standard library only. This is the reference client for the catalog API
(docs/applet-catalog.md) until `jcode applet` lands in the CLI.

  applet-catalog.py check   <dir>            validate applet.json locally
  applet-catalog.py pack    <dir> [-o out]   deterministic .tar.gz + sha256
  applet-catalog.py publish <dir>            pack and upload (needs JCODE_API_KEY)
  applet-catalog.py install <publisher/name>[@version]
  applet-catalog.py search  [query]

Install verifies the sha256 from the catalog, refuses unsafe archives, shows
capabilities with risk labels, and writes ~/.jcode/applets/<publisher.name>/.
Telemetry: one anonymous install event with a random per-install id, unless
JCODE_NO_TELEMETRY or DO_NOT_TRACK is set.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import shutil
import sys
import tarfile
import tempfile
import urllib.error
import urllib.request
import uuid
from pathlib import Path

API = os.environ.get("JCODE_APPLETS_API", "https://api.jcode.sh/v1/applets").rstrip("/")
CAPABILITY_RISK = {
    "open_url": "low", "notifications": "low", "clipboard": "medium", "remote_images": "medium",
    "start_chat": "medium", "send_prompt": "high", "read_files": "high", "html": "high",
}
ID_RE = re.compile(r"^([a-z0-9](?:[a-z0-9-]{0,30}[a-z0-9])?)\.([a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?)$")
SEMVER_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$")
SKIP = {".git", "__pycache__", ".DS_Store", "node_modules", ".venv"}
MAX_BYTES = 5 * 1024 * 1024


def die(message: str) -> "None":
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)


def jcode_home() -> Path:
    return Path(os.environ.get("JCODE_HOME") or Path.home() / ".jcode")


def api_key() -> str:
    key = os.environ.get("JCODE_API_KEY", "")
    if not key:
        env_file = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "jcode" / "jcode-subscription.env"
        if env_file.exists():
            for line in env_file.read_text().splitlines():
                if line.startswith("JCODE_API_KEY="):
                    key = line.split("=", 1)[1].strip().strip('"')
    if not key:
        die("set JCODE_API_KEY or run `jcode login` first")
    return key


def check(directory: Path) -> dict:
    path = directory / "applet.json"
    if not path.exists():
        die(f"{path} not found")
    try:
        manifest = json.loads(path.read_text())
    except json.JSONDecodeError as error:
        die(f"applet.json: {error}")
    problems = []
    if not ID_RE.match(str(manifest.get("id", ""))):
        problems.append("id must be <publisher>.<name> (lowercase letters, digits, hyphens)")
    if not SEMVER_RE.match(str(manifest.get("version", ""))):
        problems.append("version must be semver, e.g. 1.0.0")
    for field, low, high in (("title", 1, 60), ("description", 10, 500)):
        if not low <= len(str(manifest.get(field, "")).strip()) <= high:
            problems.append(f"{field} must be {low} to {high} characters")
    if not manifest.get("license"):
        problems.append("license is required (SPDX id such as MIT)")
    command = manifest.get("command")
    bins = (manifest.get("requires") or {}).get("bins", [])
    if not isinstance(command, list) or not command:
        problems.append("command must be a non-empty array")
    else:
        program = command[0]
        if "/" in program:
            if not (directory / program).is_file() or ".." in Path(program).parts or program.startswith("/"):
                problems.append(f"command {program} must be a file inside the applet")
        elif program not in bins and not (directory / program).is_file():
            problems.append(f"declare {program} in requires.bins")
    unknown = [c for c in manifest.get("capabilities", []) if c not in CAPABILITY_RISK]
    if unknown:
        problems.append(f"unknown capabilities: {', '.join(unknown)}")
    if problems:
        die("applet.json:\n  " + "\n  ".join(problems))
    return manifest


def pack(directory: Path) -> bytes:
    """Deterministic tar.gz: sorted names, zeroed owners and times, fixed modes."""
    buffer = io.BytesIO()
    with gzip.GzipFile(fileobj=buffer, mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            for path in sorted(directory.rglob("*")):
                rel = path.relative_to(directory)
                if any(part in SKIP for part in rel.parts) or path.is_symlink() or not path.is_file():
                    continue
                info = tarfile.TarInfo(rel.as_posix())
                data = path.read_bytes()
                info.size = len(data)
                info.mode = 0o755 if os.access(path, os.X_OK) else 0o644
                info.mtime = 0
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                tar.addfile(info, io.BytesIO(data))
    data = buffer.getvalue()
    if len(data) > MAX_BYTES:
        die(f"package is {len(data)} bytes; the limit is {MAX_BYTES}")
    return data


def request(method: str, url: str, body: bytes | None = None, headers: dict | None = None) -> tuple[int, bytes, dict]:
    req = urllib.request.Request(url, data=body, method=method, headers={"User-Agent": "jcode-applet-catalog/1", **(headers or {})})
    try:
        with urllib.request.urlopen(req, timeout=60) as res:
            return res.status, res.read(), dict(res.headers)
    except urllib.error.HTTPError as error:
        return error.code, error.read(), dict(error.headers)


def api_error(status: int, body: bytes) -> str:
    try:
        err = json.loads(body)["error"]
        return f"{status} {err.get('code')}: {err.get('message')}"
    except Exception:
        return f"HTTP {status}"


def risk_lines(capabilities: list[str]) -> list[str]:
    lines = ["  runs a local program with your user permissions (high)"]
    for cap in capabilities:
        lines.append(f"  {cap} ({CAPABILITY_RISK.get(cap, 'high')})")
    return lines


def safe_extract(data: bytes, dest: Path) -> None:
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for member in tar.getmembers():
            name = Path(member.name)
            if member.isdir():
                continue
            if not member.isfile() or name.is_absolute() or ".." in name.parts:
                die(f"refusing unsafe archive entry {member.name}")
        for member in tar.getmembers():
            if member.isfile():
                target = dest / member.name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(tar.extractfile(member).read())
                target.chmod(0o755 if member.mode & 0o111 else 0o644)


def telemetry_allowed() -> bool:
    return not (os.environ.get("JCODE_NO_TELEMETRY") or os.environ.get("DO_NOT_TRACK"))


def cmd_install(spec: str, assume_yes: bool) -> None:
    ident, _, version = spec.partition("@")
    ident = ident.replace(".", "/", 1) if "/" not in ident else ident
    status, body, _ = request("GET", f"{API}/{ident}/resolve" + (f"?version={version}" if version else ""))
    if status != 200:
        die(api_error(status, body))
    info = json.loads(body)
    manifest = info["manifest"]
    print(f"{info['id']} {info['version']}{' (yanked)' if info.get('yanked') else ''}")
    print(manifest.get("description", ""))
    print("Permissions:")
    print("\n".join(risk_lines(manifest.get("capabilities", []))))
    missing = [b for b in manifest.get("requires", {}).get("bins", []) if shutil.which(b) is None]
    if missing:
        print(f"warning: not on PATH: {', '.join(missing)}")
    if not assume_yes and input("Install? [y/N] ").strip().lower() not in ("y", "yes"):
        die("cancelled")
    status, data, headers = request("GET", info["package_url"])
    if status != 200:
        die(api_error(status, data))
    digest = hashlib.sha256(data).hexdigest()
    if digest != info["sha256"]:
        die(f"sha256 mismatch: catalog {info['sha256']}, downloaded {digest}")
    dest = jcode_home() / "applets" / info["install_id"]
    with tempfile.TemporaryDirectory(dir=dest.parent if dest.parent.exists() else None) as tmp:
        staging = Path(tmp) / "pkg"
        staging.mkdir()
        safe_extract(data, staging)
        (staging / ".catalog.json").write_text(json.dumps({"id": info["id"], "version": info["version"], "sha256": digest}, indent=2))
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.exists():
            shutil.rmtree(dest)
        shutil.move(str(staging), dest)
    print(f"Installed to {dest}. Desktop picks it up on the next launch or Ctrl+R.")
    if telemetry_allowed():
        id_file = jcode_home() / "applets" / ".install-ids.json"
        ids = json.loads(id_file.read_text()) if id_file.exists() else {}
        install_id = ids.setdefault(info["id"], uuid.uuid4().hex)
        id_file.write_text(json.dumps(ids, indent=2))
        request("POST", f"{API}/telemetry", json.dumps({"applet": info["id"], "version": info["version"], "event": "install", "install_id": install_id}).encode(), {"Content-Type": "application/json"})


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("check", "pack", "publish"):
        p = sub.add_parser(name)
        p.add_argument("dir", type=Path)
        if name == "pack":
            p.add_argument("-o", "--output", type=Path)
    p = sub.add_parser("install")
    p.add_argument("spec")
    p.add_argument("-y", "--yes", action="store_true")
    p = sub.add_parser("search")
    p.add_argument("query", nargs="?", default="")
    args = parser.parse_args()

    if args.cmd == "check":
        m = check(args.dir)
        print(f"ok {m['id']} {m['version']}")
        print("\n".join(risk_lines(m.get("capabilities", []))))
    elif args.cmd == "pack":
        m = check(args.dir)
        data = pack(args.dir)
        out = args.output or Path(f"{m['id']}-{m['version']}.tar.gz")
        out.write_bytes(data)
        print(f"{out} {len(data)} bytes sha256 {hashlib.sha256(data).hexdigest()}")
    elif args.cmd == "publish":
        m = check(args.dir)
        data = pack(args.dir)
        status, body, _ = request("POST", f"{API}/publish", data, {"Authorization": f"Bearer {api_key()}", "Content-Type": "application/gzip"})
        if status != 201:
            die(api_error(status, body))
        res = json.loads(body)
        print(f"published {res['id']} {res['version']} sha256 {res['sha256']}")
        print(f"state: {res['state']}" + (f" ({res['review_reason']})" if res.get("review_reason") else ""))
        print(res["url"])
    elif args.cmd == "install":
        cmd_install(args.spec, args.yes)
    elif args.cmd == "search":
        from urllib.parse import quote
        status, body, _ = request("GET", f"{API}?q={quote(args.query)}")
        if status != 200:
            die(api_error(status, body))
        for a in json.loads(body)["applets"]:
            print(f"{a['id']:<32} {a['stats']['installs']:>7} installs  {a['risk']:<6}  {a['title']}")


if __name__ == "__main__":
    main()
