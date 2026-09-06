# Media preview overlays

Click a transcript image or a rendered Mermaid diagram to open the panel's media
viewer. The viewer keeps the transcript mounted behind it, preserving its scroll
position and the composer draft. Escape, Close, or clicking the preview dismisses
it and returns keyboard focus to the composer.

The toolbar provides zoom out, Fit, and zoom in. Zoom ranges from the fitted view
(100%) to 400%. Scroll horizontally or vertically to inspect zoomed content. Zoom stays centered
on the viewport, including at the 400% limit.
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
  checks real image enlargement and dismissal with native input.
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
