# Additional theme acceptance, 2026-09-05

Implementation: `65fd76b` adds six palettes, increasing the picker from four to ten
choices. The existing four remain selectable and keep their IDs and order.

## Observed end-user behavior

The real desktop executable was driven with native X11 mouse and keyboard input
on `scripts/screenshot.py`'s private Xvfb/Openbox display. Configuration, home,
state, and display were isolated from the user's desktop. No inference was used.

All ten picker rows were clicked. Each click wrote its correct theme ID and
rendered the expected panel color. Each of the six additions was then verified
in a fresh process using the config written by that click, without selecting a
palette after restart:

| New palette | Measured panel RGB after click | Measured RGB after restart |
| --- | --- | --- |
| Midnight | `#1a2032` | `#1a2032` |
| Ocean | `#172d34` | `#172d34` |
| Forest | `#202e25` | `#202e25` |
| Plum | `#2d2232` | `#2d2232` |
| Rose dawn | `#fff7f5` | `#fff7f5` |
| Parchment | `#faf3e3` | `#faf3e3` |

After clicking the composer, ten native Super+Shift+T shortcuts cycled through
all ten saved IDs and rendered colors. The composer text region was unchanged
byte-for-byte after returning to the initial palette. Public fixture state
confirmed keyboard focus remained on panel 0.

This is a concrete improvement over the original four choices: six additional
visually distinct palettes are reachable through actual picker clicks, affect
the rendered conversation, and retain the user's selection across restart.

## Supporting checks and artifacts

- `cargo test -p jcode-desktop-ui --lib theme -- --test-threads=1`: 8 passed,
  including picker clicks, focused-composer shortcut, ID/cycle coverage,
  transition endpoints, config preservation, and contrast checks.
- All six new palettes meet the tested 4.5:1 text, syntax, and status contrast
  thresholds. Two light-theme code-type colors were darkened after the initial
  checks measured ratios below that threshold.
- `python3 -m unittest discover -s scripts -p test_screenshot.py`: 6 passed.
- `python3 scripts/screenshot.py target/ui-review.png --theme midnight`:
  production build and real-app capture passed. All six captures were reviewed.
- Native picker/restart driver: local `target/verify-theme-picker.py`, exit 0.
  Evidence: `target/more-themes-picker-acceptance/observations.json` and captures.
  The initial driver assumed a fixed three-second startup and sampled a blank
  frame. It was corrected to wait for the restarted app to render before checking.
- This is theme-feature acceptance, not a claim that the entire concurrently
  modified repository or all platform packages passed verification. Unrelated
  working-tree changes were not included in the theme commit.

## Live activation limitation

The running app acknowledged its Ctrl+R-equivalent rebuild request, and the
plugin build succeeded. Activation explicitly failed with `hot reload is
disabled; launch with --hot-reload`. The original process was not terminated,
since doing so could discard unsent drafts. Therefore the new build is verified
but activation in that existing window remains deferred pending restart approval.
