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
