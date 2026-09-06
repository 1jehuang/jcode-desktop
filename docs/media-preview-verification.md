# Media preview overlays

Click a transcript image or a rendered Mermaid diagram to open the panel's media
viewer. The viewer keeps the transcript mounted behind it, preserving its scroll
position and the composer draft. Escape, Close, or clicking the overlay's outer
padding dismisses it and returns keyboard focus to the composer. Clicking or
dragging inside the image viewport keeps the viewer open.

The toolbar provides zoom out, Fit, and zoom in. Zoom ranges from the fitted view
(100%) to 400%. Pinch or Ctrl+scroll zooms around the pointer. Toolbar zoom stays
centered on the viewport, including at the 400% limit. Drag or two-finger scroll
to pan, including diagonally. Double-click toggles between 200% and Fit.
Opening another preview resets zoom and scrolling. Mermaid previews reuse the
rendered SVG rather than re-parsing the source or opening an external browser.
Incomplete diagrams retain their non-interactive fallback until they render.

## Checks

- `cargo test -p jcode-desktop-ui --lib preview -- --test-threads=1`
  exercises image and streamed Mermaid clicks, zoom controls, both scroll axes,
  Escape and Close, draft protection, restored focus, and transcript position.
- `python3 scripts/screenshot.py target/ui-review.png` builds and renders the
  real application on an isolated Xvfb display.
- `python3 scripts/screenshot.py target/media-image-acceptance.png --no-build --transcript image --image-interact`
  checks real image enlargement, safe image clicks, Ctrl+wheel zoom, drag pan,
  double-click Fit, and Escape with native input and pixel assertions.
- `python3 scripts/screenshot.py target/mermaid-preview-acceptance.png --no-build --transcript mermaid --mermaid-interact`
  checks rendered diagram pixels after native open, zoom, Fit, Escape, Close,
  and repeated opening. Each interaction leaves a screenshot artifact. Use a
  fresh output filename for repeated runs.
- `python3 -m unittest discover -s scripts -p 'test_mermaid_preview_acceptance.py'`
  verifies that the visual oracle rejects unchanged, missing, and displaced
  diagrams and locates the actual toolbar controls.

The pre-change binary failed the native diagram-click check with unchanged
119,983 diagram pixels. The updated viewer produced 236,696 fitted pixels and
350,108 visible zoomed pixels in the default 1440×1000 fixture. Fit and both
explicit dismissal paths restored the expected pixel masks. Normal-layout
acceptance also passed. The final centered-zoom run stayed visible through every
50% step to 400% (883,663 visible diagram pixels) and returned to the same fitted
and closed pixel masks.

The live desktop acknowledged the Ctrl+R-equivalent request, coalesced it with
the rebuild already in progress, and activated UI generation 1 from the rebuilt
UI library. The transcript sessions reconnected after activation.

## Gesture acceptance, 2026-09-06

The gesture change is commit `96c13f7`. Before this change, the viewer had toolbar
zoom and scrolling, but no pinch listener or drag panning, and an image click
dismissed it. The following checks verify interaction outcomes rather than only
inspecting the implementation:

| Requirement | Observed result |
| --- | --- |
| Pinch zoom anchored under the pointer | GPUI dispatches Started/Moved/Ended `PinchEvent`s at 30%/40% of the viewport. Zoom changes from 1 to 2 and the expected image origin matches within one device pixel in both axes. |
| Zoom limits and invalid events | Dispatched pinch deltas reach the 4x upper and 1x lower bounds. NaN leaves zoom unchanged. Returning to Fit restores zero scroll offset. |
| Pan naturally without dismissing | The GPUI test verifies a drag adds exactly (30, 20) pixels to the offset, release clears drag state, and diagonal scrolling changes both axes without changing zoom. |
| Inspect without losing chat state | Viewer tests preserve the transcript scroll position and draft, block typing into the hidden composer, and restore focus on Escape. |
| Native rendering actually enlarges | In the real Xvfb app, blue chart pixels increase from 5,940 in the thumbnail to 26,271 fitted, then 84,755 after Ctrl+wheel. |
| Native dragging follows the pointer | A (-35, -25) drag moves the visible blue bar's top-left from (376, 603) to (341, 578), exactly the requested translation. Clipping changes the visible area, so translation uses the edge rather than its centroid. |
| Image clicks stay open | A single image click leaves the fitted mask at 26,271 pixels and top-left (432, 555). |
| Reset and close restore the original views | Double-click Fit restores 26,271 pixels at (432, 555). Escape restores the thumbnail's 5,940 pixels at (652, 341). |

Commands used:

```sh
cargo test -p jcode-desktop-ui image_preview --lib
python3 scripts/screenshot.py target/ui-review-image-gestures-final.png --transcript image --image-interact
```

All three image/Mermaid viewer tests passed. The broader panel regression run
passed 93 tests with two manual profilers ignored. Screenshots for every native
interaction share the `target/ui-review-image-gestures-final` prefix. The fitted
and zoomed screenshots were also visually inspected.

Pinch events were exercised through GPUI's input dispatcher, not a physical
trackpad. Native wheel, drag, click, and keyboard behavior was exercised on the
real app's private X11 display, without interacting with the user's desktop.

Delivery was verified separately: the initial reload request was ignored because
the host was busy, so it was retried after the active build finished. The accepted
Ctrl+R-equivalent request at 07:46:46 UTC activated UI generation 5 in the same host
process. The mapped library and rebuilt output had matching SHA256
`c53ab504a4344a7701665c2e2de0c84a72ca6635e0bd94f3fc6f3f6f39d56ed6`,
with both artifacts newer than the request and gesture source change.
