# Interaction latency profile

## Refresh-rate investigation, 2026-09-21

The production scheduler does **not** have a blanket 120 FPS cap. The current
Wayland backend follows compositor frame callbacks. An active window can use the
display's native cadence without estimating Hz from slow frames or adding an
application timer. GPUI deliberately throttles some unfocused animation to about
30 FPS and serious/critical thermal conditions to about 60 FPS. These are not
promises of delivered FPS. See [the platform source audit](docs/display-refresh-scheduling.md)
for exact pinned revisions, macOS behavior, and the separate X11 multi-monitor gap.

A passive 30-second capture against release host PID 346784 recorded 538 raw
draw samples and 52 input-bearing samples. Maximum draw time was **71.43 ms**,
maximum UI wake lag **112.36 ms**, and maximum input-to-frame latency **10.55 ms**.
Thirty of 336 sampling windows included a draw over the 8.33 ms budget for 120 Hz.
The median of nonempty windows' draw p95 values was 2.12 ms, not a pooled p95.
The process consumed 4.54 CPU seconds over 29.94 seconds. Load average was about
16 to 18 and CPU pressure reported about 15% at the end, with concurrent Rust
builds running. This establishes real long-tail stalls under load, not which
function caused them or how fast a quiet system would render.

Only **three animation intervals** were recorded, insufficient to characterize
sustained animation FPS. Another UI reload overlapped the capture, so raw counts
may overlap between samplers. The initial `--perf` attempt could not run because
the `perf` executable is absent. No CPU stack attribution is claimed.
Artifacts: `target/live-profile/refresh-investigation-timing/`.

An isolated release fixture on private Xvfb/lavapipe exercised 26 native overview
actions and 93 animation intervals. Idle produced zero draws over eight seconds.
During overview transitions, median per-window draw p95 was 5.76 ms, maximum
draw was 93.65 ms, and maximum wake lag was 637.92 ms under concurrent build load.
This is software-renderer workflow evidence, not performance on the Intel GPU
or a before/after speedup. Artifacts: `target/live-profile/refresh-offline-baseline/`.

The profiler now emits interval-weighted mean draw/presentation times, window
focus, thermal state, and a sampler identity. Analysis separates reload
generations, warns when animation samples are too sparse, and does not mistake
idle wall time for lost frames or invert a p95 as average FPS. Missing `perf`
falls back explicitly to timing-only capture. An exiting target preserves partial
evidence and marks the capture incomplete instead of losing it to a missing RSS
field. This handling was motivated by the original host exiting during a second
capture. No host restart was forced.

These are diagnostic improvements, not a claimed rendering optimization.
The next meaningful performance comparison needs sustained active interaction,
no overlapping reload, and documented build/system load.

### Follow-up with enhanced diagnostics

The state-preserving reload completed in remaining single-panel host PID 373219.
The emitted new fields confirm activation, rather than relying on the socket's
enqueue acknowledgement. A 30-second capture contained one sampler, 1,149 draws,
371 input-bearing frames and **623 animation intervals**. All animation intervals
were in focus-active sampling windows: their weighted mean was **13.89 ms
(72.01 FPS)**. Active-window draws averaged **5.91 ms**. Maximum draw across the
capture was **18.76 ms**, maximum wake lag **17.60 ms**, and maximum input-to-frame
latency **30.87 ms**. Thermal state remained `Nominal`. Inactive windows had no
animation interval samples, so their ordinary event-driven draws are not evidence
of an animation throttle. Focus is sampled at the window boundary, not per frame.

A read-only `wlr-randr --json` query reported the only enabled output, `eDP-1`, at
2880x1800, **120.000999 Hz**, adaptive sync disabled. No output configuration was
changed and no compositor-specific IPC command was used. This confirms a real
shortfall against the active mode, not a 60 Hz screen mistaken for 120 Hz.
Forty-seven of 114 focus-active sampling windows included a draw above 8.33 ms.
That establishes missed CPU draw budgets during part of the run. It does not
separate layout/text, GPU submission, compositor scheduling and other costs.
System load fell from about 11.9 to 8.2 and the process consumed 8.86 CPU seconds.
This different-window capture is not a controlled before/after speedup comparison.
Artifacts: `target/live-profile/refresh-single-panel-enhanced/`.

