# Compact transcript frame data, 2026-09-12

## Result and limits

Transcript row descriptors shrink from **160 to 48 bytes** on x86_64. A
10,000-message frame now reserves about **0.48 MB instead of 1.60 MB** for its
row vector, a 70% reduction. Three alternating-order pinned-test comparisons
measured **36–38% less time constructing those rows**. This is a component
improvement, not a claim of a corresponding hardware FPS increase.

Whole-frame results are mixed, including regressions. Native software-renderer
measurements below do **not** establish a consistent end-to-end FPS gain.

## Profile-first investigation

The existing `transcript_repaint_profile` first measured 100/1,000/10,000-row
repaint p50s of 2.157/2.321/4.095 ms. Inspection found that `TranscriptRowSource`
embedded the entire largest `Item` variant even when a settled row contained
only an index. All descriptors are reconstructed on a panel repaint.

A passive 15-second live capture was attempted against PID 6838. `perf` was
unavailable. The no-perf attempt encountered concatenated JSON objects from the
already documented duplicate live samplers. Decoding those objects separately
found 718 records, maximum draw 6.08 ms, maximum wake lag 14.50 ms, and no input
latency samples. These are diagnostic observations only. Neither aggregate FPS
nor interactive latency can be inferred from that capture. The first short
native focus probe overlapped test compilation near its end and is excluded
from performance comparisons.

## Implementation

`Settled(usize)` stays borrowed. Only live text and coalesced reasoning use
`Owned(Box<Item>)`, so rare owned content no longer sets the size of every row.
Adjacent restored reasoning still appends to one owned block and keeps the
first source index. Source transcript items remain untouched. Cache boundaries,
list measurement invalidation, scrolling, animation timing, and rendering are
unchanged. There is no new persistent cache or invalidation policy.

The existing manual profile now reports descriptor size and the average time
for 1,000 row-construction iterations. A screenshot-only `long-history` fixture
seeds 10,000 alternating formatted user/assistant messages. The native profiling
script exposes that fixture without changing ordinary sessions.

## Repeated component measurements

Pinned executables were run before/after, after/before, then before/after, with
no builds launched by this task during the comparisons. Other desktop activity
and concurrent development remain uncontrolled.

| Trial | Old row construction | Compact construction | Old repaint p50 / p95 | Compact repaint p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 446.418 µs | 283.559 µs | 6.486 / 7.174 ms | 4.890 / 9.531 ms |
| 2 | 428.643 µs | 271.279 µs | 6.348 / 7.765 ms | 5.947 / 11.490 ms |
| 3 | 435.572 µs | 270.602 µs | 6.618 / 7.127 ms | 6.035 / 7.079 ms |

All rows above use 10,000 messages. Row construction improved consistently.
Repaint p50 improved 6–25%, but p95 worsened in two trials. The initial separate
build runs were also mixed: row construction 321.702 → 303.196 µs, repaint p50
3.976 → 5.788 ms and p95 4.261 → 6.050 ms. These contrary samples are retained,
not discarded in favor of a faster result.

Artifacts: `target/fps-sep12-{before,after}-repeat-{1,2,3}.log`,
`target/fps-sep12-rows-{before,after}.log` and the initial
`target/fps-sep12-transcript-before.log`.

## Native application comparisons

Private Xvfb/Openbox, Mesa lavapipe, four long-history panels, eight seconds of
native focus switching. Both arms disable the existing panel cache to exercise
full panel rebuilds. This represents invalidated panels, not cache-hit focus
switches. No input was sent to the user's desktop.

| Order | Old median window draw p95 | Compact median window draw p95 | Old / compact max input latency |
| --- | ---: | ---: | ---: |
| Old then compact | 27.099 ms | 40.960 ms | 94.044 / 90.243 ms |
| Compact then old | 38.863 ms | 15.614 ms | 86.573 / 45.744 ms |

Every arm verified both focused panel states and 26–27 input-bearing frames.
All four idle phases recorded **zero draws**. Animation-presentation histograms
were empty. Draws per second are not presentation FPS, and the median of window
p95s is not a pooled percentile. CPU and wake delays also varied considerably.
The differing trial results prevent a reliable whole-window speedup claim.

Artifacts: `target/fps-sep12-long-{before,after}{,-repeat}/`, including raw
frames, navigation state, screenshots and summaries. Pinned executable SHA-256:

- Before: `1bc5e0c369b5e94e45a333534f2579b2afda9de657644d13f7e963b10a30d216`
- After: `51b4f58ce868b7fef711b9156799ae2ccf13ef97b4a1db7fa2a0818cf814462b`

## Correctness and reproduction

- Full optimization-only UI suite: **705 passed, 0 failed, 8 ignored**.
- Python profiling tests: **15 passed**.
- New regression checks the descriptor size bound, settled source indices,
  skipped todos, three coalesced Unicode reasoning segments, independent live
  reasoning/answer rows, unchanged source text, and speaker-caption behavior.
- Existing tests exercise live-to-settled reasoning paint geometry and copying,
  panel cache invalidation, descendant composer edits, streaming, resize,
  restored-history scrolling, tools and image previews.

```sh
cargo test -p jcode-desktop-ui transcript_repaint_profile -- --ignored --nocapture --test-threads=1
cargo test -p jcode-desktop-ui -- --test-threads=1
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/profile-render.py target/compact-row-profile --scenario focus-switch --transcript long-history --uncached-panels
python3 -m unittest discover -s scripts -p 'test_profile_*.py'
python3 scripts/screenshot.py target/ui-review.png
```

Use fresh output paths. Keep binaries pinned when comparing revisions, finish
builds before timing, and repeat in both orders. Raw artifacts remain local.

## Delivery verification

The pinned optimized application rendered `target/fps-sep12-ui-review.png` on a
private display. Visual inspection confirmed formatted prose, lists, quotes,
tables, Rust highlighting, math, error cards and composers remained intact.
The required `target/ui-review.png` invocation safely refused to overwrite an
existing artifact, so the task-specific path was used instead.

A subsequent shared-checkout suite had 706 passes and one failure in another
agent's new responsive-caret test. That agent's later changes made the targeted
recheck pass. Further build attempts briefly encountered declared preview
modules that another agent had not yet written. These concurrent files were
not edited or included in this optimization commit. The 705-pass full-suite
result above refers to the pinned optimization-only source, not a claim that
all evolving shared work passed its latest full suite.

After the concurrent source writes settled, the production build succeeded.
The live host's Ctrl+R rebuild-and-reload path was invoked through `--reload-ui`
and successfully activated **UI generation 7** from
`target/release/libjcode_desktop_ui.so` at approximately 05:00 UTC. The activation
log is retained at `target/fps-sep12-live-reload.log`.

Local optimization commit: `c1ca36c`. Pushing local `main` was rejected because
an earlier unrelated commit changes a workflow and the HTTPS token lacks the
`workflow` scope. The existing SSH key was also rejected. Without rewriting
local history or touching credentials, the identical optimization-only patch
was committed on the remote base and pushed to **`perf/compact-transcript-rows`**
(initial remote commit `b5e9941`). Only the panel FPS hunks, profiling option and
this report are on that branch. No unrelated workflow or concurrent UI edits
were included.
