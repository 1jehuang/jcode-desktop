# Folder-tab requirement-to-evidence map

This supplements `overlapping-tabs-verification.md`. The native acceptance is the
real daemon → SDK bridge → GPUI desktop workflow on a private Xvfb display, with
native pointer/keyboard events and actual hot reload. It is not a fixture or a
copied-source substitute. Fixture screenshots and deterministic tests supplement
that workflow.

| Explicit request / changed output | Concrete check | Observed result |
| --- | --- | --- |
| Compact top strip, all tabs accessible | `attached-tabs-accepted/navigation.jsonl`, twelve real sessions at 800×700, forward/reverse/jump clicks before and after reload | 52 native clicks selected exactly the expected session and composer. The same twelve identities remained present. No strip scrolling or accidental new sessions. |
| Recognizable overlapping folder shapes | `live_tabs_overlap_preserves_folder_width_and_centers_the_stack` and `live_tabs_anchor_inside_the_visible_panel_and_keep_every_edge`, plus crowded real-app screenshots | At a 512px strip, inactive folders retain up to 112px silhouettes with positive exposed edges. Tests cover up to 200 logical tabs, including partially clipped active panels. Selected folder paints last. |
| Centering, superseded by attachment to the actual panel | Native trace comparison of selected tab target with visible panel center, independently derived from session widths and camera position | All 151 applicable native snapshots had exactly 0.0px center error. The 800px screenshot places the active tab over the right-side panel rather than over the preceding panel. |
| More title for the current tab | `live_tabs_all_remain_visible_during_resize_and_keyboard_navigation`, geometry test and crowded screenshot | Selected folder reaches 208px versus 112px underlying inactive folders, with crowded sibling titles omitted. The selected fixture title `Review folder 4` is fully visible. |
| Movement/dynamics | `live_tabs_motion_expands_lifts_and_retargets_without_jumping`, rendered camera test, native focus changes | Intermediate width and height values occur during the 150ms transition. Reversal is continuous for the interpolated values. Active folder lifts 4px. Reduced motion and resizing snap. Native navigation reaches the correct final selection. This does not claim frame-by-frame vertical-row/reorder coverage. |
| Emoji identifiers on tabs | GPUI debug-bound checks for every tab during resize, working-emoji test, real native screenshot | Each tab has an emoji element. Native sessions show their distinct session icons, and the working-emoji state does not shift the title. |
| Tab connected to its panel | `live_tabs_remain_attached_when_panel_camera_moves` and native center comparison | At three selected panels × three camera positions, tab bounds remain inside the visible panel and the bottom meets its top. Native center error is 0.0px across 151 applicable snapshots. Completely offscreen panels use an in-bounds fallback, not a claimed impossible connection. |
| Softer, lower-contrast outline | Pixel samples from real-app `ui-review-overlap-crowded.png` at (550,16)/(550,22), versus `ui-review-attached-tabs.png` at (625,16)/(625,22) | Border/inside RGB changed from (135,121,107)/(37,34,31) to (86,78,69)/(37,34,31). Euclidean RGB contrast fell from 151.489 to 76.033, about 49.8%, while the interior stayed identical. This is a concrete rendered contrast measurement, not an accessibility contrast-ratio claim. |
| Full title discoverability retained | Tooltip regression at the actual rendered tab | Full-title tooltip appears after hover. |
| Correct native focus and reload integration | Entire `attached-tabs-accepted` run | 157 state checkpoints passed across UI generations 1 and 2, preserving sessions and composer focus through mouse, keyboard, row changes and reload. |
| New diagnostic outputs used by acceptance | Native trace `tab_targets`, `tab_motion`, `camera_motion`; readiness/read-race fix | Exposed-edge click positions come from the actual renderer after motion settles. Validation retains the exact ready snapshot, avoiding a second read during a state-file rewrite. |
| Keep test artifacts from exhausting storage | Final native cleanup inspection | No `jcode-desktop-ui-*` temporary plugin directories remained in the completed run's private `tmp` directory. |

## Explicit limits, not passing guarantees

- “No matter how many” is not physically achievable as an unlimited number of
  individually distinguishable or clickable edges on a finite-pixel display.
  Twelve real tabs were exercised end to end, and up to 200 logical tabs were
  checked geometrically. No infinite-count usability guarantee is made.
- “Always connected” is verified for visible panels and the sampled horizontal
  camera states. Completely offscreen panels cannot have a visible attachment.
  Vertical row/reorder transitions were not checked frame by frame.
- The visible design was successfully loaded into the user's desktop as live
  generation 6. That process later exited. A later diagnostic-only refresh was
  refused by the stale socket, so no claim is made that an app is running now.

## Whole-result rerun

The post-audit rerun completed successfully at
`target/attached-tabs-whole-result-audit`:

- All nine tab tests passed on the real project library.
- The complete native sequence passed again: 157 checkpoints, 52 exposed-edge
  clicks, twelve real sessions, and one actual hot reload.
- Re-analyzing this new native trace independently confirmed 153 applicable
  active-panel attachment samples with a maximum center error of 0.0px.
- Private temporary plugin directories were again absent after cleanup.
- No UI implementation changes followed these checks. The only follow-up was
  documenting this requirement map and its explicitly bounded observations.