Validation: 20 Python profiling tests, two Rust live-profiler tests and eight
real-crate scroll cadence tests passed. The current debug fixture rendered and
was visually inspected at `target/ui-review-refresh-profile.png`; the enhanced
offline overview capture exercised 17 native actions and 40 animation intervals.
That software/debug run is validation of telemetry, not a GPU FPS benchmark.

## Loaded desktop stalls, 2026-09-12

A passive 25-second capture of the running desktop recorded 875 draws and
68 input-bearing frames. The maximum draw was 52.26 ms and maximum input latency
73.60 ms. Thirty-five sampling windows contained draws over 16.7 ms. Concurrent
Rust builds were active, so these are observed stalls, not an isolated benchmark.
Artifacts: `target/live-profile/fps-before-20260912c/`.

Two UI hot paths now avoid unnecessary work:

- Transcript notifications no longer discard every cached list measurement.
  Unchanged repaints invalidate zero rows, and stable streaming text/reasoning
  updates invalidate at most three trailing rows. Structural, tool, history,
  selection, and font changes retain conservative full invalidation. GPUI still
  measures visible rows and independently invalidates wrapping on width changes.
- The event bridge processes at most 128 updates before yielding for 1 ms, so a
  continuously ready producer cannot monopolize the UI executor. It preserves
  event order and leaves excess events queued, without reintroducing idle polls.
  Session refreshes also skip durable todo-file reads when no unfinished-work
  dashboard is open.

The real-panel measurement tests cover repaint reuse, streaming geometry, tool
updates, selection, recovery, and width changes. A 10,002-row measurement plan
invalidates 300 rows across 100 stable stream updates, rather than 1,000,200.
This is a reduction in explicit invalidation work, not a claimed FPS multiplier.
Timing comparisons between existing debug binaries were inconsistent under the
concurrent build load, so they do not establish an isolated frame-time speedup.

Graphics selection matters independently of transcript work. The affected Intel
machine had Mesa OpenGL and software Vulkan installed, but no Intel Vulkan ICD.
Install the matching `vulkan-intel` package on Arch to make hardware Vulkan
available. The running process keeps its graphics context until an application
restart, not a UI hot reload. UI activation now logs GPUI's actual GPU specs once
to the persistent desktop diagnostic log, including software-emulation status.
The screenshot harness deliberately forces lavapipe and cannot prove hardware
Vulkan presentation on the user's display.

The rebuilt release UI activated successfully in the running process, and its
adapter log confirmed hardware Intel Arc B390 rendering through Mesa OpenGL.
The private-Xvfb visual review is `target/ui-review-fps-20260912.png`. Eighteen
targeted queue, measurement, cache, streaming-scroll, tool, and image tests passed.
The broader panel run passed 185 tests with two failures outside these checks:
fresh-session composer geometry (also reproduced with the earlier binary) and
multiedit diff-boundary expectations. A post-reload passive capture overlapped
further UI reloads and contained concatenated JSONL records, so its failed
analysis is not evidence of an end-to-end FPS improvement. Raw artifacts remain
at `target/live-profile/fps-after-20260912/`.

## Sidebar hover, 2026-09-05

Session rows previously lived in one eagerly built scrollable div. GPUI's hover
style invalidated the workspace, rebuilding and measuring every history row,
including title shaping for rows outside the viewport. The sidebar now uses a
variable-height virtual list with 100 px overscan and initial height estimates.
Only visible rows are constructed. Layout-affecting changes invalidate cached
row heights without resetting scroll position. The file browser retains its
separate scroll handle, and the session scrollbar uses the virtual list state.

The debug-build `sidebar_hover_frame_profile` alternates actual GPUI mouse-move
events between two rows and includes the next constructed frame. With 10 warmups
and 50 measured samples, the before/after p95 results were:

