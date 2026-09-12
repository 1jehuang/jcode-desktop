# Jcode Desktop 0.1.0-beta.27

Release candidate prepared September 12, 2026 (UTC), from desktop source
`10fc479` plus release metadata. This supersedes public beta.24. Tags beta.25
and beta.26 did not complete publication.

## Highlights since beta.24

- Native provider login and actionable model error recovery.
- SSH machines and persistent defaults for new local and remote sessions.
- Structured file-change review with syntax and intraline highlights.
- Native Mermaid diagrams, equation rendering, and zoomable image previews.
- Workspace grouping, improved session navigation, and nested swarm agents.
- More responsive panel creation, compact small-window layouts, and reduced
  transcript rendering work.
- Crash recovery and improved stream activity reconciliation after reconnect.

## Build provenance and release gates

The cross-platform workflows pin the bundled Jcode runtime and SDK to
`950e231445af43bbfeccfdc99f5f97256c7dc4b2`. Unlike the older runtime used by
beta.26, this pin includes the APIs required by the current Desktop code.

Publication uses the existing tagged-release pipeline. macOS requires Developer
ID signing, notarization, signed Sparkle updates, and installed-app checks.
Linux and Windows packages must pass package verification and launch smoke
tests. The public publisher validates the complete package set and anonymous
download hashes before promoting the website's `desktop/latest.json` channel.

Preparing this document does not assert that those release gates have passed.
The Actions runs and live public manifest are the publication evidence.

## Local preflight

- `python3 -m unittest discover -s tests -v`: 26 passed.
- `cargo test --workspace --lib --bins --locked -- --test-threads=1`:
  753 passed, 8 ignored, zero failures.
- Default parallel execution is not green: four transient animation/debug-selector
  tests failed in the first full run. All four passed individually, and the full
  serial run passed without source changes. Wall-clock animation completion and
  cached-paint debug selectors are sensitive to concurrent test scheduling. This
  release does not claim to fix parallel test flakiness.
- The current local release binary passed isolated Xvfb responsive-layout,
  native login-control, and model-picker interaction checks using
  `scripts/screenshot.py`. Login verification used offline fixtures, not real
  provider OAuth. Model verification covered local selection requests, not
  backend acknowledgement. These local checks do not replace packaged CI checks.
- Shell syntax checks passed for Linux/macOS packaging and macOS verification.
- The pinned runtime's SDK, render-core, and harness-api source matches the local
  runtime checkout used for the desktop unit tests.

## Cross-platform release recovery

The immutable release tag points to `74e07af14c99963864c948e72b1da53cd072544f`.
macOS run `34679178421` passed signing, notarization, installation verification,
and signed-update generation. Linux in run `34679178367` passed packaging,
checksums, and X11/Wayland launch checks.

The Windows job in the same run completed packaging at 08:39:02 UTC, package
verification at 08:39:07, launch smoke at 08:39:21, and artifact upload at
08:39:31 on September 12. All of those steps passed. Its post-job 2.4 GB cache
save then ran past the two-hour job limit. GitHub marked the job cancelled with
the annotation `The job has exceeded the maximum execution time of 2h0m0s`,
skipping the normal release upload despite all package acceptance steps passing.

Recovery uses only the already uploaded Linux and Windows artifacts from that
run, whose recorded source SHA matches the immutable tag. Before uploading the
missing assets to the existing prerelease, recheck each build/package/smoke/
artifact step's success, validate the downloaded packages, and verify their
SHA-256 checksums. Then run the unchanged public publisher, including its complete
platform set, signed appcast, anonymous download integrity, and public macOS
acceptance checks. Do not rebuild from current main or move the release tag.

The main workflow's timeout is increased to 180 minutes for subsequent releases
so successful cold Windows builds have time to save their cache. Package,
signing, and smoke gates are unchanged.
