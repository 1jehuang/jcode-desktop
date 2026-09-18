#!/usr/bin/env python3
"""Heuristic motion review queue. No dependencies, no automatic motion verdicts."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

SOURCE = "crates/jcode-desktop-ui/src"
INVENTORY = "docs/motion-inventory.json"


def digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def mask_rust(text: str) -> str:
    """Blank comments/literals, preserving offsets. Handle nested and raw strings.

    Lifetimes are not character literals. This is intentionally not a Rust parser.
    """
    raw_pattern = re.compile(r'(?:br|r)(#*)"')
    char_pattern = re.compile(r"(?:b)?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'")
    out = list(text)
    i = 0
    while i < len(text):
        start = i
        if text.startswith("//", i):
            end = text.find("\n", i)
            i = len(text) if end < 0 else end
        elif text.startswith("/*", i):
            i += 2
            depth = 1
            while i < len(text) and depth:
                if text.startswith("/*", i):
                    depth += 1
                    i += 2
                elif text.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
        else:
            raw = raw_pattern.match(text, i)
            char = char_pattern.match(text, i)
            if raw:
                close = '"' + raw[1]
                end = text.find(close, raw.end())
                i = len(text) if end < 0 else end + len(close)
            elif char:
                i = char.end()
            elif text[i] == '"':
                i += 1
                while i < len(text):
                    if text[i] == "\\":
                        i += 2
                    elif text[i] == '"':
                        i += 1
                        break
                    else:
                        i += 1
            else:
                i += 1
                continue
        for j in range(start, min(i, len(text))):
            if text[j] != "\n":
                out[j] = " "
    return "".join(out)


def block_end(mask: str, start: int) -> int:
    depth = 0
    for i in range(start, len(mask)):
        if mask[i] == "{":
            depth += 1
        elif mask[i] == "}":
            depth -= 1
            if not depth:
                return i + 1
    raise ValueError("unbalanced Rust block")


def functions(text: str) -> list[dict]:
    mask = mask_rust(text)
    found = []
    for match in re.finditer(r"\bfn\s+([A-Za-z_]\w*)\s*(?:<[^{};]*>)?\s*\(", mask):
        start = mask.find("{", match.end())
        semi = mask.find(";", match.end())
        if start < 0 or 0 <= semi < start:
            continue
        end = block_end(mask, start)
        found.append({"name": match[1], "start": match.start(), "end": end,
                      "signature": mask[match.start():start],
                      "sha256": digest(text[match.start():end]),
                      "line": text.count("\n", 0, match.start()) + 1})
    return found


def scan(root: Path, additional_sites: list[dict] | None = None) -> tuple[list[dict], list[dict]]:
    candidates, advisory = [], []
    selected = {(site["path"], site["symbol"]) for site in (additional_sites or [])}
    if not (root / SOURCE).is_dir():
        raise ValueError(f"missing source directory: {SOURCE}")
    for path in sorted((root / SOURCE).rglob("*.rs")):
        if path.stem.endswith("_tests") or path.stem.startswith("test_") or path.stem == "tests" or "tests" in path.parts:
            continue
        text = path.read_text()
        lines = text.splitlines()
        mask = mask_rust(text)
        # Exclude conventional inline test modules without truncating later code.
        for match in re.finditer(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*mod\s+\w+\s*\{", mask):
            end = block_end(mask, mask.index("{", match.start()))
            mask = mask[:match.start()] + re.sub(r"[^\n]", " ", mask[match.start():end]) + mask[end:]
        rel = path.relative_to(root).as_posix()
        for fn in functions(text):
            if not mask[fn["start"]:fn["start"] + 2].strip():
                continue
            if ((rel, fn["name"]) in selected or (re.match(r"(?:toggle|open|close)_", fn["name"])
                    and re.search(r"&\s*(?:'[A-Za-z_]\w*\s+)?mut\s+self\b", fn["signature"])
                    and re.search(r"\bContext\s*<", fn["signature"]))):
                candidates.append({"id": f'{rel}::{fn["name"]}', "path": rel,
                                   "symbol": fn["name"], "line": fn["line"],
                                   "sha256": fn["sha256"]})
        patterns = {
            "handler": r"\.(?:on_[A-Za-z_]+|subscribe_in|subscribe)\s*\(",
            "conditional": r"\.(?:when|when_some)\s*\(|\bif\s+(?:let\s+[^=\n]+\s*=\s*)?self\.",
        }
        for kind, pattern in patterns.items():
            for match in re.finditer(pattern, mask):
                line = text.count("\n", 0, match.start()) + 1
                advisory.append({"path": rel, "line": line, "kind": kind,
                                 "excerpt": lines[line - 1].strip()[:160]})
    return sorted(candidates, key=lambda c: c["id"]), sorted(advisory, key=lambda c: (c["path"], c["line"], c["kind"]))


def evidence_hash(root: Path, evidence: dict) -> str:
    path = (root / evidence["path"]).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError("evidence path escapes repository")
    text = path.read_text()
    if "symbol" not in evidence:
        return digest(text)
    matches = [fn for fn in functions(text) if fn["name"] == evidence["symbol"]]
    if len(matches) != 1:
        raise ValueError(f'expected one function {evidence["symbol"]}, found {len(matches)}')
    return matches[0]["sha256"]


def audit(root: Path, inventory: dict) -> dict:
    candidates, advisory = scan(root, inventory.get("additional_sites", []))
    errors = []
    if inventory.get("version") != 1 or inventory.get("scope") != "named-ui-methods-v1":
        errors.append("invalid inventory version/scope")
    entries = inventory.get("decisions", [])
    if not isinstance(entries, list):
        raise ValueError("decisions must be a list")
    reviewed = {}
    for entry in entries:
        key = entry["id"]
        if key in reviewed:
            errors.append(f"duplicate inventory: {key}")
        reviewed[key] = entry
    ids = {candidate["id"] for candidate in candidates}
    if len(ids) != len(candidates):
        errors.append("ambiguous candidate IDs: rename duplicate methods or refine scanner identity")
    for site in inventory.get("additional_sites", []):
        if f"{site['path']}::{site['symbol']}" not in ids:
            errors.append(f"stale additional site: {site['path']}::{site['symbol']}")
    evidence_cache = {(c["path"], c["symbol"]): c["sha256"] for c in candidates}
    for key in sorted(reviewed.keys() - ids):
        errors.append(f"stale inventory (site removed/renamed/out of scope): {key}")
    gaps, unreviewed = [], []
    for candidate in candidates:
        key = candidate["id"]
        entry = reviewed.get(key)
        if entry is None:
            unreviewed.append(candidate)
            errors.append(f"NEW unreviewed: {key}")
            continue
        if entry.get("decision") not in {"animated", "instant", "gap"}:
            errors.append(f"invalid decision: {key}")
        if not isinstance(entry.get("reason"), str) or not entry["reason"].strip():
            errors.append(f"missing reason: {key}")
        if entry.get("sha256") != candidate["sha256"]:
            errors.append(f"stale source review: {key}")
        evidence = entry.get("evidence", [])
        if not evidence or not any(item.get("kind") == "source" for item in evidence):
            errors.append(f"missing source evidence: {key}")
        if not any(item.get("kind") == "test" for item in evidence) and not str(entry.get("test_gap", "")).strip():
            errors.append(f"missing test evidence or explicit test_gap: {key}")
        for item in evidence:
            try:
                if item.get("kind") not in {"source", "test"} or not item.get("note", "").strip():
                    raise ValueError("evidence needs kind and explanatory note")
                evidence_key = (item["path"], item.get("symbol"))
                if evidence_key not in evidence_cache:
                    evidence_cache[evidence_key] = evidence_hash(root, item)
                if evidence_cache[evidence_key] != item.get("sha256"):
                    raise ValueError("fingerprint changed")
            except (OSError, ValueError, KeyError) as exc:
                errors.append(f"stale/invalid evidence: {key}: {item.get('path')}: {exc}")
        if entry.get("decision") == "gap":
            if entry.get("priority") not in {1, 2, 3}:
                errors.append(f"gap needs priority 1, 2 or 3: {key}")
            gaps.append({**candidate, **entry})
    gaps.sort(key=lambda c: (c.get("priority", 99), c["id"]))
    return {"scope": "named-ui-methods-v1", "candidates": candidates,
            "unreviewed": unreviewed, "gaps": gaps, "errors": errors,
            "advisory": advisory,
            "warning": "Heuristic discovery is not proof of complete UI or animation coverage. Advisory sites are NOT gated."}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--inventory", default=INVENTORY, help="path relative to root")
    parser.add_argument("--check", action="store_true", help="fail on unreviewed sites or stale/invalid reviews/evidence")
    parser.add_argument("--json", action="store_true", help="full deterministic machine-readable report")
    parser.add_argument("--discover", action="store_true", help="list advisory handler/conditional sites too")
    args = parser.parse_args(argv)
    try:
        result = audit(args.root, json.loads((args.root / args.inventory).read_text()))
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as exc:
        print(f"motion audit: {exc}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print(result["warning"])
        print(f"{len(result['candidates'])} gated candidates, {len(result['unreviewed'])} unreviewed, "
              f"{len(result['gaps'])} known gaps, {len(result['advisory'])} advisory occurrences")
        for gap in result["gaps"]:
            print(f"P{gap.get('priority', '?')} {gap['path']}:{gap['line']} {gap['symbol']}: {gap['reason']}")
        for error in result["errors"]:
            print(f"ERROR {error}")
        if args.discover:
            for site in result["advisory"]:
                print(f"ADVISORY {site['kind']} {site['path']}:{site['line']} {site['excerpt']}")
    return 1 if args.check and result["errors"] else 0


if __name__ == "__main__":
    sys.exit(main())
