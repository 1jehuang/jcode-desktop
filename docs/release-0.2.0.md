# Jcode Desktop 0.2.0 release

## Frozen source and scope

- Base public-development branch: Desktop `2509ae9e7ab50a10c989e7ea60dcaa5c72c89b6b`.
- Current committed Desktop work integrated from `7d908bfa32a1823820961137603ce18f0990bd04` (`fix/cloud-startup-and-local-models`).
- Runtime and SDK: `91c0bdda3884952bd2eba2e5cfc78fbed1281495`, retained on runtime branch `release/desktop-0.2.0-runtime`.
- Intended release tag: `desktop-v0.2.0`.
- All four Desktop package manifests use `0.2.0`.

The release was prepared in an isolated clone. Uncommitted and untracked work in the user's Desktop and runtime development checkouts is excluded and preserved. The integrated committed work includes streaming voice, independent windows, image panels, and current sidebar and editing behavior that had not yet reached Desktop main.

## Release pipeline corrections

Recent beta.33 builds failed before compilation because the old runtime pin lacked the `voice-capture` feature required by Desktop. All native platform workflows now use the same current committed runtime.

The stable release must be marked non-prerelease in both the private build repository and the public binaries-only repository. Linux/Windows/FreeBSD upload waits for the matching complete macOS assets, rather than assuming every release is a beta. A later beta must not displace a promoted stable website channel.

macOS workflow run numbers were checked before release: the last macOS build was 52, while the live signed beta.28 feed is build 47. The next normal run therefore remains newer for Sparkle updates. Signing, notarization, package verification, native smoke checks, complete platform assets, and anonymous download hash checks remain required.

## Publication status

Preparation is not publication. Final delivery requires successful platform workflows, public release promotion, `https://jcode.sh/desktop/latest.json` reporting `desktop-v0.2.0`, exact website-served asset hashes, and the public macOS installation acceptance workflow.

## Local preflight completed

- Full locked Rust workspace suite: 1,211 passed, 9 ignored, zero failed (40 host, 1 version integration, 8 API, 6 motion, and 1,156 UI tests).
- Release publication/packaging Python suite: 65 passed, 5 skipped because local Linux package tooling is absent. CI retains real package verification.
- macOS voice entitlement and microphone usage metadata: 2 tests passed.
- Exact pinned runtime: locked `cargo check` passed for the CLI and harness bridge binaries.
- Exact corrected Desktop app and reloadable UI built successfully with the explicit `0.2.0` version. The real app passed private-Xvfb responsive acceptance: four resizes, three sidebar drawers, six tab selections, preserved identities, widths, and focus. Wide and narrow screenshots were inspected.
- Packaging shell syntax and `git diff --check` passed. Workspace formatting check still reports pre-existing formatting differences in Desktop and adjacent runtime sources; these were not folded into the release change.

The first full suite exposed a real unfinished-work card click panic caused by re-entering a borrowed Panel. The card now calls its captured workspace opener without a Panel update lease, and routes through the common activation method. The unchanged original regression and a new existing-session/focus regression pass. Stale gesture tests now exercise supported upward navigation and explicitly verify that downward swipes cannot navigate. Sidebar/resize acceptance uses the current compact controls.

Platform workflow links and public download evidence will be appended after they are observed.
