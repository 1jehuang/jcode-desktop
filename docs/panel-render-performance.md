# Panel render cache investigation

## Change

Visible panels now use GPUI's definite-size cached-view boundary. Previously,
workspace chrome updates rebuilt every visible transcript, including markdown
parsing and bulk list measurement invalidation. The cache still invalidates on
panel or descendant notifications, bounds/content-mask/text-style changes, and
window refresh. Native animation requests notify their originating view.

The opt-out `JCODE_DESKTOP_SCREENSHOT_UNCACHED_PANELS=1` is honored only in offline
screenshot fixtures or unit tests. It is read once, not on every panel/frame.
This permits a causal comparison using an identical binary despite concurrent
source edits.

## Reproduce without touching the active desktop

```sh
cargo build -p jcode-desktop
python3 scripts/profile-render.py target/render-cached --scenario focus-switch
python3 scripts/profile-render.py target/render-uncached --scenario focus-switch --uncached-panels
python3 scripts/profile-render.py target/animation-cached --scenario overview
python3 scripts/profile-render.py target/animation-uncached --scenario overview --uncached-panels
python3 -m unittest discover -s scripts -p 'test_profile_*.py'
cargo test -p jcode-desktop-ui panel_cache -- --test-threads=1
```

Each capture launches an isolated Xvfb/Openbox display with Mesa lavapipe and
allowlisted environment, no daemon, credentials, real account settings, or
input to the user's desktop. Four rich offline transcript panels receive native
keyboard actions. The harness records binary SHA-256, private navigation state,
actual input-bearing frames, passive GPUI timing samples, CPU counters, and a
screenshot. Overview captures require multiple animation presents per action.

`draw_rate` means draws divided by sampled wall time, not animation FPS.
`draw_window_p95_median_ms` is the median of sampling-window p95s, not a pooled
p95. CPU is percentage of one core and can exceed 100 with software rendering.

## Evidence, 2026-09-07

A passive live capture (`target/fps-live-initial`) observed real lag: median
sampling-window draw p95 42.7 ms, maximum draw 68.2 ms, maximum input-to-frame
76.1 ms, and approximately 90% process CPU. Its hot-reloaded window had duplicate
sampling tasks, so summed draw counts/intervals cannot be used as live FPS.
Repeated session attachment timeouts also remained in live logs. This change
does not claim to fix those transport failures.

Same-binary private captures:

| Workload / trial | Uncached median window draw p95 | Cached | Interpretation |
| --- | ---: | ---: | --- |
| Focus switching, first pair | 18.07 ms | 8.19 ms | 55% lower typical draw cost |
| Focus switching, reverse-order repeat | 15.04 ms | 4.70 ms | 69% lower typical draw cost |
| Overview transitions | 3.87 ms | 3.79 ms | Essentially unchanged, moving bounds invalidate cache |

The reverse-order repeat (`target/fps-verified-focus-{cached,uncached}`) used
SHA-256 `60df5efd0f8dbc3a89bc756b1aaa0c08465c43e081af87c6c6f9105e94ebf041`,
27 native focus actions per run, and verified both focused panel states. Maximum
draw/input latencies were 26.87/63.34 ms uncached and 19.43/41.75 ms cached.
Earlier cached tails were worse under concurrent builds, and CPU did not improve
consistently. These are software-renderer measurements, not proof of the user's
GPU presentation rate or elimination of every lag source.

Overview captures (`target/fps-same-binary-overview-*`) verified alternating
native overview state and sustained animation samples, approximately 29 draws/s
with either setting on this software renderer. Idle fixtures produced zero draws
with approximately 2% process CPU while profiling was enabled.

## Correctness checks

Three real-panel tests verify that 20 unrelated workspace notifications produce
zero panel rerenders, direct panel notifications invalidate the cache, streamed
text and window resizing remain live, and descendant composer input repaints and
preserves entered text. Existing hover-scroll, startup, pending-input, navigation,
and image tests were exercised in the full serial UI suite.

The pinned GPUI version does not retain test-only `debug_bounds` selectors when
reusing a cached paint subtree. The onboarding geometry test explicitly refreshes
once at its measurement point to collect selectors, without changing any geometry
assertion or application refresh behavior. `scripts/screenshot.py` rendered the
actual final four-panel scene for independent visual inspection.

The identity-footer baseline-y assertion also failed with caching disabled
(1038px versus 1041px), so it is not evidence of a cache regression.
