# Focus action verification, 2026-09-06

## Reproduced defects and changes

- Horizontal navigation included panels already dismissed but still fading out.
  `FocusLeft` and `FocusRight` now choose the nearest non-closing panel. Closing
  surfaces remain mounted for their animation, and boundary motions remain no-ops.
- Submitting the Ctrl+O folder picker with Enter removed its focused search box
  before session creation replied. A rendered regression observed `keyboard_panel`
  becoming null instead of remaining on panel 1. Submission and cancellation now
  request focus restoration on the next render, independent of the runtime reply.

## Checks

```sh
cargo test -p jcode-desktop-ui navigation -- --test-threads=1
cargo test -p jcode-desktop-ui folder_picker_dismissal -- --test-threads=1
cargo test -p jcode-desktop-ui -- --test-threads=1 --skip gmail
python3 -m unittest discover -s scripts -p 'test_accept_navigation.py'
cargo build --workspace
python3 scripts/accept-navigation.py target/focus-final-0635 --reloads 1
```

The targeted navigation group passed 13 tests. The picker regression passed all
three dismissal paths (Cancel, Open, Enter) and subsequent left/right movements.
The broad run passed 382 tests with 7 ignored and 5 Gmail tests filtered out.
The unfiltered run aborted in the `jcode-gmail-inbox` thread inside GPUI's test
scheduler, so it is not being represented as a fully passing suite. The Python
checker suite passed 7 tests.

Native acceptance passed 76 checkpoints with four real SDK sessions and one
Ctrl+R reload on a private Xvfb display. It covered ordinary and held-key
navigation, boundaries, overview and empty strips, and rapid close+left and
close+first+right sequences without waiting for dismissal animations. Evidence
is in `target/focus-final-0635/navigation.jsonl` and `navigation.png`.

Counts above describe the tested working tree, which also contained pre-existing
uncommitted overview-focus and native-runner improvements. Those changes were
preserved and are not included in this focus-action commit.

The real application was rendered and inspected using `scripts/screenshot.py`
(`target/ui-review-focus-0633.png`). The running user desktop acknowledged its
Ctrl+R-equivalent instance command and activated UI generation 2 with the fixes.
No input was sent to the user's window during testing.

## Independent outcome comparison

The final committed regressions were rerun after delivery. All three passed.
Their destination assertions compare directly with the saved pre-fix failures:

| Public action and state | Before fix | After fix | Required result |
| --- | --- | --- | --- |
| Left from slot 3 past closing slots 1 and 2 | Focused closing slot 2 | Focused slot 0 | Slot 0 |
| Right from slot 0 past closing slots 1 and 2 | Focused closing slot 1 | Focused slot 3 | Slot 3 |
| Enter submits folder picker from panel 1 | No panel owned keyboard focus | Panel 1 owned keyboard focus | Panel 1 until the new session arrives |

The picker regression also verified four subsequent left/right steps after each
of Cancel, Open, and Enter, with selected panel and keyboard target equal at every
step. It did not deliver a session-created response, proving that focus recovery
does not depend on a successful or fast backend reply.

A separate comparison re-read all 76 native snapshots, without trusting the
runner's pass message. It found **zero keyboard/selection mismatches** and
**zero closing panels selected**. Session-identity comparisons confirmed that
close+left selected the original left neighbor and close+first+right selected
the surviving right neighbor. Each dismissed session was absent, and all
remaining sessions kept their order. Machine-readable evidence is saved at
`target/focus-outcome-comparison.json`.

These observations establish improvement for the reproduced failures. They do
not establish that either failure was the user's exact original symptom, which
was not described further.

### Native before/after executable comparison

The exact same acceptance script was also run against the retained original
executable (`/proc/69554/exe`, launched before these fixes) and the rebuilt
`target/debug/jcode-desktop`. Both used `--linked-ui --reloads 0`, separate
private displays, and newly created real sessions, so neither could silently
load a newer plugin.

The original executable failed `after-close-left`: after closing panel 1 and
immediately pressing left, it selected **slot 2 with no keyboard focus**, rather
than slot 0. Its final snapshot had `focused_slot=2, keyboard_panel=null` after
the five-second observation timeout. This was a real native input failure, not
an inferred problem or a test-only animation fixture.

The rebuilt executable passed the identical sequence with
**`focused_slot=0, keyboard_panel=0`**, then passed the rightward sequence with
**`focused_slot=1, keyboard_panel=1`**. The original run exited 1 and the updated
run exited 0. Logs and states are in `target/focus-native-before-0639` and
`target/focus-native-after-0639`, with their sibling `.log` files and summary
`target/focus-native-before-after.json`.
