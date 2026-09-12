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
