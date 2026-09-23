# Single-panel windows

```sh
jcode-desktop --single-panel
```

Each invocation opens a new native window. The visible panel fills the window
without the workspace sidebar, tab strip, overview, or map. Starting another
single-panel window does not replace or change an existing normal,
`--no-sidebar`, or single-panel window.

All single-panel windows share **one host process**. The first launch becomes
that host. Later launches forward their arguments (`--resume`,
`--session=<id>`) and window-scoped environment (`JCODE_DESKTOP_WORKING_DIR`,
`JCODE_DESKTOP_STATE`) over the host's socket and exit immediately. Each window
keeps its own launch settings, chat, and state. Sharing one GPU context, one UI
library, and one runtime saves roughly 60 to 150 MiB for every extra window.

```sh
jcode-desktop --single-panel --new-process
```

`--new-process` opts out and starts an isolated process, as every
single-panel launch did before. Use it to test a risky UI build without
affecting the other windows, or to isolate a window that is doing heavy work.

This is different from `--no-sidebar` (also called `--workspace`), which keeps
the workspace and its navigation while initially hiding the sidebar. Normal
launches still reuse the normal instance, and `--no-sidebar` launches still
reuse their own named instance.

## Standalone resume menu

```sh
jcode-desktop --resume
# Equivalent explicit form:
jcode-desktop --single-panel --resume
```

This opens the Desktop session browser directly in a new standalone window,
not the CLI and not an existing workspace. Search by title, folder, or session
ID. Use Up/Down to select a session and view its conversation preview, then
Enter to resume in the same single-panel window. Ctrl+1/2/3 selects All, Active,
or Saved. Escape returns to the window's fresh chat. Existing windows stay intact.

For Niri, add this inside `binds` (adjust the executable path if necessary):

```kdl
Alt+Shift+B repeat=false hotkey-overlay-title="Jcode Desktop: resume (single panel)" {
    spawn "jcode-desktop" "--single-panel" "--resume";
}
```

The existing Alt+B CLI resume shortcut can remain unchanged.

To verify the startup picker, search, preview navigation, draft preservation,
and resuming without workspace chrome on an isolated display:

```sh
python3 scripts/verify-resume-panel.py target/resume-panel.png \
  --binary target/release/jcode-desktop
```

## Interaction

- One panel is visible at a time. Chat retains its usual input and controls.
- Utility views such as review or login may temporarily fill the window.
  Use **Back to chat** to return to the chat rather than creating another
  workspace pane.
- **Super+Q** closes the chat window. While a utility view is open, it closes
  that view and returns to the conversation. Other windows continue running.
  The shared host exits after its last window closes.
- Each launch starts a fresh chat, without reading or overwriting workspace
  crash-recovery files. Normal session history still belongs to Jcode.
- **Ctrl+R** and `/update` in a source hot-reload build rebuild the shared host
  once and reload **every** single-panel window, preserving each chat. The
  reload is all or nothing: if any window rejects the new UI, every window is
  restored to the previous one. An isolated `--new-process` window reloads
  alone. `--single-panel --reload-ui` and
  `--single-panel --toggle-voice` are rejected because they cannot identify
  which existing standalone window to target.
- Existing workspace shortcuts for sidebar toggling, overview, creating or
  moving panels, changing strips, and panel widths are disabled. This mode
  cannot be converted into a workspace with a keyboard shortcut.

On Unix, the shared host owns `$XDG_RUNTIME_DIR/jcode-desktop-single-panel.sock`
and an isolated `--new-process` window owns
`jcode-desktop-single-panel-<pid>.sock`. Normal and no-sidebar instances retain
`jcode-desktop.sock` and `jcode-desktop-no-sidebar.sock`. Sockets are removed
when their process closes normally. Do not use the normal instance socket to
control an unrelated standalone window.

Trade-offs of sharing: a crash or a hung UI thread affects every single-panel
window at once (chat history is safe in the Jcode server, unsent drafts are
not), and one very busy window can make the others less responsive. Use
`--new-process` when that isolation matters.

## Shared host acceptance

```sh
python3 scripts/accept-shared-single-panel.py target/accept-shared-single-panel \
  --binary target/release/jcode-desktop
```

