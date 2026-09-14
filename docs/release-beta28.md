# Jcode Desktop 0.1.0-beta.28

Release prepared September 14, 2026 (UTC), from committed Desktop source
`e9fe056` plus release metadata and runtime dependency updates. Uncommitted
local development changes are excluded.

## Highlights since beta.27

- Parchment is the default theme, with clearer workspace numbers and compact
  session tabs.
- Improved transcript scrolling, momentum, draggable scrollbars, and reading
  position preservation while responses stream.
- Clickable model/account controls, context-window and credential-quota meters,
  and elapsed time and token totals after responses.
- Tool output cards, earlier streamed tool intent, and compact progress summaries.
- Pasted screenshot previews, click-to-close expanded images, and smoother
  spatial panel navigation.
- Interrupted-session recovery, a sandboxed onboarding simulator, opt-in sounds,
  and lower rendering and session-list overhead.

## Build provenance and release gates

Both platform workflows pin the published Jcode runtime and SDK to
`56fb1f163aff19f5baeeda642c79f8499e597d0f`. The old beta.27 pin lacks the
`SessionRecovery` event used by the current Desktop. The release lockfile adds
`jcode-usage-types` to the harness API and SDK dependency lists required by the
new runtime.

The existing tag-triggered pipeline must pass macOS signing, notarization,
installation, and signed Sparkle update generation, plus Linux and Windows
package validation and launch smoke tests. Public promotion requires complete
platform assets and anonymous SHA-256 verification. The website consumes the
promoted manifest automatically, without a website source change.

This preparation record is not evidence of publication. CI results and the
live `https://jcode.sh/desktop/latest.json` channel establish delivery.
