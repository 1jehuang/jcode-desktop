# Desktop changelog presentation

The updates panel has a one-row header (title, **Highlights** and **All
changes** pills, Close) and a 680px reading column in a proportional font
(Inter on Linux, Segoe UI on Windows, the system font on macOS).

**Highlights** shows the installed release:

- The editorial headline, with the running build on one dim line beneath it.
  The version appears only there.
- Each `#### Themes` bullet as a numbered paragraph, followed by its screenshot
  on a tinted mat when `assets/changelog/<version>/N-*.png` exists.
- `#### Highlights`, `#### Improvements`, and `#### Fixes` as plain one-line
  bullets under small section labels.
- Recent commits and a **See all changes** pill.

**All changes** keeps the embedded commit history grouped by release, newest
first.

## Editorial source

`CHANGELOG.md` is bundled offline. Each release starts with
`### Jcode Desktop X.Y.Z`, optionally followed by a one-line prose headline.
`#### Themes`, `#### Highlights`, `#### Improvements`, and `#### Fixes` contain
bullet lists. Older unsectioned bullets are treated as highlights. Unknown
sections and download footers are not promoted to release highlights.

Only the installed Desktop version is selected. Development versions use their
base release's notes and screenshots. Other prerelease channels require an exact
match. Missing editorial notes do not substitute another release or CLI notes.

## Theme screenshots

`build.rs` embeds every PNG under `assets/changelog/` with its pixel size, so the
panel reserves the right height before decoding. Shot `N-*.png` belongs to the
Nth Themes bullet. Render them reproducibly with
`python3 scripts/changelog_shots.py X.Y.Z`. See "Theme screenshots" in
`docs/release-orchestration.md` for the release steps.

## Verification

- `cargo test -p jcode-desktop-ui --lib -- changelog update_notes`: version
  matching, section parsing, headline selection, screenshot lookup by theme
  number, local-only view navigation, and reload behavior.
- `python3 scripts/screenshot.py --changelog --size 1440x1600
  target/changelog.png`: real offline app capture on private Xvfb.
- `python3 scripts/changelog_acceptance.py target/changelog-native-dark`: native
  clicks, scrolling, keyboard navigation, and Escape dismissal.

The harnesses inherit no user desktop sockets or credentials and never send a
provider request.
