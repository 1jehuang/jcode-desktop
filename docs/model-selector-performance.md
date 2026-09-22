# Model selector performance investigation, 2026-09-21

## Cause and changes

The model selector rebuilt a scrollable div containing every matching row on
hover, search, scrolling, and menu animation. Clipping hid offscreen content but
did not avoid building or laying out its text and controls. Broad searches and
expanded provider groups therefore made a pointer movement cost proportional to
the entire result set. Metadata generation also regrouped/ranked the catalog,
formatted every usage label, and scanned for the current model separately for
every suggestion. A selection-only change scheduled another geometry frame.

The selector now:

- Uses GPUI's variable-height virtual list. Headers remain attached to their
  first row, and keyboard indices still refer to selectable rows.
- Retains suggestion metadata across hover/repaint using a shared vector.
  Query, catalog, current-model and disclosure changes invalidate it. Relative
  usage labels refresh on interaction after the next minute boundary.
- Resolves the current route once per metadata build, instead of once per row.
- Uses indexed grouping and deduplication rather than repeated linear scans.
- Does not schedule a geometry follow-up for a highlight-only change.
- Anchors distant, unmeasured keyboard targets by item identity. Estimated row
  heights must not leave the selected route outside the visible viewport.

The rendered row and highlight still span the full menu width. Visual review
caught and corrected the initial virtual-list row's intrinsic-width sizing.

## Measurement method

`input::model_profile_tests::model_selector_frame_profile` is an ignored real
GPUI test. It creates a 1000x800 window with a bounded composer, installs
40/200/1000 synthetic routes, and alternates native mouse movement between the
first two visible rows. It checks viewport containment and selection, drains
scheduled work, explicitly draws the real window root and drains again. After
10 warmups it records 50 samples per catalog and collapsed/search scenario.

These are unoptimized debug CPU frame timings, not release GPU rendering,
compositor latency, physical input-to-photon measurements, or a production FPS
promise. Other development builds ran concurrently. The selected current model
is at the end of the catalog, exercising the formerly repeated route scan.

For the controlled comparison, a saved pre-change PromptInput was compiled as a
temporary test-only module alongside the optimized implementation, using the
same corrected profiler. Both used the indexed grouping implementation, so this
comparison isolates the input/cache/render changes rather than attributing the
small grouping improvement twice. The temporary baseline module is not shipped.
Raw evidence is in `target/model-selector-profile/controlled-comparison.log`.

Controlled comparison (milliseconds, 50 samples after warmup):

| Catalog | View | Before p50 / p95 | Virtual + cached p50 / p95 |
| --- | --- | ---: | ---: |
| 40 | Collapsed | 5.33 / 5.67 | 1.86 / 2.31 |
| 40 | Broad search | 43.74 / 49.04 | 3.56 / 3.77 |
| 200 | Collapsed | 5.01 / 5.12 | 1.53 / 1.58 |
| 200 | Broad search | 255.84 / 365.46 | 3.32 / 3.61 |
| 1000 | Collapsed | 14.06 / 15.59 | 1.43 / 1.46 |
| 1000 | Broad search | 1737.33 / 2246.69 | 3.46 / 3.65 |

The largest synthetic case is a stress test, not a typical catalog. The result
establishes removal of catalog-sized hover work, not a production speedup ratio.
Load average was approximately 15 during part of the comparison. A subsequent
small change also removed the unnecessary highlight-only follow-up frame.

Run the retained profiler with:

```sh
cargo test -p jcode-desktop-ui model_selector_frame_profile -- --ignored --nocapture
cargo test -p jcode-desktop-ui grouped_rows_profile -- --ignored --nocapture
```

The grouping microbenchmark retains the old algorithm in test-only code and
checks output parity. Results are mixed: hashing adds roughly 10 to 20 µs in
some 40-route cases, while larger cases can benefit. Concurrent build load also
varies the samples. This is not a universal grouping speedup and is not the
principal explanation for the frame-time improvement.

## Regression and acceptance checks

- Cache identity survives real hover and frames, while query, unavailable routes,
  current model, and disclosure changes produce fresh, correct suggestions.
- A 1000-route search paints fewer than 50 rows, keeps the last row reachable by
  keyboard, and clicking it submits its exact route even when a longer matching
  current model sorts first.
- Existing input tests cover draft/attachment restoration, grouped provider
  routes, usage refresh selection preservation, and tiny/resized popup bounds.
- The private-Xvfb screenshot harness exercises native search, pointer/keyboard
  handoff, group expansion/collapse, wheel scrolling, aliases and submission.

Final verification: 52 input tests and 102 model-related tests passed (the sets
overlap), plus 20 Python model-picker harness tests. All 16 native picker
acceptance checks passed on private Xvfb. The initial OCR failure disappeared
after correcting row width. Final optimized broad-search p95 values were
3.38 / 3.29 / 3.25 ms for 40 / 200 / 1000 routes. Artifacts include
`final-input-tests.log`, `final-model-tests.log`, `final-profile.log`,
`visual-full-width.log`, and `model-picker-full-width-model-open.png` beneath
`target/model-selector-profile/`.

The final source also passed all 16 native checks in `visual-final.log` and was
visually inspected at `model-picker-final-model-open.png`. The running release
host (PID 660899) activated UI generation 3 without a restart. Activation was
confirmed in the host log, and the staged library's SHA-256 matched the rebuilt
release UI, recorded in `final-reload.json`. The self-development tool could not
discover this single-panel host, so its existing private Ctrl+R socket was used.
The live preview catalog was read successfully without changing the user's UI.
