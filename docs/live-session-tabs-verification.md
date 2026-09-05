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

## Whole-result rerun after establishing the map

A fresh run on 2026-09-05, starting at 22:13 UTC, exercised the full UI test suite,
rebuilt Desktop, ran the complete native workflow, rendered the current app,
and repeated the raster/OCR measurements. Artifacts are under
`target/live-tabs-whole-result/` and `target/live-tabs-whole-native/`.

All eight mapped tests ran and passed **inside the full suite**, not merely in
a cached executable replay:

| Mapped requirement/output | Fresh observed outcome |
| --- | --- |
| Unnamed/reconnecting identity | `folder_header_keeps_an_identity_before_a_session_is_named`: PASS |
| Five live-state dots and removal of stale dots | `minimap_paints_live_state_and_todo_progress_through_the_workspace_surface`: PASS |
| Empty workspace identity and return to a live session | `empty_workspace_does_not_mark_another_rows_session_as_focused`: PASS |
| Normal-layout panel clearance and minimap clearance | `live_tabs_reserve_space_in_normal_mode_and_beside_minimap`: PASS |
| Long-title overflow and keyboard reveal in an 800px window | `live_tabs_reveal_keyboard_selection_in_a_narrow_window`: PASS |
| Folder baseline, active-tab height and cross-row selection | `live_tabs_switch_rows_and_keep_the_folder_baseline`: PASS |
| Resize, sidebar visibility and equal panel baselines | `folder_panels_share_a_level_body_without_individual_tabs`: PASS |
| Switching between independent normal cards and folder sheet | `normal_mode_uses_separate_equal_height_panels_without_the_folder_surface`: PASS |
| Real tab selection | Native click changed slot 2 → 0 and keyboard panel → 0, kept the same three real session IDs and closed overview |
| Sidebar separation at every focus position | Native raster checks passed at focus 0, 1 and 2, after the tab click, after overview and after layout restoration |
| Existing surrounding workflows | Native panel clicks, overview hover/selection, sidebar scrolling, Settings navigation, Normal/Folder switching and persistence all passed |
| Old rounded marker removed | Fresh raster: 0 non-backing pixels in the old marker bounds, versus 326 before |
| Wide top shoulder removed | Fresh raster: 0 sheet pixels in the sampled shoulder, versus 15,000 before |
| Sidebar no longer forced to connect | Fresh raster flood fill did not reach the selected sidebar row; all 968 gutter pixels use the backing color |
| Readable live-session label | Fresh OCR reads `Review markdown renderin..` in the tab. Full panel-header OCR reads `Review markdown rendering with a very long session title` |
| Build and current native rendering | Both succeeded |
| Applying to the user's existing window | Still blocked by the live host's explicit hot-reload-disabled response; no restart performed |

The exact-title OCR assertion initially failed because a concurrent change
lengthened the screenshot fixture title. The new output correctly uses an
ellipsis, rather than hiding the identity or overflowing the tab. Both the
original failed exact-match check (`measurements.json`) and the corrected
prefix/full-header observations (`title-check-corrected.json`) are retained.

The full suite's overall result was **312 passed, 3 failed, 6 ignored**. Its three
failures remain the email inbox scroll, restored transcript scroll and swipe
reticle checks described below. Separate redundant targeted retries encountered
in-progress HTML-preview compilation errors, then SIGTERM during compilation.
Those retries are not counted as passes; the eight PASS results above are from
`full-suite.log` in this new run.

Source fingerprints before/after are retained. Nine files changed concurrently,
including the screenshot fixture and unrelated preview/activity work, so this is
not presented as a frozen release certification. It is a whole-result rerun of
every mapped check, with changing-input and failed-check evidence preserved.

## Limits and rollout gate

The earlier broader serial suite reported 313 passed, 3 failed and 6 ignored;
the latest whole-result run reported 312 passed, 3 failed and 6 ignored. The failing
checks concern email inbox scrolling, restored transcript scrolling and the
swipe reticle. The baseline comparison was not completed, so they are not
claimed to be pre-existing failures. None is hidden by the targeted test run.

The desktop rebuild succeeded. Applying it to the user's existing window did
not: `--reload-ui` reached the live host, which reported
`hot reload is disabled; launch with --hot-reload`. No restart was performed
because unsaved drafts could be lost. Live-window delivery remains explicitly
blocked on permission to restart. No claim is made that the user's current
window already shows the redesign.

### Delivery follow-through, 22:28 UTC

The original request is an experiment with folder-style live-session tabs, not
proof of a subjective aesthetic improvement. The measured removal and native
selection checks support the requested behavior, but user preference is still
unconfirmed. The three wider failures have an unestablished cause, not a proven
absence of relation to these changes.

Rechecked the live host and supported recovery paths without disturbing it:

- PID 3080233 still runs without `--hot-reload`.
- The instance protocol in `src/host/instance.rs` exposes only Show and Reload,
  not snapshot export, enabling reload, or a state-preserving process restart.