| Sessions | Eager rows | Virtual rows |
| --- | ---: | ---: |
| 10 | 1.96 ms | 2.62 ms |
| 100 | 10.68 ms | 6.07 ms |
| 500 | 53.45 ms | 5.31 ms |

These are single-run headless CPU measurements, not physical input-to-photon or
live compositor latency. The test excludes transcript workload and machine load
can affect the timings. Run with:

```sh
cargo test -p jcode-desktop-ui sidebar_hover_frame_profile -- --ignored --nocapture
```

Non-timing regressions additionally assert that a 500-session hover measures
fewer than 50 titles, end-of-history remains reachable, title updates and added
history preserve the visible row, and metadata changes update row heights.
Existing UI tests cover wheel scrolling, clicking sessions, section ordering,
saved dividers, equal title heights, and the outer-gutter scrollbar placement.

### Live acceptance limitation

After reload, a 30-second passive capture against the actual running desktop
process on 2026-09-05 at 09:37 UTC returned **zero input-bearing frames**. The
capture is stored locally at
`target/live-profile/sidebar-acceptance-20260905/`. It confirms that the running
window's diagnostic boundary is reachable, but it cannot validate hover latency
or establish an improvement in the user's actual workspace. Other compilation
jobs were also active, making this unsuitable as an idle performance baseline.
No pointer input, focus changes, or compositor commands were injected into the
user's desktop. Live hover acceptance remains unverified until a capture includes
the affected interaction. The headless improvement above is not a substitute for
that missing observation.

The exact committed source passed the sidebar regressions. The broader suite
had five failures in inbox scrolling, restored transcript scroll, gesture
reticle rendering, the right-edge click target, and shortcut showcase text.
The same five failures were reproduced on untouched pre-fix commit `24ce6e7`
in an isolated source copy, distinguishing them from this sidebar change.

## Live self-development profile

Self-development launches (`--hot-reload`) show a compact live performance
badge in the upper-right corner. It reports the rolling p95 event-loop wake lag,
UI render construction time, and the worst retained sample. The badge turns
yellow when either budget is exceeded and red at twice the budget. Samples are
bounded and the display refreshes four times per second to avoid becoming a
source of continuous repaint overhead.

The badge is intentionally absent from normal launches. To diagnose a packaged
or non-hot-reload build, opt in with `JCODE_DESKTOP_PERF=1`.

The badge reports GPUI's direct p95 draw and presentation intervals, effective
presented FPS, input-to-frame latency, view-construction time, coalesced input
events, and inputs that arrived during a draw. When `JCODE_DESKTOP_STATE` is
set, the same values are appended to the public diagnostic state so
compositor-driven acceptance runs can inspect them without screen scraping.
Recorded presentation packets remain an independent end-to-end check of the
application, Wayland, Vulkan, and compositor boundary.

For action-scoped measurements against the exact restored workspace, set
`JCODE_DESKTOP_PERF_ACTIONS=/path/to/actions.jsonl`. Each meaningful focus-left,
focus-right, focus-up, or focus-down action appends one JSON record after its
animation settles. Draw, presentation, and input p95 values are computed from
the GPUI histogram samples added during that action, rather than from the
window's earlier history. The record also includes elapsed settling time,
presented and constructed frame counts, and missed 17.5 ms construction
budgets. Edge no-ops are intentionally excluded.

Presentation fields are nullable. GPUI only adds presentation-interval samples
when the compositor reports consecutive active animation presents. In isolated
headless Sway acceptance this histogram can remain empty even though draw and
input-to-frame samples prove that frames crossed the application/Wayland
boundary. Such records use `null` for `present_p95_ms` and `presented_fps` and
zero for `presented_frame_count`; they must not be interpreted as 0 FPS. Run on
the target interactive compositor to obtain user-visible presentation cadence.

## Development-build runtime