It opens several single-panel windows on a private Xvfb, checks they share one
PID and socket while each keeps its own state file, that `--new-process` stays
separate, that Super+Q closes one window without stopping the host, that a
later launch joins the same host, and that the host exits and removes its
socket after the last window closes. It records the host's PSS after each
window in `results.json`.

## Isolated acceptance

This covers `--new-process` windows. Use an already-built host containing the
current linked UI:

```sh
python3 scripts/accept-single-panel.py target/accept-single-panel \
  --binary target/debug/jcode-desktop
```

The output directory must not exist. The script **never builds or reloads**.
Build the current host/UI separately before running it. It needs Python 3,
Xvfb, Openbox, xdotool, ImageMagick `import`, and Mesa lavapipe.

The harness imports the environment allowlist from `scripts/screenshot.py`.
It creates a private Xvfb display, private Openbox, offline screenshot fixture,
and temporary home/config/runtime directories under `target/`. It does not
inherit desktop sockets, credentials, app settings, or the live display. All
four processes share the same private `XDG_RUNTIME_DIR`, but receive distinct
`JCODE_DESKTOP_STATE` paths. It uses `--no-hot-reload` to avoid startup builds.

Checks include:

1. Normal, no-sidebar, and two standalone launches remain alive with four
   distinct PIDs, native X11 window IDs, and listening socket inodes.
2. Normal and no-sidebar window IDs, socket identities, and workspace state
   remain unchanged after each standalone launch and interaction sequence.
3. Standalone navigation reports `single_panel: true`, `visible_panels: 1`,
   `sidebar_visible: false`, no overview/map/tab targets, exactly one panel,
   and canvas width equal to the 1440-pixel viewport width.
4. Native workspace shortcut aliases cannot change strips, add panels, reveal
   the sidebar/overview, resize the panel, or mutate either workspace window.
5. Super+Q exits one standalone process, removes its window and socket, and
   leaves the other three windows, PIDs, sockets, and workspace state intact.

`results.json`, separate process logs, and final state dumps are retained even
on failure. `single-a.png` and `single-b.png` capture the actual standalone
windows for visual review. Review these images for full-height panel content
and absence of workspace chrome. The state checks establish width and mode
invariants, not pixel-perfect rendering. Dedicated GPUI layout tests check the
`single-panel-root` / `single-panel-surface` bounds and absence of mounted
workspace content. This script does not inspect the GPUI element tree or test
live provider authentication. Login/review return-to-chat behavior should also
be checked with the utility-view tests.

### Recorded acceptance: 2026-09-19

The release-binary run passed:

```sh
python3 scripts/accept-single-panel.py target/accept-single-panel-release \
  --binary target/release/jcode-desktop
```

`target/accept-single-panel-release/results.json` records success for all four
independent windows, disabled workspace shortcuts, unchanged normal/no-sidebar
instances, and standalone close isolation. Native Super+Q exited the selected
process with status 0, removed its window **and per-PID socket**, and left the
other three instances unchanged. Earlier debug runs exposed a stale socket
file after exit. This release run verifies the host cleanup fix.

Both retained 1440×1000 screenshots, `single-a.png` and `single-b.png`, were
visually reviewed. Each shows full-window chat without workspace tabs, sidebar,
or map. Logs and state dumps are in the same evidence directory. This is a
focused native acceptance result, not a claim that the full UI test suite is
passing.

Additional checks on the implementation:

- Seven focused standalone UI/reload/recovery tests passed, including draft
  retention, login/review return, and disabled detached-focus navigation.
- All 40 host tests, three launch-mode tests and six Linux updater tests passed.
- The paired release host/UI build passed. The normal-workspace screenshot at
  `target/ui-review-single-panel-regression.png` was also rendered and reviewed.
- The full serial UI suite was **not green**: 1,069 passed, eight failed and
  nine were ignored. Failures covered a sidebar selector, an unfinished-work
  entity borrow, and workspace swipe/pull behavior. These broader failures
  are recorded in `target/single-panel-full-tests.log`, not counted as passing
  acceptance evidence for this mode.
