# Development-session performance diagnosis, 2026-09-18

## Conclusion

The observed desktop was a **release host with release UI hot reload**, not an
unoptimized debug build. Hot reload does not require the debug profile. The dev
profile already optimizes GPUI, layout, shaping, event-loop dependencies, and the
terminal engine at level 2 while keeping most application code at level 0.

There is evidence of retained live UI work and growing resident memory, plus
concurrent CPU contention. There is not yet a verified cause or a validated fix
for the user's perceived lag. No runtime/profile/configuration changes or live
restart were made during this investigation.

## Passive live observations

Artifacts: `target/performance-sep18-before/`. The capture did not inject input,
change focus, request redraws, or manipulate the compositor.

- Over 24.88 seconds, RSS rose from 7,726.5 MiB to 8,165.9 MiB. A UI reload from
  another ongoing task occurred during the capture, so this is not a controlled
  per-frame leak measurement.
- 510 samples all identified the same window. Their nominal sampling intervals
  sum to 53.52 seconds, more than twice the capture duration. Multiple samplers
  were active. Summed draw counts must **not** be interpreted as FPS.
- Maximum observed draw duration was 25.99 ms and maximum timer wake lag was
  26.13 ms. There were **zero input-bearing frames**, so the recording cannot
  establish typing, scrolling, or switching latency.
- Thread inspection after reload found three `jcode-bridge`, three
  `jcode-accounts`, three learning workers, and 18 session workers, consistent
  with retained workspace generations. Logs also reported failure to rebind the
  per-process preview-control socket after reload.
- Later process inspection showed approximately 10.5 GiB RSS. Most resident
  memory was anonymous heap, not the mapped UI library files. Retaining library
  code for callback safety does not by itself explain this footprint.
- CPU pressure was elevated while unrelated builds/tests were running. At the
  first process sample, Linux CPU PSI `some avg10` was 21.10%. This is contention
  evidence, not proof that all observed stalls were scheduler delays.
- `perf` was unavailable (`command not found`), so no CPU stack attribution was
  obtained. Session catalog calls ran on background threads. Their logged wall
  times are not UI-thread blocking measurements.

## Private reproduction attempts

All probes used isolated Xvfb displays and offline fixture data. None used the
user's compositor or real input/session connections. Scratch scripts adapted
`scripts/reload_lifecycle_probe.py` to select the release artifacts. Reloads used
prebuilt plugins and a no-op Cargo command, testing lifecycle rather than actual
compilation. Original artifacts remain under ignored `target/`.

| Probe | Result |
| --- | --- |
| Release, empty transcript and 80 history rows, 3 generations | 8 samples/s each generation, final RSS 285 MiB |
| Release, rich transcript and history, 3 generations | 7.6–8 samples/s, final RSS 297 MiB |
| Copy of the actual live host executable with current rich plugin, 3 generations | 7.8–8 samples/s, final RSS 286 MiB |
| Current release, four rich panels and history, continuous overview/focus/resize/typing, 4 generations | Native actions and input-bearing frames verified, no duplicate-sampler threshold violation, RSS 493→1,018 MiB |
| Copied live host, same active workload, 4 generations | Native actions and input-bearing frames verified, no duplicate-sampler threshold violation, RSS 508→1,002 MiB |

Active probes held thread counts steady at 91/92. Their RSS growth does not
separate allocator high-water marks, font/render caches, per-generation retained
state, and per-frame leaks. They did **not** reproduce the live duplicate-worker
condition. Removing software-renderer environment overrides still selected
llvmpipe, so it did not provide an Intel GPU comparison.

Artifacts:

- `target/performance-sep18-lifecycle-before/`
- `target/performance-sep18-rich-lifecycle-before/`
- `target/performance-sep18-old-host-before/`
- `target/performance-active-release-20260919/`
- `target/performance-active-oldhost-20260919/`

Ownership inspection ruled out a proposed direct
`Workspace → ListState → render callback → Workspace` cycle: GPUI's `ListState`
does not own the rendering callback. Workspace async tasks and navigation/recovery
callbacks use weak entities. GPUI's element-arena TLS is private per shared
library, which deserves investigation, but neither that fact nor the private
probe results establish it as the cause. Do not introduce an unsafe arena reset
or unload old code based on this hypothesis.

## Practical development guidance

1. Keep hot reload. Release hot reload is already active here. For genuine debug
   builds, retain the existing selectively optimized dependencies. Raising the
   UI crate to `opt-level = 1` is a possible future tradeoff, but must be measured
   against incremental build time and debugger usability. It cannot fix this
   release process's retained state.
2. Avoid overlapping heavy builds and acceptance runs while assessing latency.
   For manually launched builds, `nice -n 10 cargo build -j 8` is a reasonable
   experiment on this machine. It is not a measured optimum. Lower scheduling
   priority is inherited by compiler/linker children, but `-j` does not cap every
   compiler's internal thread count. This does not retroactively reprioritize
   Ctrl+R builds launched by an existing desktop host.
3. A deliberate full application restart may reclaim accumulated generations
   and heap. Treat this as a workaround, not a fix. Preserve drafts and consider
   active sessions/PTYs before restarting. No restart was performed here.
4. The next acceptance loop should capture the specific laggy interaction in the
   real window with input-bearing samples, after competing jobs finish. Compare
   a fresh process with a long-lived, repeatedly reloaded process using the same
   workload. Fix the implicated lifecycle/render path and repeat the capture
   before claiming a responsiveness improvement.

This is a diagnostic report only. No app rebuild/reload is required for this
Markdown-only addition, and no performance improvement is claimed.
