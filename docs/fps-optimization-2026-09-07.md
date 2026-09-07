# Desktop FPS profiling and optimization, 2026-09-07

## Delivered changes

1. **Bulk ordinary inline text.** CPU sampling implicated `markdown::inline_spans`
   among the rendering costs. Its old scanner constructed and checked formatting
   styles for every ordinary character. The scanner now copies UTF-8 prose runs
   up to the next possible syntax byte. Formatting, escapes, math, URLs, links,
   nested styles, selection offsets, and incomplete streamed syntax use the same
   parser as before. No document cache, stale theme state, or unbounded storage
   is introduced.
2. **Retire hidden strip transitions.** Resizing a panel and opening overview
   before its transition finished left width/order/camera animations marked active.
   Only strip rendering sampled those values, but overview stopped rendering the
   strip. The fallback timer consequently scheduled frames indefinitely. Hidden
   strips now advance to their natural deadlines. Visible and outgoing strips
   retain their existing animation sampling.

Both fixes have offline-only same-binary controls. Neither control is honored in
ordinary live sessions. The scalar parser is also retained for differential tests.

## Measured evidence

Initial passive live capture: `target/fps-sep7-live-baseline`, PID 911314, 20 seconds.
Maximum draw duration was **13.50 ms**, maximum input-to-frame **20.55 ms**, and
maximum event-loop wake lag **14.26 ms**. These draw tails can exceed a 120 Hz
frame's 8.33 ms budget. The capture had 920 sampling windows rather than roughly
200, so aggregate draws and intervals are **not valid live FPS evidence**.

An additional 20-second userspace `perf` capture collected 1,313 CPU samples.
Self samples included GPUI bounds-tree insertion (2.51%), text raster bounds
(1.14%), flex layout (0.91%), and inline parsing (0.53%), with costs dispersed
across allocation and generic helpers. This justified testing parser work, not
claiming it accounts for all lag. Symbol-only reports completed. Full call-graph
report expansion timed out and is not used as evidence.

### Same-binary native application comparisons

All following captures use private Xvfb/Openbox, Mesa lavapipe, four rich offline
panels, native keyboard events, and the same binary. They do not send input to the
user's display or connect to a daemon. CPU is percentage of one core and can
exceed 100% because this is a multithreaded software renderer.

| Scenario | Old behavior | Optimized | Result |
| --- | ---: | ---: | --- |
| Resize then overview, first pair: post-transition draws | 141 | 1 | Persistent redraw loop eliminated |
| Same action, reverse-order repeat: post-transition draws | 143 | 1 | Same result in reversed order |
| Resize then overview: whole 6-second process CPU | 187.7% | 12.9% | About 93% lower CPU in this scenario |
| Same action, repeat: whole 6-second process CPU | 193.8% | 13.6% | About 93% lower CPU |
| Full panel rerender, first pair: median window draw p95 | 10.12 ms | 8.36 ms | 17% lower typical draw cost |
| Full panel rerender, reverse-order repeat | 9.00 ms | 8.45 ms | 6% lower typical draw cost |

Post-transition counts exclude the first 1.5 seconds, including the intended
animation. One late redraw remains, rather than claiming completely zero draws.
The repeated native tests verify both overview state and the selected panel's
width changing from 0.25 to 0.5. Both versions retain 8–9 animation presentation
samples during the intended transition. The fix does not disable animation.

The parser comparisons disable the existing panel cache **in both arms** to
exercise full transcript rerendering, as occurs when panel content invalidates.
They are not proof of a matching gain during already-cached focus switching or
streaming on the user's GPU. Each 8-second run verified 27 native focus actions
and both focused slots. CPU did not consistently improve for the parser change,
and input latency tails did not consistently improve. All ordinary idle phases
produced **zero draws**.

`draw_window_p95_median_ms` is the median of per-sampling-window p95s, not a pooled
p95. `draw_rate` is draws divided by wall time, not animation FPS. No fixed live
FPS multiplier is claimed.

The six alternating-order same-process parser microbenchmark trials processed
100 copies of a 20,096-byte mixed Unicode/markdown paragraph. Bulk scanning took
35–45 ms versus 320–374 ms, an **8.4–10.3× parsing speedup** in the development
build. This is a component benchmark, not a whole-window FPS multiplier.

Artifacts:

- `target/fps-sep7-live-{baseline,stacks}` and `target/fps-sep7-all-symbols.txt`
- `target/fps-sep7-hidden-{stale,fixed}{,-repeat}`
- `target/fps-sep7-inline-{scalar,bulk}{,-repeat}`
- `target/fps-sep7-inline-profile.log`

Each native capture records the executable SHA-256, renderer, switches, summaries,
raw frame windows, navigation state, and screenshot. Raw captures remain local.

## Reproduce

```sh
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/profile-render.py target/fps-stale --scenario overview-after-resize --stale-hidden-animations --seconds 6
python3 scripts/profile-render.py target/fps-fixed --scenario overview-after-resize --seconds 6
python3 scripts/profile-render.py target/fps-scalar --scenario focus-switch --uncached-panels --scalar-inline
python3 scripts/profile-render.py target/fps-bulk --scenario focus-switch --uncached-panels
cargo test -p jcode-desktop-ui inline_prose_profile -- --ignored --nocapture --test-threads=1
cargo test -p jcode-desktop-ui -- --test-threads=1
python3 -m unittest discover -s scripts -p 'test_profile_*.py'
python3 scripts/screenshot.py target/ui-review.png
```

Finish builds and other captures before comparing timings. Repeat pairs in both
orders. Output directories must not exist. The switches are diagnostic controls,
not production configuration options.

