# Folder roller: requirement-to-observation map

Verified 2026-09-05 (local time), with the follow-up run ending 2026-09-06
00:07 UTC. This map distinguishes the user's explicit request from the public
behavior added to implement it. A passing implementation check is not evidence
that the user prefers horizontal orientation over a vertical picker.

## Evidence and reproduction

- **V**: real app screenshots `target/startup-ui-review.png` (before),
  `target/ui-review-roller-final.png`, `target/ui-review-roller-light.png`, and
  the final `target/roller-traceability.png`. Pixel measurements and sampling
  coordinates are recorded in [the outcome audit](sidebar-roller.md#interpretation-and-outcome-audit).
- **N**: `python3 scripts/accept-roller.py target/roller-traceability-native --actions`
  passed. Its `roller.jsonl` contains 17 screenshot/state checkpoints from native
  X11 input on a private Xvfb display, including three action launches. Assertions
  use actual page and panel state, not the diagnostic OCR text.
- **G**: `cargo test -p jcode-desktop-ui --lib sidebar_ -- --include-ignored --test-threads=1`
  passed all 46 tests, including the seven `workspace::sidebar_roller::tests` below.
- **E**: five additional tests passed: settings toggles, theme shortcut with the
  composer focused, new-session coaching attribution, opening a todo's chat,
  and Normal panel geometry. The exact test names are listed in the relevant rows.
- **L**: the running desktop acknowledged its Ctrl+R-equivalent instance command
  with `ok`, rebuilt successfully, and logged `activated UI generation 2` in the
  currently running host. Earlier generation-8 evidence belongs to the previous
  host lifetime. The final screenshot and native run used this rebuilt binary.

## Explicit user requirements

| ID | Requirement | Concrete check | Observed result |
| --- | --- | --- | --- |
| R1 | Change the folder navigation at the top left | V plus N clicks at the sidebar center (132,35), followed by actual `sidebar` state checks | The changed control is the 52px-high sidebar header. Its clicks select Chat/Learn/Files/Accounts/Theme, not the separate live-session tab strip. Pass. |
| R2 | Curve it instead of the flat horizontal tab list | V pixel sampling, using the documented five-channel-level threshold | Old inactive tops were y=22,22,22. New left-to-right contour is y=32,24,19,18,19,24,32, with four symmetric levels and 14px center-to-edge depth. The same contour reproduced in a fresh native capture. Pass. |
| R3 | Make it act like a roller | N `02-wheel-blank-browsed-learn`, `03-wheel-tab-browsed-learn`, previous/next and wrap checkpoints | One native wheel step advances the centered folder over both the backdrop and a tab face. Center-click then selects Learn. Arrows rotate through the collection and wrap back to Chat. Pass for the documented horizontal-roller interpretation. |

## Changed public outputs and preserved interaction contracts

Test names below are in `workspace::sidebar_roller::tests` unless a different
module is specified. All results are observed assertions, not merely proposed checks.

| ID | Public output or contract | Concrete check | Observed result |
| --- | --- | --- | --- |
| P1 | Full-size centered folder, smaller and lower neighbors | `roller_projection_curves_and_compresses_symmetrically`, V | Center height is 34px and bottom offset is zero. Neighbors shrink and recede symmetrically at available widths 156 and 240px, remaining in bounds. Actual image contour matches R2. Pass. |
| P2 | Labels stay within the exposed face instead of disappearing behind the next folder | `roller_exposed_labels_do_not_overlap_and_thin_tabs_keep_full_tooltips` | Learn's rendered label starts at or beyond Chat's right edge and stays within Learn's bounds vertically and horizontally. Pass. |
| P3 | Very thin tabs omit unusable partial glyphs but retain full names on hover | Same G test, using actual pointer movement and advancing the tooltip timer | Accounts is narrower than 16px, has no clipped label element, and hovering it paints the full `accounts` tooltip wider than the thin tab. Pass. |
| P4 | Selected folder touches the sidebar page | `workspace::tests::sidebar_top_folder_tabs_join_their_content_when_clicked` | After real tab clicks and motion settling, selected bottom equals body top and selected height is 34px. Pass. |
| P5 | Horizontal, vertical, and diagonal trackpad input use the dominant axis | `roller_browses_without_launching_and_clicks_each_sidebar_page` | Injected horizontal, vertical, and diagonal GPUI wheel events traverse and return to the expected centered folder without changing the page or opening panels. Pass. |
| P6 | Small deltas accumulate, wheel direction reverses, and endpoints wrap | `roller_wraps_and_accumulates_small_scrolls_without_activating`, N | 20+20 pixels do not move, the next 8 move one step. Reversing 96 moves to index -1, which maps to the last folder. Native previous, next, and seven forward steps from Theme wrap to Chat. Pass. |
| P7 | Both chevrons browse without acting, including across action folders | N before/after `panel_signature` and window assertions | Panel IDs/count, map focus, keyboard focus, active sidebar page, and native-window count remain unchanged during browsing. Pass. |
| P8 | All six page tabs still select their intended page | `roller_browses_without_launching_and_clicks_each_sidebar_page`, N, E | Actual center clicks select Sessions, Learn, Files, Accounts, Theme, and Settings in G. Each tab center is within 1px of x=132. N independently selects the first five. Pass. |
| P9 | Todos action opens the unfinished-work panel | N `09-action-todos`; `workspace::tests::sidebar_todos_button_spawns_a_persistent_movable_panel` | One panel is added with session `unfinished-work` and receives map/keyboard focus. Existing movable/persistent-panel assertions pass. |
| P10 | Todoist action opens the Todoist panel | N `10-action-todoist` | Exactly one panel is added with session `todoist://tasks`, focused in the existing native window. Pass. This does not claim external account connectivity. |
| P11 | Email action opens the inbox panel | N `11-action-email` | Exactly one panel is added with session `gmail://inbox`, focused in the existing native window. Pass. No credentials were present in the isolated fixture. |
| P12 | Folder action opens the in-app picker and cancellation is harmless | `roller_folder_and_new_session_clicks_preserve_their_action_contracts` | Two previous-arrow clicks reveal Folder. Clicking it paints `folder-picker-overlay` without an OS path prompt. Cancel removes the overlay and emits no runtime command. Pass. |
| P13 | New-session action emits one creation request without earning keyboard credit | Same G test; `workspace::tests::clicking_the_new_session_button_is_not_keyboard_credit` | Clicking the centered + sends exactly one recorded `CreateSession` with no startup correlation ID. The coaching test records a slow path and zero recalled shortcuts. Pass. |
| P14 | Roller animates between positions and reverses continuously | `roller_motion_reverses_without_jumps_and_resumes_after_reduced_motion` | The 50ms sample lies strictly between endpoints. Retargeting in the opposite direction preserves that exact sampled value and settles at the new target. Pass. |
| P15 | Reduced motion snaps immediately, and turning it off restores animation | Same G test through the production `move_to_at` path | Zero duration snaps to position 3 with no animation. Reapplying 150ms duration produces a sample strictly between 3 and 4 and reports active motion. Pass after fixing stale zero-duration reuse. |
| P16 | Hiding the sidebar or using Normal mode does not keep scheduling roller frames | `roller_restores_selected_page_and_hidden_motion_does_not_schedule_frames` | A moving visible folder roller makes `animation_active` true. Hiding the sidebar or changing to Normal makes it false. Pass. |
| P17 | Reload restores the selected sidebar page rather than a merely browsed action | Same G test using the real Workspace snapshot/apply methods; L | Snapshot with Settings selected and another folder being browsed restores Settings centered within 1px of x=132, without animation. The real host also successfully activated the rebuilt UI. Pass. |
| P18 | Normal layout retains the old horizontal navigation and separate panels | `workspace::tests::sidebar_navigation_scrolls_with_wheel_and_track`; `workspace::panel_surface_tests::normal_mode_uses_separate_equal_height_panels_without_the_folder_surface` | Wheel/track controls still reach the last horizontal tab in Normal. Normal panels retain equal top/bottom and 12px gaps. Folder mode exposes roller controls instead. Pass. |
| P19 | Settings and theme navigation remain usable after the layout change | `workspace::tests::settings_tab_toggles_window_preferences_and_preserves_theme_tab`; `workspace::tests::theme_tab_and_shortcut_work_with_the_composer_focused` | Layout/settings toggles preserve the correct view, all theme rows remain selectable, and the theme shortcut works from the composer. Pass. |
| P20 | Rendering remains usable in a narrow window and a light palette | V at 1440x1000 warm-neutral and 800x600 neutral-light, plus P1 geometry checks | The roller stays within the sidebar, the centered label remains visible, and light/dark borders and labels render. Pass for these two representative palettes and sizes, not a claim of exhaustive platform coverage. |
| P21 | Acceptance runner checks real interaction without touching live accounts/windows | N process isolation and per-checkpoint window/panel assertions | The complete 17-checkpoint run passed under isolated HOME/XDG/Jcode directories, private Xvfb/Openbox, and software Vulkan. Actions stayed in that one fixture window. Pass. |
| P22 | Deliver the latest runtime change, not only a source edit | L and final V/N run | Ctrl+R rebuild/reload succeeded before final screenshot/action verification. Pass. Commit and push evidence is in Git history. |

## Follow-up fix and limits

Mapping P15 exposed a policy-update gap: after a zero-duration move, the old
AnimatedValue retained zero duration when motion was re-enabled. The production
retarget path now reconstructs the value with the current duration while preserving
the sampled position. P14 and P15 verify both continuity and restored animation.

The complete map covers the three explicit requirements and the changed public
outputs. Horizontal rather than vertical orientation, and browse-on-scroll rather
than select-on-scroll, remain **implementation assumptions**, not missing test
results or user-confirmed preferences. External Todoist/Gmail availability and
all-platform compositor performance are unchanged dependencies outside this task.
The broader library suite's earlier failures/abort are documented in the main
roller audit and are not represented as passing by these targeted results.
