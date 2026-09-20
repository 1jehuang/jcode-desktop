# Desktop 0.3.0 release

## Scope and source

This minor release integrates the completed Desktop development branch with the published 0.2.1 release fixes. Existing work was preserved in commit `aca2f66`, followed by a non-rewriting merge of `origin/main` in `bdb85c5`.

All four Desktop package manifests use `0.3.0`. The macOS, Linux/Windows, and FreeBSD build workflows pin the same published runtime commit: `b6375bb146b211cc77b4ec63e2d4986bf7316b2b`. This includes the structured stop-reason and keyed parallel tool-streaming APIs used by Desktop. Unrelated pending changes in the adjacent runtime checkout are excluded.

See `CHANGELOG.md` for user-facing highlights and `release-orchestration.md` for the command, safety gates, measured latency, and resume behavior.

## Development versioning

Local builds retain the intended SemVer core and report commit distance separately, for example `0.3.0-dev.54`. The distance is anchored to the highest reachable stable Desktop release at or below the intended base, not an unrelated branch tag. Package builds retain the exact explicit release version. Build details still report commit and dirty status.

Regression coverage includes next-minor development, annotated and packed tags, unrelated/future tags, dirty trees, source archives, empty repositories, linked worktrees, update comparison, and human-readable labels.

## Release preflight

The first full suite found stale assertions after the image, avatar, route-label, and workspace-version UI changes, plus a real narrow-header collision. The header now preserves tabs and primary controls before displaying secondary version metadata. At very narrow widths, the optional minimap yields space without changing its saved preference. Tests exercise widths from 360 to 1440 pixels.

Cloud model synchronization is now wired into production helper entry points and the Desktop warm connection path. A sync cannot turn an expired or changed readiness proof into permission to create a session. These checks are covered offline and do not claim a new live-cloud acceptance run.

Regular FreeBSD builds now use the verified compatibility preparation and native graphical smoke fixes previously limited to the 0.2.1 recovery. No native platform, signing, package-integrity, website-download, or macOS installation gate has been removed.

Observed local preflight:

- Locked Rust workspace suite, serial UI execution: 1,337 passed, 10 ignored, zero failures.
- Python release contracts: 188 tests, 5 skipped, zero failures. FreeBSD compatibility helper: 17 passed, including exact upstream source hashes.
- Cloud helper suite: 159 passed. Desktop cloud and direct-scroll focused regressions passed.
- Matched release-profile Desktop host and UI built successfully. The executable reports `0.3.0-dev.56` with commit and dirty metadata from the build.
- Private-Xvfb responsive acceptance passed four native resizes, three sidebar drawer cycles, and six tab selections with stable panel identities, focus, and width preferences. Wide and compact PNGs were inspected.
- `git diff --check` passed.

Native CI and public publication evidence remain pending. A tag or draft alone is not publication.

## Running development host

The minor bump changes the shared API crate identity, including the process-owned image allocator's Rust type identity. The existing host must receive a normal safe restart with the matched new host/UI pair rather than a forced hot reload across this boundary. The release work preserves the running host, sessions, terminals, and unsaved drafts. Offline renders verify the new version without claiming that the old live host changed.
