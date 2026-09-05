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

## Public-binary follow-up

Built the actual debug executable and drove keyboard navigation through the private Xvfb/Openbox screenshot workflow. The initial standard screenshot command correctly refused to overwrite an existing `target/ui-review.png`; subsequent captures used unique names. Inspected `target/perf/xvfb-actions.png`, `hardware-actions.png`, and `settled-actions.png`: the fixture transcript, math, code block, accounts, and composer painted after navigation. The performance overlay and shortcut showcase overlap some top chrome, so these images are evidence of rendering, not a claim of flawless visual layout.

An initial software-rendered run showed 30.8 ms draw p95 and 104.5 ms input-to-frame p95. A follow-up requesting the Intel Vulkan ICD showed 10.0 ms draw and 32.1 ms input-to-frame, but a matched software run also showed 10.0 ms draw. This does **not** establish a GPU-driver cause. The private X11 rendering/presentation path differs from the live Wayland session.

Public testing uncovered two real instrumentation defects:

1. Focus action recording was gated by whether navigation earned learning credit. Empty-strip transitions animated but generated no records.
2. Recording those transitions then exposed a completion bug: empty strips never clear panel-camera layout dirtiness, so their captures did not finish. The first patched run emitted only five `focus_up` records instead of ten up/down records.

Fixed both: every actual strip change begins capture, boundary no-ops do not, and empty-strip capture completion depends on the row animation rather than nonexistent panel layout. This changes opt-in telemetry, not animation speed.

The final public workflow sent five `super+j` / `super+k` pairs with 650 ms between keys. It asserted exactly ten JSONL records with both action names and returned to `strip=0 focus=0 widths=1.00`. Observed ranges of **per-action p95s**, not one pooled percentile:

| Metric | Minimum | Maximum |
| --- | --- | --- |
| Draw | 5.44 ms | 20.99 ms |
| Presentation interval | 27.66 ms | 38.08 ms |
| Input-to-frame | 16.97 ms | 33.91 ms |
| Construction interval | 31.51 ms | 32.57 ms |

This reproduces approximately 31 Hz construction cadence in the private fixture, despite relatively small view-construction cost. It narrows the investigation toward frame scheduling/presentation rather than transcript-length scaling, but cannot establish the cause of lag on the user's active Wayland window. No smoothness improvement is claimed.

### Requirement-to-evidence mapping

- Profile the running app: live perf captures and process/system counters above.
- Distinguish transcript CPU work from actual frame delivery: headless 32-panel and 100/1,000/10,000-message profilers plus real public-binary presentation/input histograms.
- Capture real empty-strip navigation: new `empty_strip_navigation_is_captured_but_boundary_noops_are_not` regression passed, and public JSONL workflow produced ten of ten expected records after the completion fix.
- Preserve boundary no-op semantics: regression checks the top-edge `super+k` does not start a capture.
- Preserve telemetry calculations: all six `performance::tests` passed after the changes.
- Check UI integration: debug app built successfully and real screenshots were inspected. No network, email, payment, or daemon actions were sent by the inert fixture.
- Investigate existing suite failures: rerunning the test binary serially with isolated HOME/XDG configuration still produced the same five failures (282 passed, 5 failed, 5 ignored in that concurrently updated checkout). User configuration alone does not explain them. They remain outside this telemetry patch.
- Deployment boundary: rebuild-and-reload used the instance socket's Ctrl+R-equivalent action. An interrupted rebuild reported SIGTERM, and an explicit retry successfully activated generation 11. Final activation evidence is recorded below.

Release packaging and changes to the live compositor were not exercised: neither is changed by this telemetry patch, and private X11 timing must not be presented as a live Wayland acceptance result. Earlier statements that this work added only a test/report are superseded by this follow-up's opt-in telemetry fix.

Final hot reload: successfully activated UI generation 12 from `target/debug/libjcode_desktop_ui.so` after the final public workflow passed.

## Final whole-result verification

Reran checks on committed telemetry fix `fea4971`:

- Empty-strip/boundary regression: 1/1 passed.
- Performance histogram/capture tests: 6/6 passed.
- Loaded 32-panel first frame: passed, p95 3.636 ms in this run.
- Transcript repaint profiler: passed, p95 3.987 / 4.262 / 5.253 ms for 100 / 1,000 / 10,000 messages.
- Host integration suite: 13/13 passed, including same-window successful reload, failed-activation rollback, private and stale instance sockets, second-instance forwarding, terminal startup/resizing, and resource survival across generation handoff.
- Screenshot isolation tests: 2/2 passed.
- Full UI suite: 282 passed, 6 failed, 5 ignored. The five previously listed failures persisted. `vertical_keys_cover_both_animation_directions` additionally failed because its transient outgoing-animation bounds had disappeared; the fully qualified single-test rerun passed (1/1). This suggests timing sensitivity but does not prove its cause or make the full suite green.

**Observed improvement is limited to diagnostic correctness:** the same ten real key events in the private public-binary workflow produced zero records before the fix, five after the first partial fix, and all ten after the final fix. This is concrete before/after evidence for the changed JSONL output. It is not evidence that the live app became smoother.

**Live smoothness acceptance remains blocked by an uncaptured reproduction.** The active process was sampled without manipulating its window. The user's specific laggy interaction was not identified or captured with action-local live Wayland presentation telemetry. Private Xvfb fixtures cannot replace that acceptance path. No controlled live before/after frame-delivery comparison exists, and the task must not be summarized as a verified fix to perceived lag. Changing the compositor, forcing user interactions, or restarting the user's active session to obtain that comparison was intentionally avoided. Packaging formats were unchanged; a fresh debug executable and the actual running plugin reload were validated, but no release-package certification is claimed.
