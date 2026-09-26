#!/usr/bin/env python3
"""Release notes for a Desktop tag, shared by the publisher and Discord announcer.

Source order:
1. The curated `### Jcode Desktop VERSION` section of CHANGELOG.md on main.
2. User-visible commit subjects since the previous release tag. Automatic betas
   are cut from main without a curated section, so they still ship real notes.

Only trusted main content is read. Tags are resolved with local git, which
requires a full-history checkout (`fetch-depth: 0`).
"""
from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TAG_RE = re.compile(r"desktop-v(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?\Z")
SECTION_ORDER = ("Themes", "Highlights", "Improvements", "Fixes")
MAX_COMMITS = 30
# Commits that describe release mechanics rather than user-visible changes.
NOISE = re.compile(r"^(?:Merge\b|Revert \"Merge\b|Release Desktop\b|Bump\b|chore\b|ci\b|docs?\b|test[s]?\b)", re.I)


def version_key(tag):
    match = TAG_RE.fullmatch(tag)
    if not match:
        raise ValueError(f"Invalid release tag: {tag}")
    major, minor, patch, beta = match.groups()
    return (int(major), int(minor), int(patch), int(beta) if beta else float("inf"))


def changelog_section(text, version):
    """Return (headline, [(section, [bullets])]) for an exact version, or None."""
    lines = text.splitlines()
    heading = f"### Jcode Desktop {version}"
    start, fence = None, False
    for index, line in enumerate(lines):
        if line.strip().startswith("```"):
            fence = not fence
        elif not fence and line.strip() == heading:
            start = index
            break
    if start is None:
        return None
    headline, sections, current, fence = "", [], None, False
    for line in lines[start + 1:]:
        if line.strip().startswith("```"):
            fence = not fence
            continue
        if fence:
            continue
        if re.match(r"#{1,3}\s", line):
            break
        section = re.match(r"####\s+(.+?)\s*$", line)
        if section:
            name = section[1].strip()
            current = next((s for s in SECTION_ORDER if s.lower() == name.lower()), None)
            if current:
                sections.append((current, []))
            continue
        bullet = re.match(r"^\s*[-*+]\s+(.*\S)", line)
        if bullet:
            if current is None and not sections:
                # Older unsectioned releases list highlights directly.
                current = "Highlights"
                sections.append((current, []))
            if current:
                sections[-1][1].append(bullet[1])
            continue
        if current and sections and sections[-1][1] and line.startswith(("  ", "\t")) and line.strip():
            sections[-1][1][-1] += " " + line.strip()
            continue
        if not sections and not headline and line.strip() and not line.lstrip().startswith(("<!--", "Download")):
            headline = line.strip()
    sections = [(name, items) for name, items in sections if items]
    return (headline, sections) if sections else None


def previous_tag(tag, tags):
    """Nearest lower release. Stable releases compare against the previous stable."""
    key = version_key(tag)
    stable = key[-1] == float("inf")
    older = [t for t in tags if TAG_RE.fullmatch(t) and version_key(t) < key
             and (not stable or version_key(t)[-1] == float("inf"))]
    return max(older, key=version_key, default=None)


def git(*args, cwd=ROOT):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=True).stdout


def commit_subjects(tag, cwd=ROOT):
    tags = git("tag", "--list", "desktop-v*", cwd=cwd).split()
    if tag not in tags:
        return []
    base = previous_tag(tag, tags)
    revs = f"{base}..{tag}" if base else tag
    subjects = []
    for subject in git("log", "--no-merges", "--format=%s", revs, cwd=cwd).splitlines():
        subject = subject.strip()
        if subject and not NOISE.match(subject) and subject not in subjects:
            subjects.append(subject)
    return subjects


def notes_for_tag(tag, *, root=ROOT, changelog=None):
    """Return {"headline", "sections", "source"} for a tag. Never raises on git gaps."""
    version = tag.removeprefix("desktop-v")
    text = changelog if changelog is not None else (root / "CHANGELOG.md").read_text()
    section = changelog_section(text, version)
    if section:
        headline, sections = section
        return {"headline": headline, "sections": sections, "source": "changelog"}
    try:
        subjects = commit_subjects(tag, cwd=root)
    except (OSError, subprocess.CalledProcessError):
        subjects = []
    if subjects:
        extra = len(subjects) - MAX_COMMITS
        items = subjects[:MAX_COMMITS] + ([f"And {extra} more changes."] if extra > 0 else [])
        return {"headline": "", "sections": [("Changes", items)], "source": "commits"}
    return {"headline": "", "sections": [], "source": "none"}


def to_markdown(notes, *, limit=None, heading="###"):
    """Render notes, dropping whole bullets from the end to fit `limit` characters."""
    def render(counts):
        parts = [notes["headline"]] if notes["headline"] else []
        omitted = 0
        for (name, items), count in zip(notes["sections"], counts):
            omitted += len(items) - count
            if count:
                parts.append(f"{heading} {name}\n" + "\n".join(f"- {item}" for item in items[:count]))
        if omitted:
            parts.append(f"_Plus {omitted} more {'change' if omitted == 1 else 'changes'} in the full notes._")
        return "\n\n".join(parts)

    counts = [len(items) for _, items in notes["sections"]]
    text = render(counts)
    while limit is not None and len(text) > limit and any(counts):
        # Trim the last non-empty section first so themes and highlights survive.
        index = max(i for i, count in enumerate(counts) if count)
        counts[index] -= 1
        text = render(counts)
    if limit is not None and len(text) > limit:
        text = text[:limit - 1].rstrip() + "…"
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("tag")
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    version_key(args.tag)
    notes = notes_for_tag(args.tag)
    print(f"<!-- source: {notes['source']} -->")
    print(to_markdown(notes, limit=args.limit) or "No release notes found.")


if __name__ == "__main__":
    main()
