# Sidebar frame work, 2026-09-12

## Change

Every sidebar render now sorts the session catalog once rather than twice.
Swarm grouping builds a parent-to-children index and walks rooted trees once,
instead of walking every node's ancestry and repeatedly scanning entire groups.
Traversal is iterative, preserves catalog sibling order and depth-first nesting,
and avoids recursion overflow on deep swarms. Missing parents remain visible
roots. Cycles and their descendants remain independent visible rows. A visited
guard bounds traversal even for malformed duplicate session IDs.

This removes work rather than changing build optimization, animation duration,
input batching, user preferences, or the visual design. There is no persistent
cache or new cache-invalidation requirement. The improvement is specifically
sidebar preparation, not a claimed multiplier for whole-window FPS.

## Live investigation

The running process was already a release build, with hardware Intel Arc B390
Vulkan through Mesa 26.2.2. A passive 25-second capture observed maximum draw
29.39 ms and input-to-frame 40.17 ms. The process held approximately 18 GB of
anonymous resident memory. No memory pressure was reported during that capture.

The capture contained 931 records for one window, substantially exceeding the
roughly 250 possible from one 100 ms sampler. Consequently the summed frame
counts and aggregate FPS are not valid evidence. Duplicate live samplers were
already documented in `reload-lifecycle-2026-09-07.md`. This change does not
claim to fix their ownership or the live process's memory retention.

Two isolated tests using the existing release host and actual cdylibs did not
reproduce duplicate samplers: three generations with sidebar history, then four
generations with rich transcripts, four panels and native focus switching.
Both reused identical plugin bytes. Distinct rebuilt generations and the real
workspace remain important lifecycle differences. No input was injected into
the user's desktop and no compositor control commands were used.

Local evidence: `target/live-profile/fps-current-2238/`,
`target/fps-release-reload-before/`, `target/fps-release-reload-motion/`.

## Verification

The final grouping tests pass, including exhaustive equivalence to the old
algorithm for every four-node parent graph in both catalog orders, sibling
ordering, orphans, cycles, malformed duplicate IDs, and a 10,000-node chain.
Existing native-view tests retain child click, expansion and collapse behavior.

The manual benchmark runs both algorithms in the same debug test executable,
with three warmups and 20 samples, alternating execution order. Representative
p50 results (not whole-window FPS):

| Catalog | Previous grouping | Indexed grouping |
| --- | ---: | ---: |
| 100 independent sessions | 0.139 ms | 0.055 ms |
| 500 flat swarm sessions | 3.567 ms | 0.700 ms |
| 1,000 flat swarm sessions | 12.095 ms | 1.456 ms |
| 1,000 deeply nested sessions | 310.348 ms | 1.954 ms |

The separate real GPUI hover test uses independent sessions, not a large swarm.
Its 100/500-session p95 was 3.171/4.170 ms before and 3.358/4.083 ms after.
These mixed results do **not** establish a whole-frame speedup. They are retained
rather than inferring one from the component improvement.

Logs: `target/fps-sidebar-grouping-after.log`,
`target/fps-sidebar-before.log`, `target/fps-sidebar-hover-after.log`.

```sh
cargo test -p jcode-desktop-ui sidebar_swarm -- --test-threads=1
cargo test -p jcode-desktop-ui sidebar_swarm_grouping_profile -- --ignored --nocapture --test-threads=1
cargo test -p jcode-desktop-ui sidebar_hover_frame_profile -- --ignored --nocapture --test-threads=1
```

## Delivery checks

- The final targeted swarm suite passed: 8 tests, no failures, one ignored manual
  profile. The complete current UI suite passed 845 tests with 19 failures and
  nine ignored tests. The pinned pre-change binary passed 841 with the **same
  19 failures**, and eight ignored tests. No new failing tests were introduced.
  Logs: `target/fps-full-{baseline,after}.log`.
- `cargo build --release -p jcode-desktop -p jcode-desktop-ui` succeeded.
- The real application rendered the four-panel/swarm fixture on private Xvfb.
  Visual inspection confirmed nested agent labels, working counts, expansion
  affordance, active-session selection, formatted transcripts and composers.
  Image: `target/fps-sidebar-ui-review.png`.
- The prescribed `target/ui-review.png` path refused to overwrite an existing
  image, which was preserved. A single-panel attempt failed the harness width
  readiness assertion. The four-panel run succeeded with all widths at 0.25.
- The application's Ctrl+R rebuild-and-reload path, requested through
  `--reload-ui`, successfully activated generation 5 in the existing window at
  22:45 UTC. The new generation again confirmed hardware Intel Vulkan.
  Log: `target/fps-sidebar-live-reload.log`.

The post-reload 20-second passive capture had no input-bearing frames or
animation presentation samples. Its maximum draw was 14.70 ms, but it cannot
be compared with the input-bearing baseline as an FPS improvement. It still
contained 950 records for one window, confirming duplicate samplers remain.
Artifact: `target/live-profile/fps-sidebar-after-2245/`.
