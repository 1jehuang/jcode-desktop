# Desktop build versioning

Desktop separates its release/update identity from its development display
version. The dependency-free resolver is `crates/jcode-desktop-ui/build_version.rs`,
called by that crate's `build.rs`. Git is queried only at build time.

## Compile-time environment contract

| Variable | Ordinary source build | Explicit packaging build |
| --- | --- | --- |
| `JCODE_DESKTOP_VERSION` | Cargo root package version, currently `0.1.0` | Exact input `JCODE_DESKTOP_VERSION`, e.g. `0.1.0-beta.30` |
| `JCODE_DESKTOP_DISPLAY_VERSION` | Numbered semver, e.g. `0.1.700-dev` | Exact input `JCODE_DESKTOP_VERSION` |
| `JCODE_DESKTOP_GIT_HASH` | `git rev-parse --short HEAD`, or `unknown` | Same |
| `JCODE_DESKTOP_GIT_DIRTY` | Literal `true` or `false` | Same |
| `JCODE_DESKTOP_DEVELOPMENT` | Literal `true` | Literal `false` |

These are `cargo:rustc-env` outputs consumed with `env!`. Only
`JCODE_DESKTOP_VERSION` is also an input override. The presence of that input,
not debug assertions or Cargo's release profile, identifies a packaged build.
`cargo build --release` without the override remains a development build.
Packaging scripts already supply the stripped release version. The build script
preserves it without normalization, a `-dev` suffix, hash, or dirty marker.

Use `VERSION` for existing release/update comparisons and
package identity. **Never use the numbered display version to order updates.**
Use `DISPLAY_VERSION` for UI presentation, optionally formatting it like the
CLI: `v0.1.700-dev (abc1234, dirty)`. The `v`, hash, parentheses, and dirty marker
are not part of `DISPLAY_VERSION`. A dirty packaged build retains its exact
release version but still reports `GIT_DIRTY=true`.

The update panel, development footer, and build tooltip all use
the same numbered display version. Release history retains its exact release
tag boundaries and labels its unreleased group with the running display version.
`jcode-desktop --version` (or `-V`) reports `Jcode Desktop v0.1.N-dev (hash, dirty)`
without initializing a window, diagnostics, or the instance socket. In a
hot-reload session the panel reports the loaded UI build, while this command
reports the UI linked into the executable being inspected.

The update overview is an offline record of included changes, not an online
update check. Seeing no new commits does not claim that a newer release is
unavailable.

## Deterministic development numbering

The base comes from the root `Cargo.toml` `[package].version`, matching the CLI's
root-application source of truth. The parser supports the repository's literal
quoted version, with a UI `CARGO_PKG_VERSION` fallback if that root version is
unavailable (for example in a standalone UI source package).

For Cargo's base `MAJOR.MINOR.PATCH`, the development display version is:

```text
MAJOR.MINOR.(PATCH + offset)-dev
```

The addition saturates at `u32::MAX`, matching the CLI's patch arithmetic.
The tag lookup order is:

1. `refs/tags/desktop-vMAJOR.MINOR.PATCH` (Desktop's stable release namespace).
2. `refs/tags/vMAJOR.MINOR.PATCH` (CLI-compatible stable tag convention).
3. If neither exists, count **all commits reachable from HEAD**.

With a base tag, `offset` is `git rev-list --count <base-tag>..HEAD`, matching
`/home/jeremy/jcode/crates/jcode-build-meta/build.rs`. This counts all reachable
commits, including merged branches, not just first-parent commits. Annotated,
lightweight, and packed tags are supported, as are detached HEADs and worktrees.
Like the CLI, the range uses the exact base tag rather than the nearest tag.

The untagged fallback intentionally differs from the CLI's zero offset. Desktop
currently has only `desktop-v0.1.0-beta.*` version tags, so using zero would leave
all local builds numbered `0.1.0-dev`. Beta, alpha, RC, unrelated, and update-channel
tags are ignored. Creating another beta tag never resets the development number.
Introducing a stable base tag, changing Cargo's base, or rewriting history can
change/reset the number. It is a history-derived display identifier, not a global
monotonic counter.

If Git/history is unavailable, offset is zero, hash is `unknown`, and dirty
status defaults to `false` if it cannot be queried. Dirty detection includes
staged, unstaged, and untracked non-ignored paths. Shallow clones can only count
available history, so use a full clone with fetched tags for canonical development
numbers. Release versions are independent of history completeness.

No clock, persistent counter, file write, or dirty-tree change participates in
version numbering. Existing `BUILT_AT` and `BUILD_ID` timestamp metadata stays
unchanged for build age and hot-reload generation identity, not versioning.
Desktop's existing source/Git change watches remain in place so rebuilding after
a commit refreshes metadata. This differs from the CLI's intentionally relaxed
Git refresh policy for incremental-build performance.

## Tests without the GPUI dependency graph

Run from the repository root with Rust and Git installed and a configured Git
identity (fixture commits use that identity, never an invented one):

```sh
mkdir -p target
rustc --edition=2024 --test crates/jcode-desktop-ui/build_version.rs \
  -o target/desktop-build-version-tests
target/desktop-build-version-tests
```

The same fixtures are discovered by Cargo through `tests/build_version.rs`:

```sh
cargo test -p jcode-desktop-ui --test build_version
```

Fixtures are isolated temporary Git repositories and are deleted on completion.
They cover stable tag precedence, annotated/packed tags, beta-independent
numbering, nonzero base patches, determinism, dirty status, exact release
preservation, unavailable HEAD, worktrees, detached HEAD, merges, and saturation.

UI and command checks:

```sh
cargo test -p jcode-desktop-ui --lib build_info::tests
cargo test -p jcode-desktop-ui --lib update_notes::tests
cargo test -p jcode-desktop-ui --lib changelog::tests
cargo test -p jcode-desktop-ui --lib update_views_switch
cargo test -p jcode-desktop --test version
python3 scripts/screenshot.py target/ui-review.png --changelog --transcript empty
python3 scripts/accept-changelog.py target/changelog-version-accept
```

Native acceptance checks the visible numbered version against `--version`,
view navigation, keyboard controls, startup acknowledgement, and draft/session
preservation on a private Xvfb display. Use fresh artifact paths for each run.