The hot-reload workflow keeps Jcode Desktop's own crates and most dependencies
at `opt-level = 0`, but builds a profile-selected list of runtime-hot
dependencies (taffy layout, GPUI, text shaping, the Wayland/Calloop event loop,
and the renderer) at `opt-level = 2`.
The UI-independent animation registry and per-frame interpolation primitives live
in `jcode-desktop-motion`, which is also built at `opt-level = 2`. The frequently
edited GPUI composition remains in the unoptimized, dynamically reloadable UI
crate, while the small UI adapter applies the current reduced-motion setting.
GPUI, Calloop, Wayland dispatch, text shaping, and rendering dominate idle and
paint-time CPU. Leaving those dependencies unoptimized caused the development
host to consume 16–40% of one CPU core while idle on the test machine. A
sampled live profile attributed the work to the GPUI timer scheduler,
Calloop/Wayland dispatch, and atomic/task bookkeeping rather than application
state handlers. With only the profile-selected crates optimized, the rebuilt
debug executable used 3.7% CPU over a 10-second steady-state sample in an
isolated headless Sway compositor (a full `opt-level = 2` dependency build
measured 3.0%, so the selective list captures the win while leaving most of the
graph fast to compile). This is development-only overhead: the release
interaction profile below was already fast.

Two 16 ms polling boundaries remain intentional. The workspace bridge drains
streaming session updates at up to 60 Hz, and each embedded terminal drains PTY
output at up to 60 Hz. Empty polls do not notify or repaint, but they do wake the
executor. If idle power becomes a release concern, replace these polls with
event-driven wakeups rather than increasing their interval and adding visible
streaming or terminal latency.

The Jcode runtime log is separate from the desktop renderer. `TUI_SLOW_FRAME`
records found during this investigation described terminal Jcode sessions
rebuilding multi-megabyte, roughly 2,050-line transcripts in 70–82 ms. Those
warnings explain lag inside a TUI session, but are not desktop window frames.
Repeated skill-frontmatter and stale Claude OAuth warnings likewise add log
noise but are not on the desktop render path.

Measured on 2026-08-22 on a Dell XPS 13 9350 (Intel Core Ultra 7 256V), Linux x86_64, using an optimized build and GPUI's headless interaction harness.

## Method

The opt-in `interaction_latency_profile` test drives the real key bindings through GPUI into a workspace containing 32 panels across four strips. It records 500 input-to-state samples after 50 warmups for each operation. Three independent runs used isolated state directories.

Run it with:

```sh
cargo test --release -p jcode-desktop-ui interaction_latency_profile -- --ignored --nocapture
cargo test --release -p jcode-desktop-ui loaded_animation_first_frame_profile -- --ignored --nocapture
```

These measurements include key parsing, action dispatch, workspace mutation, focus updates, learning-model updates, and learning-state persistence. They exclude the physical keyboard, compositor, GPU presentation, and the subsequent animation frames. Policy-driven visual settling is listed separately.

The loaded first-frame profile constructs 32 panel entities across four strips
and includes GPUI's next rendered frame after each focus change. Persistent
coach-state filesystem writes run on a coalescing worker, and settled transcript
rows are borrowed rather than cloned during rendering. Panel entities remain
GPUI cache boundaries, so a workspace strip translation does not require
reparsing unchanged transcript markdown.

## Results

| Interaction | p50 range | p95 range | p99 range | Visual settle policy |
|---|---:|---:|---:|---:|
| Horizontal focus | 0.82–1.31 ms | 0.92–1.79 ms | 1.10–1.95 ms | 150 ms |
| Vertical focus / strip transition | 1.31–2.25 ms | 1.40–3.54 ms | 1.69–3.66 ms | 150 ms |
| Horizontal panel move | 0.82–1.42 ms | 1.34–1.94 ms | 1.54–2.85 ms | 150 ms |
| Panel resize preset | 1.31–1.43 ms | 1.77–1.81 ms | 1.90–1.96 ms | 150 ms |

## Findings

