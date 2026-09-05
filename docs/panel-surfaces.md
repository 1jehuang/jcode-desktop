# Connected folder panel surfaces

Panels share adjoining edges and a common bottom baseline. The workspace canvas remains visible above and below them. Focus is conveyed by a raised top edge and surface color, not an outline. Overview cards also use surface color instead of a focus ring.

## Acceptance checks (2026-09-05)

| Requirement | Check | Observed result |
| --- | --- | --- |
| Leave background space | GPUI geometry test at 800×600 and 1440×1000, sidebar shown/hidden | Active top and all bottom gaps are 16px. Inactive top gap is 24px. |
| Keep folders connected | Same test, focusing each of three panels through workspace keyboard bindings | Adjacent panel edges touch and bottom edges remain aligned. |
| Raise and recolor the active folder | Actual app on private Xvfb, three panels, native X11 click from middle to left | Public app state changes from `focus=1` to `focus=0`. The raised surface moves to the left panel. Rendered surface color changes from `#302b27` to `#25221f`, with the middle reverting to `#302b27`. |
| Remove the panel focus ring | Before/after screenshots inspected, edge pixels sampled | Center edge and interior have identical colors in both focus states. No panel outline is visible. Canvas pixels remain `#1c1a18`. |
| Preserve usable transcript rendering | Scroll-aware demo transcript regression | All expected item shapes paint when revealed in the shorter, virtualized viewport. |
| Deliver to running desktop | App rebuild-and-reload command and live process log | Rebuild succeeded and UI generation 9 activated. |

Reproduce the native focus check without affecting the user's desktop:

```sh
cargo test -p jcode-desktop-ui folder_panels_keep_canvas_space
python3 scripts/screenshot.py target/ui-review-folder-panels.png --panels 3
python3 scripts/screenshot.py target/ui-review-folder-click.png --panels 3 --focus-panel 0 --no-build
python3 -m unittest discover -s scripts -p 'test_screenshot*.py'
```

These screenshots run the real application and platform input path with offline session fixtures. They validate folder styling and focus behavior, not live SDK latency or sidebar hover performance. The two screenshot isolation tests pass.

The final integrated UI suite reports **296 passed, 3 failed, 6 ignored**. Remaining failures also occurred before this styling work: `email_inbox_moves_when_the_user_scrolls`, `restored_scroll_is_not_replaced_when_history_reattaches`, and `a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`. The suite is not claimed to be green.