## Correctness and remaining uncertainty

- Entire serial UI suite: **684 passed, 0 failed, 8 ignored**.
- Markdown suite: 31 passed plus the manual benchmark ignored by default.
- Differential parser test compares all outputs at every UTF-8 streaming boundary
  of representative inputs and 1,000 deterministic malformed mixed-syntax strings.
- Two hidden-animation tests verify expiration, final camera/width/order state,
  ongoing hidden animation deadlines, and unchanged visible/outgoing row sampling.
  The original regression fails with the same binary's stale control enabled.
- Three overview and two vertical-navigation regressions passed independently.
- Fifteen Python profiling tests passed, including the post-transition boundary
  and missing-sample rejection.

The duplicate live sampling issue remains unresolved. Actual isolated cdylib
reloads did not reproduce it. See [the separate investigation](reload-lifecycle-2026-09-07.md).
No GPUI arena root cause or memory-leak fix is claimed. The live app was also
restarted independently during this work, so a later capture cannot establish a
causal live before/after performance result. The paired same-binary captures
above establish the specific improvements without relying on those live changes.


## Final delivery checks

The paired native evidence above used SHA-256
`a3754dbd5178c13f0facd411c16ea52ce29c4ac401cba36a78b80acc6c694383`.
Additional default-cache captures (`target/fps-sep7-production-*`) had different
executable hashes because concurrent rebuilds occurred. They are excluded from
causal performance comparisons.

The final real-app screenshot, `target/fps-sep7-ui-review.png`, was rendered on
a private display and visually inspected. Inline styling, lists, table, syntax
highlighting, math, composer, sidebar, and panel chrome remained intact. The
standard `target/ui-review.png` path was already occupied, so the harness safely
refused to overwrite it and the task-specific path was used instead.

The verified UI was rebuilt and the running host's Ctrl+R rebuild-and-reload
path was invoked through its instance socket. Successful UI generation
activation was confirmed in the desktop diagnostics. Only task changes were
committed, leaving unrelated concurrent edits intact.


## Requirement-to-acceptance map

The user requested profiling and optimization, without specifying a numeric FPS
threshold. The checks below cover that request and every behavior changed by
this patch. They do not convert the separate unresolved live-sampler issue into
a claim of an overall hardware FPS gain.

| Requirement or changed public behavior | Concrete public-interface/acceptance check | Observed result |
| --- | --- | --- |
| Profile the real desktop rather than guess at hotspots | Passive `profile-live.py` capture of the running process, plus userspace perf sampling | 13.50 ms maximum draw, 20.55 ms maximum input-to-frame, 1,313 CPU samples. Duplicate sampler output identified and excluded from FPS totals. |
| Reduce actual rendering work | Same-binary real app, native focus changes, scalar/bulk controls, reversed execution order | Median window draw p95 fell 10.12→8.36 ms and 9.00→8.45 ms. Each arm verified 27 input-bearing actions and actual focus changes. |
| Stop hidden animation work without removing visible animation | Real native resize then overview, followed by no input | 141→1 and 143→1 post-transition draws. Width changed 0.25→0.5, overview changed state, and both arms retained 8–9 intended animation presentation samples. |
| Preserve markdown text, formatting, links, math, and streaming prefixes | Differential parser test at every UTF-8 boundary and 1,000 mixed malformed-syntax inputs, plus real rendered screenshot | All plain text, byte ranges, highlight styles and links match the scalar parser. Rich markdown/code/math screenshot visually passed. |
| Keep visible/outgoing panels and camera endpoints correct | `hidden_strip_sampling_preserves_visible_rows_and_natural_deadlines` and native panel moves | Test passed for ongoing hidden deadlines, final width/order/camera positions, and unchanged visible/outgoing sampling. Native moves and focus across four workspace rows passed. |
| Preserve cache boundaries and descendant input/stream invalidation | Three `panel_cache_tests`: descendant composer notifications, streaming/resize, unrelated workspace notifications | All three passed in the full serial UI suite. |
| Preserve scroll routing | Four `hover_scroll_tests`, covering wheel/touchpad and empty/populated active panels | All four passed, including scrolling only the hovered panel. These are actual GPUI integration regressions, not native X11 scroll captures. |
| Preserve user keyboard focus and rendered FPS-header geometry | `screenshot.py --fps-header-interact --panels 4` on the pinned final binary | Native tab clicks, panel moves and keyboard row navigation passed. All four rendered headers had centered unclipped text, height20, and matching keyboard-panel/workspace IDs. |
| Preserve session selection and composer editing | `screenshot.py --history-interact`, followed by inspection of the actual screenshot | Repeated native history selections retained session and keyboard focus. Typed `history click typing works` is visibly present in the selected History session06 composer and absent from the other composer. |
| Do not add idle redraws | Idle phase in every controlled native capture | Zero draws in all eight initial/reverse-order phases. |
| Deliver the verified changes without stale UI | Real host Ctrl+R rebuild-and-reload through instance socket, persisted activation diagnostic, Git remote check | Final UI generation4 activated successfully. Optimization/report commits were pushed; unrelated concurrent edits remain uncommitted. |

Additional native acceptance artifacts are
`target/fps-sep7-native-header-final.fps-header.json`, its four workspace PNGs,
`target/fps-sep7-native-history-final.png`, and matching `.log` files. The binary
was copied before these tests to avoid concurrent build replacement, SHA-256
`e366f4c9147a28f0fdb4921ac12958255ee94368a9d3e1ee04c3b46e50380d47`.
These later native checks validate integration/correctness, not a timing
comparison against the earlier profiling binary. Test code does not replace
live GPU acceptance, and no such performance claim is made.
