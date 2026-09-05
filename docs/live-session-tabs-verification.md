# Live-session folder tabs: verification

Verified on 2026-09-05. Implementation: `b378196`.

## Result against the request

The redesign satisfies the observable parts of the request: the tiny vertical
markers are replaced by readable, clickable folder tabs, the wide top shoulder
is gone, and sidebar selection no longer has to connect to the session sheet.
This is supported by raster measurements and native input, not just source
inspection. It does not establish the user's subjective aesthetic preference.

### Controlled before/after measurements

Both captures use the real app, the same one-session offline fixture, the warm
dark theme, and a 1440×1000 private Xvfb display:

- Before: `target/ui-before-live-tabs.png`, captured before the implementation.
- After: `target/ui-live-tabs-comparison.png`, rebuilt and captured afterward.
- Measurements: `target/live-tabs-measured-evidence.json`.

| Requested change | Concrete check | Observed before → after |
| --- | --- | --- |
| Replace tiny markers with identifiable live-session tabs | Tesseract OCR of the session header, rectangle `(276, 8, 1428, 48)`, enlarged 3× | No readable session title → `Review markdown rendering` |
| Remove the old floating rounded marker | Count non-backing pixels in its former bounds, x=838..865, y=8..34 | 326 → 0 pixels |
| Remove the wide top bar/shoulder | Count session-sheet pixels in x=900..1399, y=18..47, away from the tab | 15,000 → 0 pixels |
| Stop forcing sidebar and session surfaces to connect | Flood-fill the actual session-sheet raster from `(280,300)` and inspect sidebar selection at `(20,120)` | Connected → disconnected |
| Keep a real separation between the two surfaces | Inspect the gutter at x=270, y=16..983 | 850 → all 968 pixels use the canvas backing color |
| Keep live sessions directly selectable | Native X11 click on the first rendered tab after focusing slot 2 | Focus 2 → 0, keyboard focus 0, same real session IDs, overview false |

The tab click uses real isolated SDK sessions, not the screenshot fixture or a
call that directly changes application focus state. Evidence is in
`target/live-tabs-acceptance-v3/folder-2-focused.state` and
`target/live-tabs-acceptance-v3/live-tab-selected.state`.

## Changed public outputs and their checks

| Output/behavior | Check | Observed result |
| --- | --- | --- |
| Active tab, folder baseline and cross-row selection | `live_tabs_switch_rows_and_keep_the_folder_baseline` | Active tab is 4px taller, tab bottoms align with the body, clicking another row selects that row and removes the stale focus marker |
| Long labels and keyboard overflow | `live_tabs_reveal_keyboard_selection_in_a_narrow_window` | In an 800px window with 12 long session titles, keyboard selection reveals the last tab fully, then returns the first tab to the left edge; target width stays at least 100px |
| Empty workspaces | `empty_workspace_does_not_mark_another_rows_session_as_focused` | Empty workspace label appears with no stale focused session; clicking a live tab returns to its row and removes the empty label |
| Small live-state dots | `minimap_paints_live_state_and_todo_progress_through_the_workspace_surface` | Idle, working, error, streaming, idle-with-partial-todos and complete each render exactly their current 5×5px dot inside the live tab; other status dots disappear |
| Unnamed/reconnecting labels | `folder_header_keeps_an_identity_before_a_session_is_named`, plus native `unnamed-folders` capture | Shared label helper keeps a nonempty identity; real unnamed session headers contain visible label ink before rename |
| Normal layout and minimap coexistence | `live_tabs_reserve_space_in_normal_mode_and_beside_minimap` | Tabs remain above the panel and left of the minimap |
| Equal panel baselines, sidebar visibility and resizing | `folder_panels_share_a_level_body_without_individual_tabs` | Geometry remains aligned at 800×600 and 1440×1000, sidebar shown/hidden, and focus on each panel |
| Independent sidebar selection, sheet extent and margins | Native `check_strip` at focus 0, 1, 2, after tab click, after overview and after layout restoration | Exactly one sidebar selection, no connecting gutter, no wide shoulder, level body and visible outer margin |
| Existing panel/overview/settings interactions | Full native acceptance script | Real panel clicks, overview selection and hover, sidebar scrolling, Normal/Folder switching, and persisted layout settings pass |

The eight mapped tests passed in `target/live-tabs-requirement-tests.log`.
The same built test executable was replayed successfully, recorded in
`target/live-tabs-requirement-tests-replay.log`. A later redundant Cargo rerun
waited behind another build and its GPUI compilation was terminated by SIGTERM.
That interrupted rebuild is not reported as a passing test run.

Run the mapped regression tests:

```sh
cargo test -p jcode-desktop-ui --lib -- \
  live_tabs::tests \
  minimap_paints_live_state_and_todo_progress_through_the_workspace_surface \
  folder_header_keeps_an_identity_before_a_session_is_named \
  panel_surface_tests --test-threads=1
```

Run native acceptance after building the app, using a new output directory:

```sh
cargo build -p jcode-desktop
python3 scripts/accept-folder-panels.py target/live-tabs-acceptance-new
python3 scripts/screenshot.py target/live-tabs-review-new.png
```

## Limits and rollout gate

The broader serial suite reported 313 passed, 3 failed and 6 ignored. The failing
checks concern email inbox scrolling, restored transcript scrolling and the
swipe reticle. The baseline comparison was not completed, so they are not
claimed to be pre-existing failures. None is hidden by the targeted test run.

The desktop rebuild succeeded. Applying it to the user's existing window did
not: `--reload-ui` reached the live host, which reported
`hot reload is disabled; launch with --hot-reload`. No restart was performed
because unsaved drafts could be lost. Live-window delivery remains explicitly
blocked on permission to restart. No claim is made that the user's current
window already shows the redesign.
