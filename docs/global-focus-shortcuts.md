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
For `jcode-desktop` only, `previous` and `next` send Ctrl+PageUp and Ctrl+PageDown,
which are existing Desktop focus bindings. It never re-emits the globally
intercepted Super chord. New/close behavior and unrelated apps remain unchanged.

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
- All five helper tests passed against both the repository script and the
  installed replacement. They cover both Desktop directions, all eight Firefox
  app-ID/action combinations, missing focus, unrelated apps, invalid arguments,
  and unchanged Desktop new/close behavior. The compositor and virtual keyboard
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
