# Desktop 0.3.1 release

## Scope and source

This patch release supersedes the immutable Desktop 0.3.0 tag at `a8f7537`. That release failed native bundled-runtime compilation. Its tag is not moved or rewritten, and a tag alone is not evidence of a published working release.

The release is prepared in an isolated detached worktree from the latest committed Desktop source, `51c72a2`. It includes the five UI commits after 0.3.0: live sidebar sessions and compact adjacent rows, shortcut action pictograms, compact Desktop chrome, compact inline background-task status, and special-key shortcut icons. The original checkout and its six uncommitted header edits are deliberately left untouched. Those uncommitted edits are not part of this release.

All four Desktop package manifests and their own Cargo.lock package entries use `0.3.1`. Dependency versions are not bulk-replaced. All three native build workflows pin published runtime commit `3d7f5073ac10d93f4c099494771e1bde86911910`, a single scoped fix on the original runtime pin. ACP routes interleaved tool input by its explicit tool ID, NDJSON preserves IDs while retaining the legacy unkeyed shape, and regression tests cover both paths.

See `CHANGELOG.md` for the complete user-facing highlights and `release-orchestration.md` for publication and acceptance gates.

## Release preflight and publication status

Observed local preflight:

- Full Python release-contract suite: 188 tests, 5 skipped, zero failures. A stale changelog assertion now follows the package manifest version rather than hard-coding 0.3.0.
- FreeBSD compatibility helper suite: 17 tests, 1 skipped, zero failures. The opt-in cached-upstream-source check was not run.
- Parsed manifest/lock metadata confirms exactly four own package version changes and no dependency lock changes.
- `git diff --check` passed.
- Full locked Rust workspace suite against the isolated pinned runtime: 1,339 passed, 10 ignored, zero failures, with serial UI tests.
- Matched debug Desktop host and UI built successfully. Private-Xvfb responsive acceptance passed four native resizes, three drawer cycles, and six tab selections, preserving panel identities and widths. Wide and 640-by-480 screenshots were visually inspected.
- Corrected runtime: 20 ACP tests, one NDJSON keyed/legacy regression, one legacy-wire protocol regression, and a locked native CLI binary build passed.
- No Cargo command used the dirty live runtime source, and no running application was reloaded.

Native CI, signing, package integrity, public GitHub release assets, website downloads, and installation acceptance are pending. This document does not claim that Desktop 0.3.1 is already available on GitHub or the website. No native or public-download gate is bypassed.

## Running development host

The package version bump changes the shared API crate identity. A matched host/UI pair requires a normal safe restart rather than a forced UI hot reload across this identity boundary. The user's live host, sessions, terminals, layout, and unsaved work are preserved. No live reload or restart is performed by this isolated release preparation.
