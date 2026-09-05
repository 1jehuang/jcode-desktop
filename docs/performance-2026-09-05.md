# Desktop lag investigation, 2026-09-05

## Measurements

Running app: debug host with `--hot-reload`, optimized GPUI dependencies, on Linux/Wayland. Other builds and UI reloads were occurring during observation, so these are diagnostic samples, not controlled before/after comparisons.

- Existing headless 32-panel key-to-first-frame profiler: p50 1.774 ms, p95 1.982 ms, p99 2.328 ms.
- New headless transcript repaint profiler (50 measured frames after 10 warmups):

  | Messages | p50 | p95 |
  | --- | --- | --- |
  | 100 | 3.248 ms | 3.826 ms |
  | 1,000 | 3.690 ms | 4.503 ms |
  | 10,000 | 4.417 ms | 4.802 ms |

  A prior run gave p95 4.288 / 4.602 / 5.444 ms respectively. The fixture uses alternating user/assistant messages with markdown. Assertions check that the transcript painted and the expected row count was installed. These timings include headless GPUI frame construction, **not GPU rendering or compositor presentation**, and do not cover expensive diagrams or large tool output.

- Live 60-second CPU-clock capture: 322 samples, approximately 1.62 seconds of sampled userspace CPU across the app and inherited children. Sample shares: jcode child processes 51.24%, desktop main thread 24.84%, bridge session thread 13.04%, Tokio workers 6.21%, session thread 4.66%. This is low utilization, not evidence of a saturated UI thread. Child startup includes configuration parsing. Bridge work includes sidebar JSON metadata scanning.
- Separate 20-second observation: main-thread CPU increased by 22 ticks (about 1.1% of one CPU at 100 Hz), with zero additional main-thread minor or major faults. Process I/O increased by about 17.3 MiB read and 0.54 MiB written.
- System I/O pressure during observation: `some avg60` around 8%, `full avg60` around 6%. Memory pressure was lower (`some avg60` around 0.5%). Builds were competing for CPU and storage. Filesystem was 97% full, with about 17 GiB free. These are possible contributors, not proof of the reported stall's cause.
- App RSS was approximately 781 MiB after seven retained hot-reload libraries. Logs showed session metadata loading taking 2.8–5.3 seconds and session list calls taking about 0.4–0.8 seconds. These operations are on background threads, so their wall time must not be equated with UI blocking.

## Conclusion and limits

No persistent UI CPU bottleneck or long-history repaint explosion was reproduced. The strongest observed environmental concern is concurrent build/storage contention. Intermittent input or presentation stalls remain unmeasured. Do not label this investigation a lag fix or infer 60 Hz presentation from the headless results.

The next useful capture should target the specific laggy action (typing, transcript scrolling, panel switching, or opening a session). The existing `JCODE_DESKTOP_PERF_ACTIONS` recorder can distinguish draw duration and presentation/input latency when enabled at launch. Avoid restarting an active session merely to enable it without coordinating with the user.

## Reproduction

```sh
cargo test -p jcode-desktop-ui loaded_animation_first_frame_profile -- --ignored --nocapture
cargo test -p jcode-desktop-ui transcript_repaint_profile -- --ignored --nocapture
perf record -e cpu-clock -F 199 --call-graph dwarf,4096 -p "$PID" -o target/perf/live.data -- sleep 60
perf report --stdio -i target/perf/live.data --no-children -g none --sort comm
```

Allow time for perf to finalize mappings after sampling ends. An initial capture was killed by an overly short 30-second command timeout and was unusable. The successful capture needed 77 seconds including finalization. Profiling itself introduces overhead, especially DWARF capture and symbolization.

Only an opt-in test and this report were added. No runtime behavior was changed, so no application rebuild/reload is required for these additions. Raw local captures and logs remain under ignored `target/perf/`.

## Validation

Both focused profilers passed. The full UI suite reported 281 passed, 5 failed, and 5 ignored. Failures were `email_inbox_moves_when_the_user_scrolls`, `restored_scroll_is_not_replaced_when_history_reattaches`, `a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`, `right_edge_is_a_full_height_click_target_for_a_new_session`, and `showcase_is_on_by_default_and_only_paints_workspace_motions`. The new test is ignored in that suite and changes no runtime code. These failures were not repaired as part of profiling, and the full suite must not be reported as green.