1. **State response is fast.** Every measured path remained below 3.7 ms at p99, comfortably inside one 60 Hz frame (16.7 ms) and one 120 Hz frame (8.3 ms).
2. **Perceived latency is animation-policy dominated.** Movement and focus state changes happen in roughly 1–3 ms, but the visible camera, row, order, and width transitions intentionally settle over 150 ms. Modal transitions settle over 180 ms.
3. **Vertical focus is the most expensive measured path.** It updates row selection, creates a row animation, changes focus, updates the coach, and renders incoming and outgoing strips during the transition. Its worst observed p99 was 3.66 ms, still well within frame budget.
4. **Learning persistence is on the interaction hot path.** Successful navigation, movement, and resize shortcuts synchronously serialize and write coach state, making persistence and coach bookkeeping the clearest optimization target if tail latency becomes visible on slower storage.
5. **No CPU-side interaction bottleneck was found on this machine.** Reducing the 150/180 ms policy durations would change perceived snappiness far more than micro-optimizing handlers, but that is a product-motion decision rather than a correctness fix.

## Next measurement layer

A physical input-to-photon profile should use compositor presentation timestamps or a high-speed camera. It should record key event arrival, first changed frame, frame pacing throughout the 150 ms animation, and final presentation. The headless profiler intentionally remains deterministic and does not disturb the active desktop session.

## Acceptance coverage

The release binary was launched through its public CLI (`jcode-desktop --no-sidebar`) on an isolated headless Sway compositor. `wtype` injected real Wayland keyboard events, local terminal panels exercised the host/UI/resource boundary, GPUI rendered the resulting surfaces, and the public `JCODE_DESKTOP_STATE` diagnostic supplied the observable acceptance state. This kept the test entirely separate from the user's active compositor.

The observed end-to-end state-change times were 15.8 ms for focus-left, 8.3 ms for focus-right, and 14.9 ms for moving a panel left. Each operation produced the expected externally visible focus position in the running release binary. These are conservative single-sample bounds because they include launching `wtype` and shell polling at roughly 1 ms intervals. They corroborate the micro-profile's conclusion that the state change arrives within one 60 Hz frame, while the deliberate 150 ms animation controls final settling.

An earlier Xvfb attempt failed because GPUI's Vulkan presenter requires DRI3, which Xvfb does not provide. The headless Wayland run closes that presentation gap. This still does not measure a physical keyboard or monitor scanout, so it is presentation-path acceptance rather than a physical input-to-photon claim.

## 2026-08-24 reprofile

The optimized headless interaction profile was repeated after the session-ordering
and reconciliation changes. All p99 state transitions remained below 3.5 ms:

| Interaction | mean | p95 | p99 |
|---|---:|---:|---:|
| Horizontal focus | 1.62 ms | 1.90 ms | 2.82 ms |
| Vertical focus / strip transition | 2.70 ms | 3.17 ms | 3.48 ms |
| Horizontal panel move | 1.83 ms | 2.51 ms | 3.33 ms |
| Panel resize preset | 1.64 ms | 2.03 ms | 2.25 ms |

A direct 10-second `/proc` sample of the running hot-reload desktop measured
0.10% process CPU after startup. The persistent diagnostics log also showed that
session reconciliation is the dominant recurring background operation. Its
historical samples include 100-session refreshes ranging from roughly 0.2 to
0.65 seconds in recent runs. This work is performed off the UI thread and is
coalesced, so it does not impose that duration on interaction handling.

The same log contains CPU warnings from older, long-running debug generations.
Those warnings measure whole-process utilization in five-second windows, not UI
event latency, and should not be interpreted as slow frames. The old
`~/.cache/jcode/desktop/performance.log` predates the current desktop telemetry
and is retained only as historical evidence.

The terminal output poll remains at 16 ms to preserve one-frame output latency.
Its 32 KiB read slab is now retained for the terminal lifetime instead of being
allocated on every idle poll, removing approximately 62 allocations per second
per open terminal without increasing latency.

The rebuilt release binary was also exercised through its public
`jcode-desktop --no-sidebar` entry point in a fresh state directory on an
isolated headless Sway compositor. A real compositor window and the public
diagnostic state both appeared after 259.5 ms. The process remained alive,
produced no lag warnings, and averaged 1.13% CPU during the following 15-second
idle sample. This validates the release host, GPUI Wayland renderer, UI plugin,
state diagnostic, and persistent diagnostics boundaries together without
touching the active desktop session.
