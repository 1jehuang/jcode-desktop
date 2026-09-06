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
python3 scripts/screenshot.py target/image-cache-two-panels.png \
  --transcript image --panels 2 --image-cache-interact
cargo test -p jcode-desktop-ui --lib render_callbacks_detect -- --nocapture
python3 scripts/accept-mermaid.py --output-dir target/flicker-mermaid-reloads
```

The native test requires the screenshot harness's Xvfb, Openbox, lavapipe, ImageMagick, and xdotool dependencies, plus Python Pillow and GTK3/PyGObject for an isolated clipboard owner. It never reads or writes the live desktop clipboard. It verifies the existing chart and different pasted image together, submits the attachment, and checks sixteen repeated frames for image disappearance/substitution and stable final image geometry. Artifacts include screenshots and an `.image-diagnostics.log` file beside the requested output.

## Requirement-to-observation map

| Requirement or changed output | Concrete check | Observed result |
| --- | --- | --- |
| Reproduce the user's original intermittent one-panel flicker | Native image workflows in one and two panels, two isolated UI reloads, and post-deployment live diagnostic inspection | **Not established.** The image-cache collision below is reproduced, but is not presented as proof of the original flicker's cause. The follow-up at 07:56:08 UTC on 2026-09-06 observed nine image-state events across three reload activations and zero geometry, ready-to-pending, texture-change, or error signatures. See `target/image-flicker-followup.json`. This is limited negative evidence, not proof of no visual flicker. |
| Improve the reproduced image behavior | Load the chart, paste a different solid-red PNG, inspect the attachment, and submit it | Before: six captures showed the wrong chart and zero expected red pixels. After: the attachment was red, and all eight submitted-image frames had exactly 99,856 red pixels. See `target/image-collision-before/pasted-5.png` and `target/image-cache-final-transcript-frame-7.png`. |
| Keep the image stable in its panel without affecting another panel | `--panels 2 --image-cache-interact`, byte-compare the neighboring image/content crop across sixteen captures | Passed. The right panel's crop was byte-identical while the left attachment was pasted and submitted. All eight final left-panel frames had 99,856 red pixels. The expiring tutorial toast is excluded from this image/content crop. See `target/image-cache-two-panels-verified.log`. |
| Preserve decoded identity during submission | Count actual decoder starts in the native acceptance log | Exactly two decodes for the two distinct images. Submitting the pasted attachment reused its existing decoded texture. No ready-to-pending, texture-change, error, or layout-oscillation warning occurred. See `target/image-cache-final.image-diagnostics.log`. |
| Keep shared image rendering stable across real UI library reloads | Existing Mermaid acceptance sends Ctrl+R twice to an isolated hot-reload process and compares its 885,500-pixel crop | Both reloads changed **zero pixels**. Both retained exactly 5,024 colorful pixels. See `target/flicker-mermaid-reloads/evidence.txt`. |
| Track an occurrence through the actual image and panel render callbacks | `render_callbacks_detect_cached_image_eviction` deliberately evicts a decoded asset and supplies invalid image bytes. `render_callbacks_detect_layout_oscillation` alternates real painted editor bounds | Passed through the actual GPUI render callbacks: asset eviction emitted `ready-to-pending`, invalid bytes emitted sanitized `decode-failed` and `became-error`, and painted 100/200/100/200-pixel editor heights emitted `panel_geometry_oscillation`. Eight settled redraws emitted no new geometry occurrence. The fixture byte string was absent from the logs. See `target/flicker-render-callback-verified.log`. These are diagnostic fault-injection tests, not reproductions of the user's original intermittent symptom. |
| Avoid repeated or unbounded diagnostic output | Detector tests cover steady layouts, idle gaps, per-panel cooldowns, per-image cooldowns, 128-entry eviction, and 1,000 distinct source observations in one interval | Passed: steady observations do not log, geometry uses four samples, anomalous reports obey the ten-second cooldown, image tracking stays at 128 entries, and source churn emits at most 32 image-state events per ten seconds. |
| Expose repeatable isolated verification without accepting incompatible modes | `test_image_cache_probe_requires_its_isolated_fixture_geometry` and actual one/two-panel CLI runs | Invalid size, three panels, conflicting interaction mode, and wrong layout are rejected before app launch. Supported one/two-panel native workflows pass. |
| Deliver the change to the running application | Real live Ctrl+R-equivalent action, watching only newly appended application log data | Rebuild/reload succeeded and activated UI generation 4. New `desktop-image paint-state` events were observed. Production fix and diagnostics were pushed in `193c8d6`. |

The feedback loop is closed for the reproduced cache defect and the tested rendering workflows. It remains open for the original intermittent symptom until it is captured or identified from an actual diagnostic occurrence. Absence of warnings is not proof that a GPU-only or otherwise uninstrumented flicker cannot occur.
