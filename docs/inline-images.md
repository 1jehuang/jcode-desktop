# Inline transcript images

Hover over an image in the transcript and pinch to zoom without clicking or opening the viewer. Ctrl+scroll is the mouse-wheel alternative. Zoom is anchored to the pointer and bounded between 100% and 400%.

Keep both fingers on the trackpad and move them together while pinching to pan and zoom in the same gesture. Movement also pans when the pinch scale is momentarily unchanged. Pinch movement never scrolls the chat, including at Fit. Lifting or cancelling the gesture restores normal scrolling.

Once zoomed, two-finger scrolling or dragging pans inside the fixed-height image viewport. Gestures do not scroll the surrounding transcript, even at an image edge. At 100%, ordinary scrolling scrolls the transcript as before. **Fit** restores the original framing. **Open** or a normal click still opens the full-window viewer, but completing a drag does not.

Interaction state belongs to the currently mounted image row. Ordinary rerenders retain it, while replacing an image or unmounting its virtualized row resets it to Fit.

## Verification

- `cargo test -p jcode-desktop-ui inline_image --lib` exercises hover-pinch, pointer anchoring, limits, fit, scroll routing, drag capture, click suppression, and image identity.
- `python3 -m unittest discover -s scripts -p test_image_preview_acceptance.py` checks the native acceptance harness.
- `python3 scripts/screenshot.py target/inline-image-review.png --transcript image --image-interact` builds the real app and runs native inline zoom, drag, fit and full-window viewer regression checks on a private Xvfb display. Use a new output filename for each run.

## Linux input patch

Only `gpui_linux` is patched from the pinned Zed fork. Its shared GPUI dependencies still use the original upstream revision so the host and hot-reloaded UI keep identical event types. Wayland `dx/dy` and XInput pinch `delta_x/delta_y` are forwarded as precise pixel motion immediately after each scale update. Cumulative native scales are converted to multiplicative zoom deltas.

`cargo test -p gpui_linux linux::pinch::tests --lib` verifies the native conversion helper. Inline image tests cover continuous pinch plus translation without intermediate rendering, Ctrl-held translation, and scroll routing at Fit.

Native input dispatch belongs to the desktop host, not the reloadable UI. Activating this platform patch requires a newly launched desktop process, not just Ctrl+R. Do not automatically terminate an existing single-panel window: its unsent draft, attachments, and queued prompts are not durably restored. Preserve that state before manually reopening the session.
