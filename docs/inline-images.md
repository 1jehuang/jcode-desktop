# Inline transcript images

Hover over an image in the transcript and pinch to zoom without clicking or opening the viewer. Ctrl+scroll is the mouse-wheel alternative. Zoom is anchored to the pointer and bounded between 100% and 400%.

Once zoomed, two-finger scrolling or dragging pans inside the fixed-height image viewport. Gestures do not scroll the surrounding transcript, even at an image edge. At 100%, ordinary scrolling scrolls the transcript as before. **Fit** restores the original framing. **Open** or a normal click still opens the full-window viewer, but completing a drag does not.

Interaction state belongs to the currently mounted image row. Ordinary rerenders retain it, while replacing an image or unmounting its virtualized row resets it to Fit.

## Verification

- `cargo test -p jcode-desktop-ui inline_image --lib` exercises hover-pinch, pointer anchoring, limits, fit, scroll routing, drag capture, click suppression, and image identity.
- `python3 -m unittest discover -s scripts -p test_image_preview_acceptance.py` checks the native acceptance harness.
- `python3 scripts/screenshot.py target/inline-image-review.png --transcript image --image-interact` builds the real app and runs native inline zoom, drag, fit and full-window viewer regression checks on a private Xvfb display. Use a new output filename for each run.
