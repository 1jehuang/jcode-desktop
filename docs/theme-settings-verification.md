# Theme and Settings acceptance, 2026-09-05

Change: `24767ac` removes the floating top-right theme button and adds separate
Theme and Settings tabs to the existing scrollable sidebar navigation.

## Observed acceptance results

These checks drove the **real desktop executable through native X11 mouse
input**, using `scripts/screenshot.py`'s private Xvfb/Openbox display, offline
transcript, software renderer, and isolated HOME/XDG/Jcode configuration.
They did not manipulate the user's active desktop or real settings.

| Requirement or changed behavior | Action | Observed result |
| --- | --- | --- |
| Remove the floating theme button | Click its former top-right position | The 112 × 96 pixel corner region was unchanged and no config file was written. The GPUI interaction test also found no `theme-picker-button` element before or after theme selection. |
| Provide a Theme tab | Scroll the sidebar navigation and click Theme, then Neutral light | The actual canvas sample changed from RGB **(37, 34, 31)** to **(248, 247, 244)**. The private config contained `theme = "neutral-light"`. |
| Preserve saved theme selection | Terminate only the private fixture process and start a fresh process with the same private config | The new process rendered the identical light RGB **(248, 247, 244)**. No palette was selected programmatically. |
| Provide a separate functional Settings tab | Click Settings, then toggle the minimap Off → On → Off | Turning it on changed **32,133 RGB channel values** in the top-right minimap region. Turning it off restored that region **byte-for-byte**. |
| Settings shortcut-display control works | Toggle On → Off → On | The rendered toggle label changed, then its captured region returned **byte-for-byte** to the original state. |
| Navigation and shortcuts remain usable | Run GPUI interaction tests through tab scrolling, real clicks, and the keyboard binding | Theme cycling from the focused composer passed. Settings toggles, returning to Theme and Chat, active-tab/page geometry, and todos-card navigation passed. |

The result is concretely better for the request: the conversation canvas no
longer has the theme affordance in its corner, palette selection remains usable
and survives a process restart, and Settings has its own working controls rather
than a placeholder tab.

Local captures and machine-readable observations are retained under ignored
`target/theme-settings-acceptance/`, including `observations.json`, six transition
screenshots, and `final.png`. The local native verification driver is
`target/verify-theme-settings.py`. Its coordinates describe the inspected Linux
fixture, not a cross-platform automation API.

## Test and delivery evidence

- `cargo test -p jcode-desktop-ui --lib tab_ -- --test-threads=1`: **6 passed**,
  including the Theme and Settings interaction tests.
- `cargo test -p jcode-desktop-ui --lib unfinished_work -- --test-threads=1`:
  **1 passed** for opening the chat from its todos card.
- `cargo test -p jcode-desktop-ui --lib persisting_theme -- --test-threads=1`:
  **1 passed**, preserving unrelated shared settings and comments.
- Production build and `python3 scripts/screenshot.py target/ui-review.png`
  succeeded. Native acceptance above subsequently ran against the desktop
  executable with a 2026-09-05 02:59:05 -0700 modification time.
- The running desktop acknowledged the instance socket's Ctrl+R-equivalent
  rebuild/reload request. Its log confirmed **UI generation 2 activated**.
- Only the Theme/Settings changes were committed in `24767ac` and pushed.

## Limits

This is feature acceptance, not a claim that the entire concurrent working tree
is green. An earlier full-suite attempt reported separate email scrolling,
restored-scroll, and touchpad-reticle failures. A later full-suite rerun was
blocked during compilation by the concurrently edited `tutorial.rs` referencing
an undefined `heading`. Neither was counted as a successful full-suite run.
The requested Theme/Settings interactions were independently verified above.

No live Wayland performance improvement or cross-platform visual equivalence is
claimed. Settings controls are explicitly scoped to the current window. Only
palette selection is claimed to persist across a full application restart.
