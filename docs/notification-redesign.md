# Notification redesign

Replaced the coaching popup and showcase feedback with a shared visual language:
neutral borders, modest shadows, compact spacing, and individual platform-aware
keycaps. Shortcut chords stay together when wrapping. Tips distinguish their
category, title, explanation, and keycap footer. A dedicated 28px close control
replaces the old click-anywhere dismissal. Cards occlude the content behind them.
Showcase feedback is a compact horizontal card instead of a two-story popup and
no longer runs a decorative entry animation. Existing expiry and coaching policy
are unchanged.

## Verification

- `cargo test -p jcode-desktop-ui --lib -- --test-threads=1`: 685 passed,
  8 ignored, 0 failed.
- The real GPUI coaching test verifies placement below the minimap, visible title
  and footer, body clicks preserving the tip, and the close control dismissing it.
- Existing showcase tests verify its toggle from workspace and composer focus.
- `python3 -m unittest discover -s scripts -p test_screenshot.py`: 9 passed.
- Real app captures on private Xvfb, both visually inspected:
  - `python3 scripts/screenshot.py target/notification-work/final-wide.png --notification`
  - `python3 scripts/screenshot.py target/notification-work/final-small-light.png --notification --no-build --size 640x480 --theme neutral-light`
- The offline-only `--notification` fixture shows both surfaces without depending
  on their brief production expiry windows.
- Invoked the running app's Ctrl+R-equivalent `--reload-ui` action. The live log
  confirmed activation of UI generation 6 after the final behavioral changes.

No active-desktop automation or compositor commands were used for visual testing.
