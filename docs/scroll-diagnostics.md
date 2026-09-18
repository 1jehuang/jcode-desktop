# Tracking jumpy transcript scrolling

Transcript panels automatically keep the last 24 painted scroll samples in
memory. There is nothing to enable. Suspicious movement emits a single JSON
`scroll_anomaly` record, including the preceding samples, into the existing
local desktop log:

```sh
tail -F "${XDG_STATE_HOME:-$HOME/.local/state}/jcode-desktop/jcode-desktop.log" \
  | grep --line-buffered '"event":"scroll_anomaly"'
```

Each record identifies a numeric panel entity and a reason:

- `painted_movement_mismatch`: movement of a measured row differs from the
  applied scroll step by more than 24 pixels during a recent scroll gesture.
- `logical_scroll_discontinuity`: the measured anchor is no longer available,
  and the top row changes without applied movement or against its direction.
- `scroll_frame_gap`: an 80–500 ms interval between paints during active
  scrolling with a nontrivial applied step.

Samples include wall-clock Unix milliseconds, time since the preceding paint,
coalesced input count and pixel delta, precise-touchpad versus wheel input,
applied and observed movement, logical row and offset, estimated scrollbar
position and extent, row count, viewport geometry, tail-follow and drag state.
Positive input/applied/observed values mean moving down through the document.
The estimated scrollbar Y coordinate uses GPUI's opposite sign convention.
Observed movement is `null` if no comparable measured anchor exists.

The detector uses measured row movement, not changes in estimated total list
height, to avoid treating ordinary lazy row measurement as visible jumping.
Scrollbar drags and inactive changes are excluded. Histories reset after gaps
over 500 ms. Reports are limited to one per panel per 10 seconds. Recording
uses existing paints and does not schedule frames or poll files.

These records are diagnostic leads, not proof of a rendering bug. Content
updates, viewport changes, and deliberate navigation soon after scrolling can
also cause a mismatch. The samples help distinguish those cases. This covers
transcript scrolling, not workspace panning, sidebars, terminal scrolling, or
GPU/compositor flicker that leaves layout unchanged. Nothing is uploaded.
Scroll records contain no transcript text, drafts, file paths, or session names.
Other entries in the shared desktop log can contain unrelated diagnostic data.

If a jump is noticed, retain its approximate time. Search for `scroll_anomaly`
and the existing `panel_geometry_oscillation` records around that time. The
host rotates its log at startup once it reaches 5 MiB, retaining
`jcode-desktop.log.previous`, so inspect that file too after a restart.

Regression checks:

```sh
cargo test -p jcode-desktop-ui panel::flicker
cargo test -p jcode-desktop-ui panel::scroll_momentum_tests
cargo test -p jcode-desktop-ui panel::stream_scroll_tests
```
