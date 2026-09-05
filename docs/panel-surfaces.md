# Two connected folder surfaces

The workspace uses two structural background tones, independent of message/code styling:

- **Selected page (`PANEL_BG`, warm-neutral `#25221f`)**: selected sidebar session, the 12px connector gutter, surrounding page, and focused panel are one connected surface.
- **Backing sheet (`HEADER_BG`, warm-neutral `#302b27`)**: sidebar background, inactive panels, and the 16px bottom connector are one connected surface.

The gutter lets a selected sidebar tab reach the focused panel even when inactive panels lie between them. The bottom connector independently joins all inactive panels back to the sidebar. There is no third canvas tone or panel/selected-session outline. Session hover uses the existing page tone instead of introducing another structural shade. Inputs, code blocks, text, and status indicators retain their own semantic styling.

## Tight visual feedback loop

Render the real current app with rich offline content on private Xvfb/Openbox:

```sh
python3 scripts/screenshot.py target/two-tone-review.png --panels 3
python3 scripts/screenshot.py target/two-tone-left.png --panels 3 --focus-panel 0 --no-build
```

The first command builds current code. Use `--no-build` only when the binary is current. Inspect both PNGs before further edits. This does not open windows or move focus on the user's display. The reviewed captures for this change are `target/ui-review-two-tone-1.png` and `target/ui-review-two-tone-left.png`.

## Real SDK/runtime acceptance

```sh
cargo build -p jcode-desktop
python3 scripts/accept-folder-panels.py target/two-tone-acceptance
```

Use a fresh output directory. Dependencies: the installed `jcode` binary, Xvfb, Openbox, xdotool, ImageMagick, Mesa lavapipe, and Python Pillow.

The driver disables screenshot fixtures and launches an isolated real daemon, harness API, and desktop. Native keys create three SDK-backed sessions. A separate public API client attaches, names, and detaches from those sessions. The app's supported five-second catalog refresh brings their metadata into the actual sidebar. Keyboard, panel clicks, and overview selection then transfer focus.

The supported `--provider jcode` startup initializes lazily, so these lifecycle operations require neither credentials nor inference. No user credentials are inherited, no model request is sent, and the user's daemon and desktop sockets are never used. Process groups are terminated on success or failure.

## Requirement-to-evidence map (2026-09-05)

| Requirement | Concrete check | Observed result |
| --- | --- | --- |
| Only two structural tones | Real-session raster samples through panel bodies, gutter, sidebar, and footer | Page is `#25221f`, backing sheet is `#302b27`. The former canvas tone is absent from these surfaces. |
| Selected sidebar tab connects around to focused panel | Flood-fill the actual page-color pixels from the gutter, then inspect selected-tab and focused-panel pixels | Both belong to the same connected component for middle, left, and right focus. |
| Sidebar background connects to every inactive panel | Flood-fill actual backing-color pixels from the sidebar, then inspect footer and inactive panels | All belong to the same connected component in every tested focus state. |
| Preserve folder spacing without focus rings | GPUI geometry regression at 800×600 and 1440×1000, sidebar shown/hidden, focusing each panel | 12px gutter when sidebar is present, 16px top/bottom panel spacing, 8px inactive inset, shared bottom edge touching the backing connector. Raster scan finds no outline or third-tone gap between panel bodies. |
| Keep focus and overview interaction working | Real native keys, clicks on left/right panels, overview hover, overview-card selection | Focus follows `1 → 0 → 2 → 0`; four resulting strip frames pass both connectivity checks. Overview hover and return to the selected folder also pass. |
| Keep capture CLI correct | Updated native click coordinates include the gutter; public argument-error subprocess tests | Five screenshot tests pass, covering isolation and five invalid-argument cases. Rich-content native focus capture also succeeds. |
| Deliver the updated view | App rebuild-and-reload command plus process log | Build succeeded and the running app activated the updated UI generation. |

The complete non-fixture run passed in 31.2 seconds. Its six screenshots and state files are in `target/two-tone-real-4/`. The full-width page connectivity was checked on actual rendered pixels, not inferred from source inspection or replaced with synthetic session data. These checks do not claim to measure user-workspace hover latency or Wayland-specific behavior.

## Measured improvement over the previous design

Compared actual 1800×1000 native app captures from `target/folder-real-5/middle-focused.png` (before) and `target/two-tone-real-4/middle-focused.png` (after). Both came from real SDK-backed session runs, not UI fixtures.

| Observable property | Before | After |
| --- | --- | --- |
| Distinct tones sampled across sidebar, page, focused panel, inactive panel, and footer | 3 | 2 |
| Sidebar backing pixels connected to the inactive panel beyond the focused panel | No | Yes |
| Focused-panel pixels connected to the left page connector | No | Yes |

These are pixel-value and flood-fill results, not subjective visual ratings. The former third tone `#1c1a18` was replaced by page tone `#25221f` above/alongside the panels and backing tone `#302b27` at the bottom connection. Machine-readable results are saved in `target/two-tone-before-after.json`.

The subsequent full UI regression run reported **301 passed, 3 pre-existing failures, 6 ignored**. The remaining failures are `email_inbox_moves_when_the_user_scrolls`, `restored_scroll_is_not_replaced_when_history_reattaches`, and `a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`, all observed before this styling change. All two-tone, sidebar geometry, and tutorial geometry checks pass. The full suite is not claimed to be green.
