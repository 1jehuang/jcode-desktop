# Live desktop lag feedback loop

## What this measures

`scripts/profile-live.py` enables a short capture in an **already running, real desktop window**. It does not use Xvfb, synthetic history, simulated input, compositor commands, or the performance overlay. It does not request animation frames or redraws. The UI generation must include `live_profile.rs` (rebuild/reload once after installing).

Every approximately 100 ms during capture, the UI sampler subtracts GPUI's previous cumulative histograms from the new ones. Records contain new draw durations, input-to-frame latency, animation presentation intervals, coalesced inputs, and mid-draw inputs. No input events means `null` latency, not a fabricated zero. A foreground timer also measures wake lateness. Background workers perform control-file reads and JSONL writes, never the UI thread. Disabled sampling checks for a request at 1 Hz without accessing the window.

The CLI samples the process and Linux CPU/I/O/memory pressure at 4 Hz using the same wall clock. `--perf` additionally samples userspace CPU at 99 Hz, excluding inherited child processes. `capture.json` stores a monotonic/wall-clock anchor to align perf timestamps with UI records.

No prompt text, keystroke content, transcripts, or screenshots are collected. Control files and raw runtime logs have mode 0600. Requests expire automatically within two minutes, and the CLI removes its own request on normal exit. Multiple windows are identified in each sample.

## Run the loop

1. **Finish builds first.** Do not compare an idle baseline with a run competing against a linker, VM, or indexing job. Keep the same app, sessions, viewport, display, and workload across comparisons. Do not restart between samples unless testing restart effects deliberately.
2. Take an idle baseline:
   ```sh
   python3 scripts/profile-live.py --seconds 20 --scenario idle --output target/live-profile/idle-1
   ```
3. Pick one action that actually feels slow. Capture it for 60 seconds:
   ```sh
   python3 scripts/profile-live.py --seconds 60 --scenario scrolling --output target/live-profile/scroll-1
   ```
   Repeat the same movement on the same conversation. For typing, type in the composer without submitting. For panel switching, repeat the same pair of panels. Use `--pid` if multiple desktop processes are running.
4. Inspect `summary.json` and the worst input windows printed by the command. **A run with no input-bearing frames cannot validate input lag.** The output explicitly says so. Do not report the maximum of bucket p95s as a pooled p95 or assume an animation interval measures continuously presented FPS.
5. If lag occurs, repeat the same scenario with `--perf` to attribute CPU work:
   ```sh
   python3 scripts/profile-live.py --seconds 60 --scenario scrolling --perf --output target/live-profile/scroll-stacks
   perf report --stdio -i target/live-profile/scroll-stacks/perf.data --no-children
   ```
   `--perf` has extra overhead, so preserve a no-perf run for comparison. Frame-pointer stacks can be incomplete in libraries without frame pointers; self samples still locate executing symbols. Perf permission or collection failures are reported separately from window metrics.
6. Change **one** implicated component, rebuild/reload, let startup/build work settle, and repeat the same scenario at least three times. Accept a performance fix only when the affected input-delay / slow-draw / presentation metric improves on actual input-bearing runs without increasing idle redraws or losing inputs.

## Attribute the stall

| Observation in the same time window | Next check |
| --- | --- |
| Long draw time and high main-thread CPU | Inspect main-thread perf symbols, layout/text work, transcript rendering. |
| High wake delay plus CPU pressure | Compare with the same workload after competing jobs finish. This is scheduler contention evidence, not automatically an app bug. |
| High wake delay plus major faults or I/O/memory pressure | Inspect paging/file reads and synchronous work. Do not equate background session-list wall time with UI blocking. |
| Input latency much greater than draw time, with low wake delay | Investigate event dispatch, invalidation scheduling, and frame delivery rather than optimizing transcript rendering blindly. |
| Long animation presentation intervals but cheap draw | Inspect frame pacing and presentation scheduling on this live backend. Xvfb cannot answer this. |
| Coalesced/mid-draw inputs increase | Check event batching/dropped timing coverage before trusting an apparent latency improvement. |
| No input or animation samples | Repeat the actual laggy action. Idle CPU profiles do not establish interactive smoothness. |

Main-thread CPU and fault counters in `process.jsonl` are cumulative. Compute deltas between adjacent records. The process I/O counters include background threads. Match samples by `unix_ms` and allow for the 100/250 ms sampling windows, rather than implying exact event-level causality.

For perf timestamps, approximately:

`wall_time_ns = perf_monotonic_seconds * 1e9 + capture.unix_ns - capture.monotonic_ns`.

Clock adjustments during a capture can reduce correlation accuracy. Capture IDs reset histogram baselines so old runs cannot contaminate new results.

## Verification

- Rust tests check bounded/expired requests, safe capture IDs, histogram subtraction, no-input null values, and mid-draw deltas.
- Python tests check process counters, explicit inconclusive idle results, and correlated slow-input reporting.
- Public acceptance is a live run producing records for the current PID/window. No fixture result substitutes for this check.
- The ultimate lag-fix acceptance is a repeated matching real interaction with improved timings. Installing the capture loop alone does not establish that the app is faster.
