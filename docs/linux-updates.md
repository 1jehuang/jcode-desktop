# Linux desktop updates

`/update` is a **local desktop command**. It never sends a prompt to a model and
never invokes the standalone Jcode CLI's updater.

## Managed user installation

On x86_64 Linux, the supported archive layout is:

```text
~/.local/bin/jcode-desktop -> ../opt/jcode-desktop/<version>/jcode-desktop
~/.local/opt/jcode-desktop/<version>/
  jcode-desktop
  jcode
  jcode-harness-api-bridge
  jcode.desktop
  jcode.png
```

Absolute launcher symlinks work too. This includes the beta24 installation on
the development laptop. The updater detects the **running executable**, not the
working directory or the source location embedded when the package was built.

`/update` checks the public `1jehuang/jcode-desktop-releases` repository's
`desktop-latest/latest.json`. For a newer semantic version it downloads that
immutable `desktop-v<version>` tag's Linux tarball and `SHA256SUMS-linux`, verifies
SHA-256, and extracts only the five expected regular files. Paths, symlinks,
hardlinks, device files, duplicates, missing files, excessive sizes, invalid gzip,
and unsafe install-directory permissions are rejected. HTTPS, connection and
request timeouts, and a nonblocking per-install lock bound background work.

The bundle is staged on the installation filesystem, then installed without
replacing an existing version. The desktop launcher is switched atomically. Old
versions remain available for rollback. The running desktop and its sessions are
**not terminated**. The result appears in the conversation and the status chip.
Quit and reopen Desktop when ready to activate it. Standalone CLI launchers and
other application files are not rewritten. The new Desktop resolves its bundled
CLI and bridge beside its new executable.

A checksum from the same HTTPS release origin protects against corrupt or
incomplete downloads. It is **not an independent publisher signature** like
macOS Sparkle's Ed25519 signature. The GitHub release publisher remains trusted.
There are no automatic periodic Linux checks and no automatic restart.

## Other installations and source development

- **System or `.deb` installation:** use the original package installer or package
  manager. `/update` refuses to overwrite `/usr/bin`, escalate privileges, or
  prompt for a password.
- **Unmanaged archive or another architecture:** use the original installer.
  The command reports why the detected layout is unsupported.
- **Source executable under this checkout's `target/`:** `/update` requests the
  host's existing **Ctrl+R rebuild-and-reload** action, respecting
  `--no-hot-reload`, `--hot-reload`, and `JCODE_DESKTOP_UI`. It rebuilds the current
  checkout, not a downloaded release. It does **not** fetch, pull, reset, stash,
  or modify Git changes. Fetch/review source changes separately. The response
  distinguishes an acknowledged rebuild request from a completed activation.
  Build and activation results are in the desktop log. Source builds launched
  with hot reload disabled must be relaunched with `--hot-reload` first.

Failures stop the busy state and show an actionable error. `/update` can be
retried. If a version was installed but a later launcher switch failed, the error
identifies the retained version. Never delete an existing version merely to
silence that error. Inspect it or restore the launcher deliberately.

## Verification

```sh
cargo test -p jcode-desktop-ui --lib updates::
cargo test -p jcode-desktop-ui --lib update_command_submits -- --test-threads=1
cargo test -p jcode-desktop-ui --lib the_update_chip -- --test-threads=1
JCODE_DESKTOP_VERSION=0.1.0-beta.24 cargo build --release -p jcode-desktop -p jcode-desktop-ui
python3 scripts/screenshot.py target/ui-review.png --binary target/release/jcode-desktop --no-build
python3 scripts/accept-linux-update.py target/linux-update-current
python3 scripts/accept-linux-update.py target/linux-update-download --version 0.1.0-beta.23 --expect-version 0.1.0-beta.24
```

Updater tests use temporary installations and archives. They cover supported and
unsupported layouts, version ordering, checksum parsing/integrity, extraction
attacks, locking, launcher changes, atomic activation, rollback preservation,
source opt-out handling, and the real host socket protocol. The prompt test
submits `/update` through GPUI and exercises success and failure result rendering
with deterministic platform callbacks, without contacting the model or modifying
the user's installation. These tests do not simulate a signed release publisher
or claim a live in-place update occurred.

The native acceptance harness uses a private Xvfb display, isolated HOME, fixture
sessions, the real built application, native keyboard input, and OCR of the
rendered result. It contacts the public release channel. The default case checks
beta24 is current without changing its launcher. The second invocation downloads
and stages the actual beta24 archive from a temporary beta23 layout, checking all
bundle files, the atomic launcher selection, old-version retention, and that the
original app stays running. Update the version arguments when the public channel
advances. Neither run touches the user's live desktop or installed launcher.

## Observed acceptance on 2026-09-07

- Final updater tests: **29 passed**. Full UI suite: **617 passed, 7 ignored**,
  clean exit. An initial full-suite teardown abort exposed real Gmail workers in
  unit tests. Test-only Gmail isolation and a regression fixed it without
  changing production Gmail behavior.
- Optimized host and UI plugin builds passed. Both standard fixture PNGs and the
  native update-success PNG were read and visually inspected.
- `target/linux-update-current-3/evidence.json`: native `/update` reached the
  public beta24 channel, displayed already-current status, and kept the launcher
  and desktop process unchanged.
- `target/linux-update-download/evidence.json`: native `/update` downloaded and
  verified the actual beta24 release, installed all five bundle files from a
  temporary beta23 layout, switched its launcher, retained the old version, and
  did not terminate the process. `result.png` shows the quit-and-reopen result.
- Native acceptance initially exposed a source-detection bug for managed test
  installs nested beneath `target/`. Detection now accepts only Cargo's normal
  profile/target-triple output shapes, with a regression test.
- The actual installed beta24 desktop binary was atomically replaced with the
  validated local build. Its original was preserved as
  `jcode-desktop.before-linux-update-20260907T013044Z` in the same version folder.
  Bundled CLI and bridge hashes remained unchanged. New desktop SHA-256:
  `5128a6951452f3bc5e920350aa400b1afbfbbba04d458cfff34f4acb18b8899d`.
- **Live activation remains user-controlled:** original PID 3901 was left
  running on its old binary. It has hot reload disabled and no crash-recovery
  checkpoint, so forcing a restart could lose drafts/layout. Save any drafts,
  quit Desktop fully with **Super+Shift+Q**, then reopen from the normal launcher
  or `~/.local/bin/jcode-desktop`. Merely closing the window may hide rather than
  quit the process. `/update` becomes available on that next launch. No safe
  state-preserving live relaunch is claimed.
