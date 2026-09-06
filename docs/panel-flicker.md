# Image and panel flicker diagnostics

Desktop records privacy-safe diagnostic events in its normal application log:

```sh
tail -F "${XDG_STATE_HOME:-$HOME/.local/state}/jcode-desktop/jcode-desktop.log" \
  | grep -E 'desktop-image|panel_geometry_oscillation'
```

- `panel_geometry_oscillation` records a repeated A/B/A/B layout pattern within a panel, its numeric entity ID, timestamp, viewport/editor rectangles, and pinned-prompt state. It uses four samples, ignores panel translation and subpixel noise, and emits at most once per panel every ten seconds. An idle gap clears the pattern. The observer never schedules a frame or notifies an entity.
- `desktop-image decode-start`, `decode-ready`, and `decode-failed` record the encoded source identity, GPU texture identity, and decode duration. These occur on asset loads, not every repaint. They do not include image data, labels, paths, or decoder error text.
- `desktop-image paint-state` associates an image with its numeric view ID and records unexpected ready-to-pending regressions, texture-ID changes, and errors. Tracking retains at most 128 view/source pairs, reports anomalies at most once per pair per ten seconds, and caps all paint-state events at 32 per ten seconds so cache churn cannot spam the log. Unchanged paint state emits nothing.

These are diagnostic signals, not a guarantee of detecting every visual problem. In particular, a GPU-only texture glitch can happen without a layout change. Inspect the image events around an oscillation or a reported occurrence.

## Reproduced image-cache collision

Composer attachments and transcript images previously each used an independent source-ID counter starting at 1. GPUI keys the shared asset decoder cache by the source's hash, which for `Image` is only its ID. As a result, displaying a chart and then pasting a different image into the composer showed the chart in the attachment preview. Reloading the UI also restarted those counters.

The fix uses GPUI's content-based encoded image identity for both paths. Reconstructing an identical attachment for submission or reload reuses its decoded pixels, while different images no longer alias each other. The app-lifetime high-range allocator still supplies GPU texture IDs, which are a separate identity layer.

This collision was reproduced with real native clipboard input on private Xvfb. Six pre-fix frames showed the chart instead of a solid red pasted image, with zero expected red pixels. This establishes an image bug, not yet that it is the cause of every reported flicker.

The corrected native workflow passed sixteen image-frame checks. All eight submitted-image frames contained exactly 99,856 red pixels. The decoder log showed only the two distinct image loads, with no additional decode when the attachment moved into the transcript.

## Repeatable verification

```sh
cargo test -p jcode-desktop-ui --lib image_cache::tests
cargo test -p jcode-desktop-ui --lib panel::flicker
python3 -m unittest discover -s scripts -p test_screenshot.py
python3 scripts/screenshot.py target/image-cache-review.png \
  --transcript image --image-cache-interact
```

The native test requires the screenshot harness's Xvfb, Openbox, lavapipe, ImageMagick, and xdotool dependencies, plus Python Pillow and GTK3/PyGObject for an isolated clipboard owner. It never reads or writes the live desktop clipboard. It verifies the existing chart and different pasted image together, submits the attachment, and checks sixteen repeated frames for image disappearance/substitution and stable final image geometry. Artifacts include screenshots and an `.image-diagnostics.log` file beside the requested output.
