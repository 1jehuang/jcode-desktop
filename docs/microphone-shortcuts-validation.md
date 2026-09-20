# Microphone control and shortcuts (2026-09-20)

The composer voice control is now a 16px microphone in a 28×22px button, rather
than the Voice/Copilot text pair. Active phases use the accent color. Hover shows
the current action and platform shortcut first: Copilot key on non-macOS,
Command+Shift+M on macOS. Ctrl+Shift+V remains a composer fallback.

## Verification

- `cargo test -p jcode-desktop-ui voice --lib`: 29 passed. Includes actual hover
  tooltip visibility, platform shortcut text, icon bounds at five widths,
  click/fallback-key routing, Copilot repeat suppression and recording ownership.
- `python3 scripts/screenshot.py target/microphone-review-visible.png --no-build`
  after a current production-code debug build: passed. Image visually inspected.
  Initial inspection caught an invisible SVG without its explicit theme tint.
  Corrected rendering visibly shows the microphone beside Ready.
- The configured local Linux global binding already maps Super+Shift+F23 to
  the installed launcher with `--toggle-voice`, with repeat disabled. Static XKB,
  launcher, private IPC and workspace action routing were inspected. No compositor
  command, physical keyboard monitoring or microphone recording was used.
- `jcode-desktop --reload-ui` uses the same rebuild path as Ctrl+R. The running
  Linux host completed its release rebuild and activated UI generation 4.

The macOS host registers Command+Shift+M through Carbon and forwards a typed voice
command to the existing workspace action. A release-reset latch suppresses held
key repeats. Registration conflicts leave the existing activation shortcut and
in-app voice binding intact. Native macOS registration and a physical Copilot
press still need platform/hardware trials, not claimed verified on Linux.
