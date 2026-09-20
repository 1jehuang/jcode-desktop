# Native donut activity indicator

Desktop's transcript and sidebar activity indicators share the native `Spinner`
entity in `crates/jcode-desktop-ui/src/panel_activity.rs`. The spinner is a tiny
animated version of the Jcode halftone torus, not a video, raster animation, or
per-frame SVG decode.

## Rendering and power budget

- Keep the existing 14 logical-pixel footprint so status labels and sidebar rows
  do not move as the animation advances.
- Use an 18-cell halftone screen at this 14px activity-indicator size, rather
  than painting the full 76-cell, 3,344-circle packaging logo. This is an
  animation-specific level of detail, not a change to the canonical logo.
- Share 32 cached animation meshes between spinner instances. Store only XY
  coordinates, not redundant per-vertex GPUI masks and shader coordinates.
  Tests bound the mesh cache to 3 MiB and dot data to 128 KiB per UI generation.
  Initialize individual frames lazily rather than building the entire loop on
  the first visible paint.
- Paint fractional circles together in one native path on one canvas, not one
  layout element per circle. Native paths avoid device-pixel snapping of tiny dots.
- Advance at eight frames per second rather than requesting display-rate redraws.
- Arm at most one timer, and only from a visible paint. Hidden or clipped spinners
  stop rearming. A timer already in flight can complete once after hiding.
- Reduced motion displays the canonical resting pose without rearming animation.
- Only the spinner entity notifies, preserving the transcript's render cache.

The canonical logo and its full-resolution packaging assets remain unchanged.
The geometry follows the website logo's torus proportions, lighting, and resting
pose. Small-size sampling is intentional, not a replacement brand asset.

## Validation

```sh
cargo test -p jcode-desktop-ui --lib panel::activity -- --nocapture
cargo test -p jcode-desktop-ui --lib sidebar_spinner
cargo test -p jcode-desktop-ui --lib panel::activity -- --ignored --nocapture --test-threads=1
python3 scripts/screenshot.py target/donut-spinner.png --transcript streaming
```

Inspect the actual screenshot at normal scale as well as enlarged. Verify an
open central hole, recognizable shaded torus, no layout shift, and a visible
activity marker. Geometry tests are not a substitute for native rendering.

## Verification record (2026-09-20)

- Focused activity and sidebar lifecycle checks: 11 passed, with three manual
  benchmarks excluded from the normal run. All three benchmarks also passed.
- Shared compact mesh positions: 2,475,720 bytes for all 32 frames, versus
  9,902,880 bytes when retaining full GPUI vertices. Dot geometry uses 79,164
  bytes. These are shared per loaded UI generation, not per spinner.
- Debug-build warm native paint preparation measured about 258 microseconds per
  frame in one run. Concurrent builds substantially affected repeated timings.
  This measures CPU path preparation, not GPU presentation or a release-build
  frame-time guarantee. First visits also generate and tessellate their frame.
- Actual Xvfb/lavapipe captures inspected in light/dark themes and idle state.
  Eight successive spinner crops were distinct while animated, and identical
  with reduced motion. The final compact renderer was inspected separately.
  Under heavy concurrent builds, capture needed additional presentation settling
  time to avoid a black first-frame screenshot.
- A broad shared-checkout run passed 1,215 tests and failed 17 outside the
  activity module. Submission and workspace-swipe failures also reproduced with
  the pre-spinner test binary. The remaining broad-suite failures were not
  attributed or fixed by this scoped change. The broad run excluded the mesh
  budget test during optimization, which passed in the final focused run.
