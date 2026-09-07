# Held panel close focus regression

## Cause and fix

`row_indices` includes dismissed slots until their close animations finish. The
close handler passed those slots to `focus_after_close`, which prefers the right
neighbour. Closing from the right therefore selected the already-dismissed right
neighbour on the second repeat, and focus disappeared when that slot retired.
The previous regression only closed from the left and checked dismissal counts.

The close handler now selects only live successors. `focus_active` also rejects
closing slots, leaving keyboard shortcuts on the workspace when the last panel
is dismissed. Rendering keeps its existing fade-out lifecycle.

## Verification

- Before the fix, the new right-edge regression failed on repeat 1 of a 12-panel
  run: `selected a dismissed panel` (`target/held-close-before.log`).
- All eight close/navigation tests pass. They cover left, middle and right starts,
  key repeats without key-up, actual composer focus, interleaved slot retirement,
  empty-workspace focus, and opening a new panel (`target/held-close-after.log`).
- The serial UI suite passes 597 tests, with seven existing ignored tests and one
  unrelated Gmail sidebar test excluded (`target/held-close-ui-serial.log`). The
  unfiltered parallel run aborted in that Gmail test's background thread due to
  GPUI's deterministic scheduler check. A vertical animation test also failed
  in the parallel run and passed serially. Neither failure required a change to
  the panel-close fix.
- The native desktop build succeeds (`target/held-close-build.log`).
- The running desktop acknowledged the same rebuild-and-reload action as Ctrl+R
  and activated UI generation 3 without replacing the host
  (`target/held-close-live-reload.log`).

Native held-key acceptance passed in both layouts using six offline panels and
real X11 autorepeat (60 ms initial delay, 10 Hz). Both runs observed overlapping
fades, retained selection and composer focus on the rightmost survivor, retired
all closed slots, and opened a new focused panel with Super+N. Screenshots of the
surviving panes were visually inspected in both layouts, as was the reopened
folder-tab composer. Final passing reports are
`target/held-close-{normal,folder-tabs}-final.close-panel.json`.
Earlier probe iterations exposed XTest duplicate-keydown suppression and a
20 Hz hold-release overshoot. The final probe uses real server repeat at 10 Hz,
immediate key release, and waits for presentation before taking screenshots.

Reproduce with:

```sh
python3 scripts/screenshot.py target/held-close-normal.png --no-build --close-interact --panels 6 --layout-mode normal
python3 scripts/screenshot.py target/held-close-folder-tabs.png --no-build --close-interact --panels 6 --layout-mode folder_tabs
```

Each run writes a `.close-panel.json` report and survivor, empty, and reopened
PNG artifacts alongside the initial screenshot. This uses an isolated Xvfb
display and offline fixture panels, never the active compositor. Native Wayland
shortcut forwarding is outside this check. No compositor configuration changed.

## Independent reproduction and picker follow-up

A second private-Xvfb probe reproduced the original bug with six offline panes,
starting in the middle and holding Super+Q at 25 Hz. Three panes closed, then
`keyboard_panel` became null while three live panes remained. The trace and
screenshot are in `target/held-close-baseline-middle-2/`.

After merging the upstream fix, an independent regression exposed the same
successor-selection error in immediate default-directory picker removal. With
`[left, picker, right]`, closing right then picker selected the fading right
slot after indices shifted. Picker removal now filters closing successors too.
The picker-only regression failed against the merged upstream code and passed
after this narrow follow-up. All nine close/navigation tests and all fifteen
`default_directory` tests passed.

The independent native probe then passed all five scenarios at 25 Hz: left,
middle, right, right-to-left through the picker, and Ctrl+Shift+W (the forwarded
close alias). Every scenario emptied the workspace, reopened one focused pane,
and confirmed that release stopped closing. It uses real X11 autorepeat, the
production linked UI, offline data, and private display/config/runtime paths.

```sh
python3 scripts/accept-held-close.py target/close-middle
python3 scripts/accept-held-close.py target/close-right --start last
python3 scripts/accept-held-close.py target/close-left --start first
python3 scripts/accept-held-close.py target/close-picker --picker
python3 scripts/accept-held-close.py target/close-alias --alias
```

Each output directory must be new. It retains `trace.json`, before/after PNGs,
and app logs. The native build and these checks passed locally. The user's
packaged running host had hot reload disabled, so it was not restarted or
replaced during this investigation. These local checks do not claim live
Wayland forwarding or deployment to that already-running process.

## Public session integration acceptance

`accept-shortcuts.py --global-helper <installed-helper>` additionally invokes
that helper's `close` operation repeatedly against real isolated daemon sessions.
Each close must select a surviving pane with matching keyboard focus. All slots
must retire, then the same helper must create one new focused session through the
real session API in the pinned directory. A later observation verifies that the
new session remains open. The harness records each close in `acceptance.json`.
This passed with ten real panes closed to zero and one new session reopened.

The private X11 adapters replace only the compositor identity query and Wayland
key injector. The installed shell helper, production UI, native alias events,
SDK bridge, daemon, session creation and preference persistence are real.
The successful run is `target/held-close-public-close/acceptance.json`.
A separate linked-UI navigation probe was stopped before a visible window or
state frame appeared and its private processes were cleaned up. It supplies no
passing evidence. Native Wayland acceptance could not run because headless Sway
is not installed. Neither probe touched the user's compositor or live sessions.

## Native Wayland integration verified

The missing-tool blocker was subsequently resolved without installing system
packages: Arch's Sway and missing runtime dependencies were extracted into
`target/headless-sway-tools`. The acceptance harness now accepts `--sway-prefix`,
uses `jcode api-bridge` when a standalone bridge binary is absent, and supports
`--linked-ui --held-keys` to avoid an unrelated development hot reload.

The full native Wayland held-key run passed in 32 seconds. It used a private
headless Sway compositor, real Super key-down/up and autorepeat, the installed
shortcut helper, real wtype injection, the production Desktop UI, and real
isolated daemon sessions. Only the helper's compositor-specific focus-query
shape was translated to actual private Sway IPC. No key injection or session API
was mocked. Eight real panes were created. Holding Super+Q dismissed multiple
panes, release stopped dismissal, a subsequent hold emptied the workspace, and
Super+Enter reopened a focused session in the configured directory. Further
checks verified no stale repeats and held navigation/creation behavior.

```sh
python3 scripts/accept-wayland-shortcuts.py target/held-close-wayland \
  --helper ~/.config/niri/firefox-tab-shortcut.sh \
  --directory target/held-close-wayland-directory --held-keys --linked-ui \
  --sway-prefix target/headless-sway-tools
```

`target/held-close-wayland/acceptance.json`, `focus-queries.jsonl`, and
`empty-reopen-creation.json` preserve the observations. This closes the native
Wayland forwarding integration check, but does not claim testing on the user's
live Niri compositor or replacing their running packaged app.
