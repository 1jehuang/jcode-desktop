# Diff review outcome verification

Verified on 2026-09-06 after the delivered desktop build and live UI reload.

## Measured improvement, not just visual inspection

The same two-file patch was executed through the previous production preview parser (from `4ca5e8e^`) and the delivered `diff.rs` parser. The comparison used a standalone Rust harness importing the production code, not a reimplementation of either parser.

| Observation | Previous parser | Delivered parser |
| --- | --- | --- |
| Files exposed from the two-file patch | 1 | 2 |
| `src/navigation.rs` includes lines belonging to `tests/navigation.rs` | Yes | No |
| Source file metadata | Only a path with combined patch lines | Modified, +1 / −1 |
| Test file metadata | Not exposed separately | Added, +4 / −0 |

The executed assertions confirmed that the added test function is absent from the source file's change lines and present in the test file's change lines. Output is recorded in `target/diff-outcome-comparison.json`. This is a parser-level comparison, not a claim that the old UI was replayed.

## Requirement-to-observation evidence

| Requirement | Executed check | Observed result |
| --- | --- | --- |
| More clickable per-file metadata | `python3 scripts/accept-diff.py target/diff-outcome-recheck.png` | Two distinct native Review targets appeared. Both were clicked and opened review. |
| Open the selected file's diff | First native file click, followed by tree selection | `src/navigation.rs` appeared first. Selecting the second tree leaf displayed `tests/navigation.rs` and `navigation_label_is_clear`. |
| A navigable file tree in that panel | Native selection plus `diff_click_tree_copy_and_escape_preserve_chat` | Both file leaves were present. Selecting index 1 changed the selected file to index 1. Collapsing and expanding `tests` removed and restored its leaf. |
| Preserve the surrounding chat workflow | Native Escape and Back clicks plus GPUI interaction assertions | Both dismissal methods returned the two inline Review targets. Draft remained exactly `keep my draft`; transcript offset was unchanged. File metadata clicks did not expand raw tool JSON. |
| Full changes, not only the compact snippet | `diff_review_scrolls_full_changes_without_scrolling_chat` | A 500-line write retained all 500 rows. A real wheel event moved the review scroller while leaving the transcript offset unchanged. |
| Useful detail and copy behavior | `diff_click_tree_copy_and_escape_preserve_chat` | Copy output contained the selected test path and its added test function. |
| Updated running application | Host Ctrl+R-equivalent command and appended application log | Host replied `ok`; a fresh `activated UI generation 1` was observed for `target/debug/libjcode_desktop_ui.so`. |

The final native recheck used the desktop executable built at 00:38:31. The five exact `panel::diff_review::tests` were rerun from the current test binary and all passed in 0.36 seconds. The earlier integrated diff suite passed 94 tests.

Screenshots from the successful final native sequence are `target/diff-outcome-recheck-{ready,review,selected,closed,reopened,back}.png`. The tests drive the actual GPUI desktop on a private Xvfb display, without touching the user's active window. They use offline transcript data, so they do not claim to validate a live provider or remote filesystem. No filesystem state is needed to review the supplied patch.

These checks establish a concrete improvement in file attribution and navigation. They do not establish a subjective preference for the new visual styling.