- `ReloadManager::reload` rejects a missing plugin source before snapshotting.
- Suspend/resume retains snapshots in host memory and reactivates the same
  generation. Closing and reopening the surface cannot install the new build.
- Drafts, attachments and terminal resources survive in-process handoff, but
  this does not establish that they survive terminating this host.

Live activation therefore remains an outstanding blocked task, not a cancelled
requirement or a successful delivery. Repeating the rejected reload or replacing
the process without establishing state preservation would not close that gap.
No runtime changes or additional build were needed for this evidence correction.

## Compact, always-fitting tabs (2026-09-05)

The live-session strip no longer scrolls horizontally or imposes a 100px minimum
per tab. Tabs share the available width with a 144px cap, 11px labels, reduced
padding and gaps, and full-title hover tooltips. Decorative status dots and row
numbers yield space at very narrow widths. The active folder's height and join
with its panel are unchanged. Sizing uses the actual canvas width, including
sidebar, margins, and minimap reservation. Flex distributes fractional pixels
instead of rounding each fixed-width tab and accumulating overflow.

Verification:
- `cargo test -p jcode-desktop-ui live_tabs --lib`: 5 passed. Coverage includes
  first/last keyboard navigation, mouse selection, full-title tooltip appearance,
  all 12 tabs visible through 480/640/800/1000/1440px window resizes, 24 tabs beside
  the minimap in normal mode, empty-strip selection, and the folder baseline.
- Sizing arithmetic checked with 1–200 tabs across six available widths.
- Real-app private-Xvfb captures inspected at `target/ui-review-compact-tabs.png`
  (1000×700) and `target/ui-review-compact-tabs-final.png` (800×700), each with six
  tabs. All tabs remain within the strip with ellipsized labels.
- Full UI suite, serial: 331 passed, 3 failed, 6 ignored. The failures are the
  previously documented email-inbox scroll, restored-history scroll, and touchpad
  reticle tests (see `theme-settings-verification.md`). A parallel run also hit
  the timing-sensitive vertical-key animation assertion. The suite is not green.
- The desktop executable and UI plugin rebuilt successfully. The live instance
  acknowledged the Ctrl+R-equivalent command, but activation was rejected with
  `hot reload is disabled; launch with --hot-reload`. Its active sessions were
  preserved rather than force-restarting the process. Applying this build to that
  window remains blocked until a safe restart.

### Real-session acceptance follow-up

The fixture and geometry checks were followed by the actual desktop, SDK, API
bridge, and daemon on a private Xvfb display, with no screenshot-fixture mode and
no inference requests:

```sh
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/accept-navigation.py target/compact-real-fixed --compact-tabs --reloads 1
```

Observed result: **PASS**, with 12 distinct daemon-created sessions at 800×700,
131 state checkpoints, and 26 native top-tab clicks. Every tab was selected once
plus a return to the first tab, both before and after a native Ctrl+R rebuild and
activation of generation 2. Every click preserved all 12 sessions in order and
left keyboard focus in the selected composer. No horizontal tab scrolling was
used. The final real-session screenshot `compact-tabs-g1.png` was inspected.
The available tab row is 512px wide, where the old 100px minimum could not fit
12 tabs. All 12 now remain visible and individually clickable.

This stronger check found two issues that geometry-only tests missed:

1. The invisible right-edge add-session button overlapped the last tab. Clicking
   that tab also created an unwanted thirteenth real session. Its hit area now
   starts below the tab strip. The regression test checks both non-overlapping
   bounds and that clicking the last tab does not invoke the spawn path.
2. A plugin built separately from the host did not receive native mouse events,
   while the linked UI did. GPUI mouse listeners downcast shared Rust event types.
   Rebuilding host and plugin together preserves host dependency-feature
   unification and resolves this cross-library dispatch failure. The acceptance
   run now demonstrates working native tab clicks across actual plugin reloads.

The five focused tab tests, the edge add-session regression, and six Python
navigation-check tests all pass. The previously reported full-suite failures are
not represented as resolved by these targeted checks. All temporary mouse-event
tracing used during diagnosis was removed.

The exact feature difference was subsequently isolated: the host enables
`libc/extra_traits` through its PTY dependencies, while a standalone UI build did
not. The UI now explicitly enables that feature too, retaining compatibility with
already-running hosts whose reload command builds only the UI package.

Compatibility was verified by running the **exact live host executable** via
`--binary /proc/4149623/exe` on the private display. That second real-session run
(`target/compact-live-host-compat`) also passed all 131 checkpoints and 26 tab
clicks across a native Ctrl+R reload. The final feature configuration again passed
all five tab tests and six Python navigation-check tests.

**Live activation blocker resolved:** the desktop was subsequently relaunched
with hot reload enabled. After compatibility verification, its queued
Ctrl+R-equivalent rebuild successfully activated UI generation 2 in live PID
55458. This supersedes the earlier note that the change was built but not live.
