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

## Local preflight

- Tested an isolated export of committed Desktop source against the exact pinned
  runtime, not the dirty development checkout.
- Release publication/package Python tests: 26 passed.
- Linux/macOS packaging and macOS verification shell syntax checks passed.
- Final locked serial workspace suite: 866 passed, 9 ignored, zero failures.
- Updated three image fixtures for the SDK's optional history-message index.
- Fixed a real narrow-footer flex bug that reduced model/login controls to zero
  width. Responsive acceptance covers 240x240 through 800x600, long model/account
  labels, and long drafts, with clipping and nonoverlap assertions.
- Corrected two stale activity-row test expectations while preserving exact
  geometry, no-scroll, title-stability, and status-state checks. All original
  failures were reproduced individually before correction, and the corrected
  cases also pass with an empty Desktop config.

These local checks do not replace the platform packaging and public-install
acceptance gates. Unrelated local staged and unstaged work remains excluded.

## Actions artifact quota recovery

The immutable beta.28 tag is `ddad95424cd098e82cc76ca91c99742a6f654927`.
Original macOS run `34804105441` passed universal packaging, Developer ID
signing, notarization, installed-app checks, and signed Sparkle appcast
creation. At 04:22 UTC on September 14, its Actions artifact transfer
failed because the account's artifact-storage quota was exhausted. The run
failed and never published the private release. Its earlier passing checks are
not evidence that the public release was delivered.

The concurrent Linux/Windows run `34804105442` was stopped because its required
Actions artifact transfers would hit the same quota. Recovery must rebuild the
same immutable Desktop and runtime sources, transfer packages through a private
draft GitHub release instead of Actions artifacts, and retain all signing,
notarization, package, and launch checks. Only a complete, verified platform set
may leave draft state and enter the unchanged public publisher. No billing
settings or existing artifacts are changed. The recovered Sparkle build number
must exceed beta.27's build 46.


## Publication verified, September 14 at 08:12 UTC

Recovery run `34806079844` passed all three platform jobs, including macOS
Developer ID signing, notarization, installed-app checks and signed Sparkle
archive generation, Linux X11/Wayland smoke tests, and Windows package/launch
checks. Each job uploaded its verified assets to the private beta.28 draft.
The final publication job was refused before runner startup because of an
account payment/spending-limit restriction. The overall recovery run remains
failed, not green.

Publication was completed locally using the unchanged scripts exported from
immutable Desktop commit `ddad954`. Before publication, the recorded platform
job and required step successes were rechecked, the 26 immutable Python tests
passed, and the downloaded complete package set passed package and checksum
validation. The private prerelease was then published and the existing public
publisher uploaded only its explicit binary/metadata allowlist. It verified all
nine anonymous origin downloads before promoting the website channel.

A separate fresh download through every website-owned asset URL verified all
nine exact byte lengths and SHA-256 hashes. `/desktop/latest.json` matches the
validated beta.28 manifest. `/desktop/appcast.xml` matches its manifest hash,
retains its Ed25519 signature, targets the public beta.28 archive, and carries
Sparkle build 47 (newer than beta.27 build 46). Machine-readable results are in
`release-beta28-website-verification.json`.

### Remaining acceptance limitation

Public macOS installed-package acceptance was attempted in runs `34821509497`
and `34821510568`. Both were refused before any test step ran by the same GitHub
account payment/spending-limit restriction. This is not a passing public-install
test. The exact published macOS package had already passed installation and
notarization checks in the recovery build, and its public bytes were verified,
but a fresh quarantined install from the public URL remains unverified. No
billing settings were changed and no existing artifacts were deleted.
