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

The stable release must be marked non-prerelease in both the source/build repository and the public binaries-only repository. The release stays draft until Linux/Windows/FreeBSD gates and the macOS workflow for the exact same tag and commit have succeeded, followed by complete asset validation. A later beta must not displace a promoted stable website channel.

macOS workflow run numbers were checked before release: the last macOS build was 52, while the live signed beta.28 feed is build 47. The next normal run therefore remains newer for Sparkle updates. Signing, notarization, package verification, native smoke checks, complete platform assets, and anonymous download hash checks remain required.

Tagged builds stage their verified assets directly in a draft release instead of depending on the Actions artifact quota that blocked beta.28. Exact tag, draft status, and prerelease metadata are checked before uploads. The source repository is now public, but draft assets remain unpublished. No repository visibility, billing configuration, or pre-existing artifacts were changed. Manual branch validation still uses Actions artifacts.

## Publication status

Preparation is not publication. Final delivery requires successful platform workflows, public release promotion, `https://jcode.sh/desktop/latest.json` reporting `desktop-v0.2.0`, exact website-served asset hashes, and the public macOS installation acceptance workflow.

## Local preflight completed

- Full locked Rust workspace suite: 1,222 passed, 9 ignored, zero failed (40 host, 12 version integration, 8 API, 6 motion, and 1,156 UI tests).
- Release publication/packaging Python suite: 70 passed, 5 skipped because local Linux package tooling is absent. CI retains real package verification.
- macOS voice entitlement and microphone usage metadata: 2 tests passed.
- Exact pinned runtime: locked `cargo check` passed for the CLI and harness bridge binaries.
- Exact corrected Desktop app and reloadable UI built successfully with the explicit `0.2.0` version. The real app passed private-Xvfb responsive acceptance: four resizes, three sidebar drawers, six tab selections, preserved identities, widths, and focus. Wide and narrow screenshots were inspected.
- Packaging shell syntax, actionlint for all four release workflows, and `git diff --check` passed. Workspace formatting check still reports pre-existing formatting differences in Desktop and adjacent runtime sources; these were not folded into the release change.

The first full suite exposed a real unfinished-work card click panic caused by re-entering a borrowed Panel. The card now calls its captured workspace opener without a Panel update lease, and routes through the common activation method. The unchanged original regression and a new existing-session/focus regression pass. Stale gesture tests now exercise supported upward navigation and explicitly verify that downward swipes cannot navigate. Sidebar/resize acceptance uses the current compact controls.

Platform workflow links and public download evidence will be appended after they are observed.

## Native release builds started

On September 20, 2026 at 09:44 UTC, `desktop-v0.2.0` was pushed at immutable commit
`d420a1933a5a686fa9baf4c8c9c979060c608755` together with the reviewed source on main.

- macOS universal: https://github.com/1jehuang/jcode-desktop/actions/runs/35503143057
- Linux x86_64/ARM64, Windows x86_64/ARM64, FreeBSD x86_64: https://github.com/1jehuang/jcode-desktop/actions/runs/35503143193

The build release was observed as a draft with `isPrerelease: false`. It had no
assets yet. This is evidence of release initiation, not public availability.

## Native gate failure and 0.2.1 recovery

The Windows ARM64 build failed at 09:57 UTC before packaging. The experimental
cloud helper used Unix process groups and `waitid` unconditionally, which do not
compile on Windows. The publication gate correctly kept 0.2.0 draft and left the
website on the last verified release.

The original `desktop-v0.2.0` tag is retained unchanged. The corrected stable
candidate is 0.2.1, still in the requested 0.2 series, rather than rewriting an
already pushed source tag. All platform gates and website verification must run
again for that exact new tag before it is considered released.

The helper now retains Unix process-tree cleanup only on Unix. Other platforms
return an explicit unsupported error and never start a cloud session. The Unix
`waitid` call converts the child PID to `libc::id_t`, also fixing FreeBSD's wider
argument type. All 31 focused Unix cloud tests pass. The stable changelog UI test
now verifies that development-only build details are absent, while retaining
both stable and development keyboard/click navigation checks.

Recovery preflight passed with explicit `JCODE_DESKTOP_VERSION=0.2.1`: the full
locked workspace suite passed all 1,222 tests with 9 ignored, using
`--test-threads=1` to isolate wall-clock UI animation assertions. The initial
parallel run exposed the stable/development changelog fixture assumption and
six timing-sensitive animation failures. Those animation assertions passed
unchanged in the serial suite. Both explicit stable and unset-version changelog
test runs passed. The release Python suite again passed 70 tests with 5 local
package-tooling skips.

At 10:05 UTC, `desktop-v0.2.1` was pushed at immutable commit
`4c5495f85a03671b20a00a108f10e6dbe041584d`.

- macOS universal: https://github.com/1jehuang/jcode-desktop/actions/runs/35504110073
- Linux/Windows/FreeBSD: https://github.com/1jehuang/jcode-desktop/actions/runs/35504110181

After both original Windows jobs failed with the same compile diagnostics, the
remaining unpublishable 0.2.0 workflows were cancelled at 10:07 UTC to avoid
wasting runner time and unblock the serialized macOS recovery build. The old
tag and draft were preserved. The 0.2.1 workflows must independently satisfy
every native publication gate. Public availability has not yet been verified.

## FreeBSD dependency compatibility recovery

The 0.2.1 Linux ARM64 job passed packaging and both native X11/Wayland launch
checks. FreeBSD job `106060890220` then failed in the pinned upstream GPUI
dependency: its queue module and public exports omit FreeBSD from their cfgs,
although `gpui_linux` requires those exports on FreeBSD.

The separately reviewed `freebsd-0.2.1-recovery.yml` keeps Desktop source
`4c5495f85a03671b20a00a108f10e6dbe041584d` and the runtime pin unchanged. It
uses a new, private-to-the-build Cargo home and the exact two-cfg compatibility
overlay documented in [FreeBSD GPUI compatibility](freebsd-gpui-compatibility.md).
Both pristine and patched dependency file hashes are pinned. No shared cache or
tagged application source is edited, and post-build verification rejects any
other tracked dependency change.

Recovery retains package verification, CLI launch, dynamic dependency checks,
and the full native FreeBSD graphical smoke. Upload refuses to overwrite assets
and redownloads them to compare exact bytes. Publication additionally requires
the original, exact macOS workflow and all four Linux/Windows matrix jobs to
succeed, plus full asset validation. The original known FreeBSD failure is the
only replaced gate. Release notes record the exact recovery recipe commit and
workflow run. Successful recovery is still required before public promotion.
