# Global focus shortcut conflict

## Cause

The installed global Super+H/L bindings invoked a Firefox tab helper. That
helper returned without sending input for every non-Firefox window. The global
binding still consumed the original chord, so Desktop's FocusLeft/FocusRight
handlers never received it. Earlier Xvfb tests exercised Desktop directly and
could not detect this host configuration conflict.

Desktop also lacked a native application ID. New and reopened windows now set
`jcode-desktop` before mapping. UI activation also updates it on the next frame
so existing hosts gain it on hot reload. Helpers can identify it without
matching titles or every GPUI application. Setting WM_CLASS synchronously during
initial activation caused an X11 first-paint regression. The screenshot check
caught it, and setting it before mapping plus deferring the compatibility update
fixed it. The final three-panel offline render passed and was inspected at
`target/ui-review-global-focus-final.png`.

## Compatibility helper

`scripts/firefox-tab-shortcut.sh` preserves all four existing Firefox actions.
For `jcode-desktop`, `previous` and `next` send Ctrl+PageUp and Ctrl+PageDown,
which are existing Desktop focus bindings. `new` forwards Super+Enter as
Ctrl+Alt+Enter. Both Enter shortcuts share Super+;'s `pinned_working_dir`, falling
back to home only if it is unset. `close` forwards Super+Q as Ctrl+Shift+W,
which dismisses the focused panel without using Ctrl+W (the composer's
DeleteWordBack key). It never re-emits a globally intercepted Super chord.
Firefox's four actions and unrelated apps remain unchanged.

On hosts with this Firefox helper configuration, back up the existing helper
and install this script at the path already referenced by the global bindings.
Do not install additional global bindings or replace unrelated custom helpers
without reviewing them. The script needs bash, jq, the compositor CLI and wtype.
The local installation was backed up as
`~/.config/niri/firefox-tab-shortcut.sh.bak-desktop-focus-20260906` and replaced
at its existing path. No compositor configuration was changed or reloaded.
Other installations can use Ctrl+Tab / Ctrl+Shift+Tab or Ctrl+PageDown / Ctrl+PageUp
without this helper.

## Verification, 2026-09-06

- Running the new navigation-routing regression against the original installed
  helper failed both directions: no keyboard event was emitted.
- All six helper tests passed against the repository script. They cover both
  Desktop directions and pinned Enter, all eight Firefox app-ID/action
  combinations, missing focus, unrelated apps, invalid arguments, and the safe
  Desktop close alias. The compositor and virtual keyboard
  were PATH stubs. No real compositor queries or user-window input were used.
- `cargo build -p jcode-desktop -p jcode-desktop-ui` passed.
- `python3 -m unittest discover -s scripts -p 'test_accept_navigation.py'`
  passed all seven tests.
- `python3 scripts/accept-navigation.py target/global-focus-final --reloads 1`
  passed 88 checkpoints on private Xvfb with four real SDK sessions. The added
  Ctrl+PageUp/Down checks moved through every panel in both directions before
  and after reload. The new native WM_CLASS check found exactly one visible
  `jcode-desktop` window in both generations. Evidence is in
  `target/global-focus-final/navigation.jsonl` and its sibling `.log`.
- The running user instance acknowledged the same rebuild/reload action as
  Ctrl+R and logged successful UI generation 3 activation. Its existing four
  sessions reconnected. The helper update takes effect on its next invocation.

This verifies routing with synthetic compositor replies and real native app
navigation separately. It does not claim a physical Super+H/L test against the
user's active compositor, which project instructions prohibit.

## Super+Enter follow-up

Super+Enter (Cmd+Enter on macOS) now uses the same persisted pinned directory
as Super+;. Ctrl+Alt+Enter invokes that same action for the global helper.
Super+N and Super+' retain home-directory behavior. The existing local setting
was verified as `/home/jeremy/jcode-desktop`, so no additional personal config
field or hardcoded repository path was introduced.

`cargo test -p jcode-desktop-ui shortcut -- --test-threads=1` passed all nine
matching tests. Native `scripts/accept-shortcuts.py target/pinned-enter-native
--bridge target/debug/jcode-harness-api-bridge` passed seven real-session
creation checks. Both Enter chords created exactly one focused panel in the
pinned directory before and after restart, even after different history became
more common. Super+; remained pinned and Super+' still used home. Daemon creation
records, not inferred labels, verified each working directory. Evidence is in
`target/pinned-enter-native/acceptance.json`.

All six routing tests passed against the installed helper. Its prior version
was backed up with suffix `.bak-desktop-enter-20260906`. The current live host
(PID 486587) acknowledged the Ctrl+R-equivalent action and activated UI
generation 2. The final offline screenshot was inspected at
`target/ui-review-pinned-enter.png`.

### Measured original-versus-installed helper outcome

The combined path was subsequently tested rather than relying on separate
helper mocks and direct-key tests. `accept-shortcuts.py` now accepts
`--global-helper`, `--baseline-helper`, and `--pinned-directory`. In a single
private native app, it ran the backed-up original helper and the installed
replacement with an explicit `/home/jeremy/jcode-desktop` pin.

| Invocation | Before → after panels | Keyboard target | Daemon-reported cwd |
| --- | --- | --- | --- |
| Original helper, `new` | 1 → 1 (zero created) | Unchanged | No session created |
| Installed helper, `new` | 1 → 2 (one created) | New panel, slot 1 | `/home/jeremy/jcode-desktop` |
| Installed helper after restart, `new` | 4 → 5 (one created) | New panel, slot 4 | `/home/jeremy/jcode-desktop` |

The same run verified direct Super+Enter, Ctrl+Alt+Enter, and Super+; before
and after restart, plus the unchanged home shortcut. All nine creations passed,
eight in the exact requested repository. The baseline no-op is a separate tenth
observation. `target/enter-outcome-comparison.json` independently compares panel
counts, keyboard focus, and actual daemon creation records from
`target/enter-outcome/acceptance.json`.

The helper's compositor-query executable is replaced by an adapter that checks
the real private X11 active window and its `jcode-desktop` class. Its virtual
keyboard executable translates the helper's modifier/key arguments into native
X11 input. The actual installed shell helper, actual Desktop, and actual SDK
daemon all execute. No real compositor command, Wayland input, or user-window
interaction is used. This is concrete application-outcome evidence with an
adapted transport, not a claim about a physical keypress on the active desktop.
