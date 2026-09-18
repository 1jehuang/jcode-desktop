# Jcode Desktop 0.1.0-beta.30

## Frozen release source

- Desktop: `7d04fc9093a3de485eb88b1814026dc3d978504c`.
- Source tree: `31267a775bf96f9a295adb5caf014968f49577d7`.
- Runtime and SDK, in all three platform workflows:
  `7dd973912a7acb8792c7d6caf9254270b0dd1b02`.
- Tag: `desktop-v0.1.0-beta.30`.

The release uses committed source only. Unrelated staged, unstaged, and
untracked development work is excluded. Every tracked file in the isolated
preflight export was compared with the release commit before tag publication.
Concurrent development can continue on main without changing this release.

## Why beta.29 was not published

The September 18 beta.29 builds revealed two compile errors: an old runtime
pin lacked `jcode_base::account_login`, and workspace animation polling still
called the removed sidebar roller animation API. The macOS, Linux, and ARM64
Windows compiler logs reproduced these errors. The remaining beta.29 build
jobs were cancelled rather than spending runner time on a known-broken source.
The published beta.29 tag was not moved, and no beta.29 binaries were promoted.

The current runtime pin supplies account login and cancellable OAuth callbacks.
The obsolete animation poll was removed. Tests that still used removed roller
arrows and animation settling now exercise the actual section menu. Existing
page, action, tutorial, panel geometry, and canvas-stability assertions remain
covered.

## Local preflight

Validation used an isolated source export and the exact pinned runtime, with
copy-on-write build caches isolated from the development checkout.

- Locked full Rust workspace suite: **934 passed, 9 ignored, 0 failed**.
  This includes 24 host, 5 API, 4 motion, and 901 UI tests.
- Release publication and packaging Python suite: **53 passed, 5 skipped**.
  The skipped tests require Linux package tooling unavailable on this host.
- Linux/macOS packaging and macOS verifier shell syntax checks passed.
- The real Desktop app and reloadable UI built successfully with `--locked`.
- Private-Xvfb native section-menu interaction acceptance passed, including
  hover, scroll selection, dismissal, and preserved focus. Its screenshot was
  inspected.
- Private-Xvfb account-sign-in interaction acceptance passed, including welcome,
  waiting, skip, and Settings re-entry, without network access.
- The running development application's Ctrl+R rebuild-and-reload path was
  requested through its host IPC. A new UI generation activated successfully.

These checks are not a substitute for platform signing, notarization, packaging,
launch smoke tests, and verified anonymous public downloads.

## Platform builds and publication

Tag publication started these GitHub Actions runs on September 18, 2026 UTC:

- macOS: https://github.com/1jehuang/jcode-desktop/actions/runs/35313789480
- Linux, Windows, and FreeBSD:
  https://github.com/1jehuang/jcode-desktop/actions/runs/35313789572

At preparation time these builds are pending. This document does **not** claim
that beta.30 is publicly available. Successful platform gates, public promotion,
`https://jcode.sh/desktop/latest.json`, verified asset hashes, and public macOS
installation acceptance establish final delivery.
