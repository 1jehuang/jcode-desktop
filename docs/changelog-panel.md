# Desktop changelog presentation

The updates panel uses a compact monospace document rather than a large version
card. **Highlights** shows the installed release's editorial headline and named
sections. **Full changelog** retains the complete embedded commit history with
inline version/date headings, hanging bullets, and newest-first grouping.
The panel has only these two views, including in development builds. Build
diagnostics remain available in the development footer tooltip.

## Editorial source

`CHANGELOG.md` is bundled offline. Each release starts with
`### Jcode Desktop X.Y.Z`, optionally followed by a one-line prose headline.
`#### Themes`, `#### Highlights`, `#### Improvements`, and `#### Fixes` contain
bullet lists. Older unsectioned bullets are treated as highlights. Unknown
sections and download footers are not promoted to release highlights.

Only the installed Desktop version is selected. Development versions may use
their base release, explicitly labeled as a development preview. Other
prerelease channels require an exact match. Missing editorial notes do not
substitute another release or CLI notes. Commit history and unseen counts remain
independent of this curated overview.

## Verification

- `cargo test -p jcode-desktop-ui update_ --lib -- --test-threads=1`: 18 passed,
  including release/version matching, section parsing, headline selection, and
  bounded summaries.
- `cargo test -p jcode-desktop-ui changelog --lib -- --test-threads=1`: 12 passed,
  including local-only view navigation, slash command opening, singleton panels,
  focus/draft preservation, and reload behavior. One view test overlaps the
  preceding filter.
- `python3 scripts/screenshot.py target/changelog-highlights.png --changelog
  --theme graphite`: real offline app capture on private Xvfb, visually reviewed.
- `python3 scripts/changelog_acceptance.py target/changelog-native-dark`: native
  clicks, scrolling, keyboard navigation, independent view scroll positions, and
  Escape dismissal. PNG/OCR evidence and `acceptance.json` remain in the output
  directory. The same harness supports `--theme paper --width 1100` to exercise
  light-theme and responsive single-tab layout.

The native harness inherits no user desktop sockets or credentials. It uses the
existing offline fixture and never sends a provider request.
