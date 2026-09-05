# Sidebar session-click investigation (2026-09-05)

## Live observations

The user reported lag when clicking hovered sidebar sessions. A passive 45-second
capture of the existing Wayland desktop (PID 178621) is retained locally at
`target/live-profile/sidebar-clicks-0950`. It recorded input-bearing frames.

- Maximum recorded input-to-frame latency: 43.25 ms.
- Maximum recorded draw duration: 27.23 ms.
- Maximum UI sampler wake lateness: 224.57 ms.
- Desktop resident memory at investigation start: about 3.6 GiB. An smaps
  breakdown attributed about 3.0 GiB to the heap, about 342 MiB to other anonymous
  mappings, and roughly 32 MiB to each retained UI library.
- Concurrent compiler/linker jobs and CPU, memory and I/O pressure were present.
  Our older tab verification builds were cancelled to reduce contention.

The capture contains callbacks from multiple retained UI generations. Perf
samples explicitly include histogram sampling in generations 4 through 9, and
there are more 100 ms sampling records than one sampler should produce in 45
seconds. Consequently aggregate draw/input counts are not independent observations
and must not be used as a throughput or pooled latency statistic. Perf also adds
overhead. The maxima above are observations, not a controlled click benchmark or
proof of the cause of the user's perceived lag.

## Reproduced defect and fix

`Panel::new` stored a scroll handler in its owned `ListState`. The handler captured
a strong `Entity<Panel>`, forming a cycle:

`Panel -> ListState -> scroll handler -> Entity<Panel>`

The regression `transcript_scroll_handler_does_not_keep_closed_panel_alive`
failed before the fix: after dropping the last external entity handle and draining
GPUI work, the weak handle could still be upgraded. It passed after changing the
callback capture to a weak entity. This proves panel release improved, without
relying on timing thresholds or synthetic FPS claims.

`replacing_window_root_releases_rendered_workspace` separately checks that a
rendered workspace is released after replacing its root in the same native
window. This distinguishes panel retention from workspace teardown.

## Limits and next acceptance step

This fix prevents the cycle in newly created panels. Hot reload cannot
retroactively rewrite callbacks inside already leaked panels from older library
generations. It does not prove the source of all heap growth or resolve the
multiple-sampler observation. Do not describe the full click-lag problem as fixed.

A conclusive responsiveness comparison needs matching clicks with no competing
builds, one active timing sampler, and a clean instance if reclaiming already
leaked objects. Restarting the user's active desktop requires approval because
unsaved drafts and layout may otherwise be lost.

## Deployment and post-fix observation

Both lifecycle regressions passed, as did 12 transcript-related tests (one manual
profiler ignored). The attempted hot reload could not finish: PID 178621 exited
while the rebuild was running. No application error or cause was established by
the available logs. No termination command was sent by this investigation.
The already-exited desktop was restored as PID 714979 with hot reload enabled;
UI generation 1 activation was confirmed from the runtime log.

A subsequent 20-second no-perf live capture at
`target/live-profile/sidebar-clicks-after-0956` contained 189 sampling windows,
22 draws, and only two input-bearing frames. Maximum wake lateness was 3.98 ms,
draw duration 31.36 ms, and input latency 48.01 ms. Resident memory in the new
instance was about 224 MiB. The sampling rate is consistent with one sampler.

This is **not** a matching before/after workload: the process and open sessions
changed, contention decreased, perf was disabled, and only two inputs occurred.
It therefore does not establish improved session-click latency. The confirmed
improvement is the lifecycle regression changing from leaked to released panels.
The user's original click-lag acceptance remains open pending repeated matching
session clicks in the restored app.
