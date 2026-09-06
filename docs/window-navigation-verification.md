# Navigation without keyboard focus, 2026-09-06

## Failure and fix

The rendered regression reproduced Super+J leaving `active_row=0` instead of
moving to row 1 after `Window::blur()`. GPUI dispatches keys to the window root
when focus is absent or names an unmounted control. Workspace capture handlers
are not on that path.

A workspace-owned keystroke subscription now handles navigation in that case.
It uses GPUI's actual keymap, preserves overrides and disabled bindings, and
restores the active focus target even for boundary no-ops. Mounted inputs still
use the existing capture handlers. The subscription verifies the event window's
current root identity, so other windows and retained old roots cannot respond.
It is dropped with its workspace rather than accumulating on reload.

## Verification

- Before the fix: `navigation_without_mounted_focus` failed on Super+J with row
  0 rather than 1 (`target/window-navigation-before.log`).
- After the fix: all 3 new regressions passed. They exercise absent and detached
  focus, immediate keys without a settling frame, top/bottom boundaries, empty
  rows, return to composers, H/L, repeated keymap installation, root replacement,
  a disabled binding, and a newly bound alias (`target/window-navigation-fixed.log`).
- All 17 navigation tests passed (`target/window-navigation-suite.log`).
- `python3 scripts/screenshot.py target/ui-review-window-navigation.png` rebuilt
  the desktop and rendered it on a private Xvfb display. The screenshot was
  inspected: transcript, sidebar, and focused composer rendered normally.
- `python3 scripts/accept-navigation.py target/window-nav-native --linked-ui --reloads 0`
  passed 45 native checkpoints with four real SDK sessions. Selected panels,
  keyboard focus, and map state stayed consistent. This run did not hot reload.
- The running user process, PID 727408, accepted the Ctrl+R-equivalent instance
  command. Its activated generation 2, staged at 00:30:58 in
  `/tmp/jcode-desktop-ui-rpUJR2/jcode-desktop-ui-0002.so`, was present in its memory
  maps and contained `Workspace::install_navigation_fallback` symbols. This
  independently confirmed the running app loaded the fix. The redundant log
  watcher was stopped after this confirmation, without stopping the application.

A broader UI run was not fully green: 517 passed, 6 failed, 7 ignored, and 5 Gmail
tests were excluded. Failures concerned Mermaid zoom centering (0.25px), two
remote-sidebar assertions, a sidebar creation request contract, and two account
layout height assertions. They are recorded in `target/window-navigation-broad.log`.
No full-suite success is claimed. Concurrent worktree changes were preserved and
excluded from the shortcut implementation commit `6e4b4e6`.
