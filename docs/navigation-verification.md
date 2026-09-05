# Navigation and minimap regression checks

## Bug and fix

Every UI activation installs the workspace key bindings again. GPUI retains
matching older bindings and tries them if an action propagates. Workspace
navigation had moved to capture-phase handlers, which do **not** consume actions
by default. Consequently, after hot reload, one physical shortcut could execute
several left/right or up/down motions. Previous-panel navigation could toggle
back to its starting point.

The seven workspace focus handlers now explicitly stop propagation, including
at strip boundaries. This keeps canvas navigation independent of the focused
composer or terminal without clearing the host's rebuild/rollback key bindings.
A pre-fix rendered regression reproduced `super-l` moving from slot 2 to slot 4
instead of slot 3 after repeated key registration. It passes with the fix.

The minimap also used the retained slot index as its focused flag after switching
to an empty strip. It now requires the panel to belong to the active strip, so
empty strips have neither a stale focused panel nor a focus pin.

## Repeatable checks

```sh
# Bound keys, all Linux left/right aliases, boundaries, reversals, previous
# focus, vertical movement, keyboard ownership and structured state.
cargo test -p jcode-desktop-ui navigation_keys_move_once_after_rebinding

# Actual rendered map rectangles/pin, clicking, reordering and empty strips.
cargo test -p jcode-desktop-ui navigation_map_tests

# Confirm the native checker rejects wrong focus, order, memory and map flags.
python3 -m unittest discover -s scripts -p 'test_accept_navigation.py'

# Real daemon + SDK sessions, native X11 keys, and two actual Ctrl+R reloads.
cargo build --workspace
python3 scripts/accept-navigation.py target/navigation-acceptance
```

The native runner allocates a private Xvfb display and private daemon/API sockets.
It creates four real sessions but never submits inference. It checks every panel
in both directions, boundaries, Ctrl+Tab aliases, first/last, previous focus,
empty-strip navigation and remembered focus before and after each reload.
It leaves `navigation.jsonl`, logs and `navigation.png` under the output path,
then cleans up all private processes. The output directory must not exist.
`--reloads 0` is a quick navigation-only run. The default is two reloads.

## Opt-in state format

Set `JCODE_DESKTOP_STATE=/path/to/state` when starting a test instance. Existing
human-readable lines are preserved. An added `navigation=<JSON>` line contains:

- `version`, currently 1.
- `active_row`, nullable `focused_slot`, nullable `keyboard_panel`.
- `minimap_visible` and `overview`.
- Every row's ordered panels, remembered focus, camera and camera target.
- Each panel's slot index, entity ID, session ID, width, focus and closing flags.

Slot indices describe the current order. Entity IDs identify panels during one
UI generation. Session IDs survive reloads. A nullable keyboard panel means no
panel's primary input target owns focus, which is expected on an empty strip or
when a different control owns focus. The dump is sampled after render-time camera
updates and only generated when this environment variable is set. It contains
session identifiers, not transcript text. A reader must tolerate partial file
writes. The native runner's `navigation_state` helper does so.

Rendered tests compare state to actual minimap geometry. The native runner
independently checks expected one-step destinations and focus ownership through
the platform key dispatch path. A correct state dump alone is not proof that the
map's pixels are correct, so both layers are necessary.

## Verified on 2026-09-05

- The key-rebinding regression failed before the fix and passed afterward. The
  expanded matrix passed all 68 keyboard steps, including six Linux alias pairs.
- All four rendered minimap regressions and all five Python checker tests passed.
- The integrated UI suite reported 309 passed, 3 failed and 6 ignored. The failures
  were the already documented `email_inbox_moves_when_the_user_scrolls`,
  `restored_scroll_is_not_replaced_when_history_reattaches`, and
  `a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`. The suite is not
  being reported as fully green.
- `python3 scripts/accept-navigation.py target/navigation-acceptance-4 --reloads 1`
  passed **41 state checkpoints** with four real SDK sessions, before and after
  a native Ctrl+R rebuild/reload. Session order and focus remained consistent.
  Artifacts include `navigation.jsonl`, `navigation.png` and daemon/app logs.
- The four-panel offline screenshot was built, rendered and visually inspected
  at `target/navigation-ui-review.png`.
- The running user desktop acknowledged the Ctrl+R-equivalent instance command
  and logged **activated UI generation 23** with the navigation fix.

Early native-run attempts exposed runner setup issues, not navigation failures:
a HOME-dependent Cargo shim, build-lock timeouts, and the daemon's ordinary
five-minute idle shutdown while startup was still compiling. The runner now uses
an explicit Cargo proxy, a bounded configurable build timeout, and a temporary
server owned by the runner. The successful run above exercised those fixes.
