# Interaction latency profile

Measured on 2026-08-22 on a Dell XPS 13 9350 (Intel Core Ultra 7 256V), Linux x86_64, using an optimized build and GPUI's headless interaction harness.

## Method

The opt-in `interaction_latency_profile` test drives the real key bindings through GPUI into a workspace containing 32 panels across four strips. It records 500 input-to-state samples after 50 warmups for each operation. Three independent runs used isolated state directories.

Run it with:

```sh
cargo test --release -p jcode-desktop-ui interaction_latency_profile -- --ignored --nocapture
```

These measurements include key parsing, action dispatch, workspace mutation, focus updates, learning-model updates, and learning-state persistence. They exclude the physical keyboard, compositor, GPU presentation, and the subsequent animation frames. Policy-driven visual settling is listed separately.

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
