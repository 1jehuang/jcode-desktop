# Interaction latency profile

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
