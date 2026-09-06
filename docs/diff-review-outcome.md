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

## Whole-result rerun, including earlier work

The complete workspace was exercised again after the feature was delivered, rather than relying only on the earlier targeted checks. The shared checkout continued changing during this run, so these results are observations of the integrated worktree, not a claim that every concurrent change is green.

- `cargo test --workspace -- --test-threads=1` completed the desktop, API, and motion suites with 19, 3, and 4 passing tests. Its UI run reported 552 passed, 3 failed, and 7 ignored. All 101 diff-named checks in that run passed.
- The three failures were obsolete regression expectations following the new machine switcher and optimistic session creation. The accounts tests now assert that their section starts below the machine switcher and ends at the sidebar body's bottom. The session test now verifies that the create request identifies the immediately opened pending draft, while retaining the one-click/one-command check. All three corrected tests passed in subsequent complete UI execution.
- `cargo build -p jcode-desktop -p jcode-desktop-ui` succeeded, followed by a successful native diff acceptance run (`target/diff-whole-result-*.png`).
- A later unfiltered UI execution encountered a real-thread Gmail callback that violated GPUI's deterministic test scheduler and aborted. Repeating with only the two live-Gmail-opening tests excluded reached the end: **562 passed, 5 failed, 7 ignored, 2 filtered out** in 33.26 seconds. This included **103 passing diff-named checks** and all three corrected assertions. It was not a green full-suite run.
- The five remaining failures were in concurrent image-cache and shortcut work: `render_callbacks_detect_cached_image_eviction`, `super_shift_d_focuses_existing_todoist_without_service_requests`, `super_shift_g_focuses_existing_gmail_without_service_requests`, `super_shift_slash_creates_correlated_help_and_records_its_prompt`, and `super_t_inserts_and_focuses_an_inert_terminal`. They were not hidden by the diff test filter or changed to make this report green.
- Finally, `python3 scripts/accept-diff.py target/diff-whole-final-retry.png` passed against the rebuilt app. It clicked both file metadata targets, opened review, selected the second file through the tree, verified its test function, and returned to chat via both Escape and Back. The selected-file PNG was also read and inspected. An immediately preceding native attempt timed out with a blank frame, so repeatability under concurrent build load is not claimed.

Detailed local logs are `diff-whole-workspace-retry.log`, `diff-whole-build.log`, `diff-whole-direct-tests.log`, `diff-whole-no-live-gmail.log`, and `diff-whole-native-final-retry.log` under `/home/jeremy/.jcode/scratch/`. Several additional Cargo attempts were blocked by shared build locks, timed out while linking, or received SIGTERM. Reusing the newly linked real test executable avoided another competing build. These constraints do not change the observed diff workflow pass, but they prevent describing the entire moving checkout as fully green.
