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

## Whole-result rerun, 2026-09-06

After both production fixes, the complete mapped suite was rerun from 08:12:33 to 08:16:11 UTC using one copied set of the freshly built app, UI plugin, and test executable. Concurrent builds could not replace those verification artifacts.

```sh
python3 scripts/verify-image-flicker.py --output-dir target/flicker-whole-result
```

All fourteen build/check stages succeeded. This is **not** a claim that every repository test was run. The requirement-level observations below, rather than the aggregate stage count, establish the result. All artifact paths in this table are relative to `target/flicker-whole-result/`.

| Requirement / public behavior | Fresh check and actual observation |
| --- | --- |
| Distinct transcript and pasted images | `one-panel.log`: correct red attachment remained distinct from the chart across sixteen native captures. Every submitted frame contained 99,856 red pixels, versus zero red pixels in the preserved pre-fix reproduction. |
| One panel must not destabilize its neighbor | `two-panels.log`: sixteen native captures passed, neighboring content remained byte-identical, and each submitted-image frame contained 99,856 red pixels. |
| Reuse the same decoded attachment on submission | Both `.image-diagnostics.log` files contained exactly two decoder starts for the two distinct images, with no extra decode on submission and no image/layout anomaly signature. |
| Preview images must survive repeated opening and closing | `preview-lifecycle.log`: real wheel zoom, drag pan, double-click fit and Escape passed. Four additional open/close cycles passed 32 coordinate-stable captures and decoded the chart only once. The enlarged and closed blue bars had 26,271 and 5,940 pixels respectively. |
| Settled HTML preview must survive unrelated streaming | `streaming-image-matrix.log`: the real Panel retained one HTML preview entity across start/end, where the preserved pre-fix run created three. This verifies the actual lifecycle defect, not a native video of the original symptom. |
| Small-panel image geometry and history reconstruction | The same matrix passed both 640×480/900×600 viewports and tall/wide PNGs, retaining source identity, decoded pixels/Arc and image-card bounds through all three streaming/reconstruction cycles. |
| Track image failures through actual render callbacks | `cache-diagnostics.log`: injected asset eviction emitted `ready-to-pending`; invalid image bytes emitted `decode-failed` and `became-error`. The raw invalid-image fixture string was absent. |
| Track layout oscillations without repeated idle logging | `geometry-diagnostics.log`: actual painted ABAB editor heights emitted `panel_geometry_oscillation`; eight steady redraws produced no additional occurrence. |
| Bound diagnostic state and output | The fresh cache/geometry tests passed 128-entry eviction, 1,000-source churn bounded to 32 events per ten seconds, per-source/per-panel cooldowns, idle-gap reset, translation/subpixel filtering and unchanged-state suppression. |
| Preserve native HTML controls | `html-controls.log`: Copy, Choose, slider, keyboard, Reset, expand/collapse, paused input, retry, scroll, Escape and source view all passed against the rebuilt app. |
| Preserve streaming, selection and startup geometry | `streaming-neighbors.log`, `selection-neighbors.log`, and `layout-neighbors.log` passed nine streaming cases, both UTF-8 selection cases and all five startup lifecycle cases. |
| Image pixels must survive UI reloads | `reload-pixels/evidence.txt`: both actual isolated Ctrl+R library reloads changed zero of 885,500 compared pixels and retained 5,024 colorful pixels. These reloads reuse the pinned, already-built plugin. |
| Verification arguments and evidence safety | `fixture-arguments.log`: all eight argument tests passed, including incompatible modes and unsupported geometry. `driver-contract.log`: the new runner's help succeeded, missing output was rejected, and a nonempty evidence directory was protected. |
| Deliver the verified behavior to the running app | After the suite, a new live Ctrl+R rebuild/reload activated UI generation 6 at 08:16:55 UTC. `live-result.json` and `live-reload.log` record the fresh activation and no rebuild/reload failure. The immediate activation window contained zero anomaly events, not proof of long-term absence. |
| Identify the user's exact intermittent symptom | No unforced image/layout anomaly was logged in the cache and repeated-image-preview workflows. Two concrete related defects have before/after proof, but attribution of the user's exact symptom remains unconfirmed. GPU-only flicker is not excluded. |

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

## Remaining hypothesis probes

- **Settled HTML image remount on streaming boundaries, reproduced and fixed:** the real Panel regression `settled_html_preview_keeps_its_instance_across_streaming_ancestor_changes` recorded one initial browser-preview entity, a second when an unrelated response started, and a third when it ended. Each recreation discards the previous image and browser state. The transcript ancestor now retains the same element ID while its debug selector still reflects streaming. The same regression passes with exactly one retained preview through both boundaries. Before/after logs: `target/image-flicker-hypotheses-before.log` and `target/image-flicker-hypotheses-after.log`. This proves the remount defect, not a native recording of the user's exact intermittent flicker.
- **Encoded images under streaming/reconstruction in short windows:** the real Panel matrix passed at 640×480 and 900×600 with tall 64×256 and wide 256×64 PNGs. Across three streaming/history cycles per configuration, the source identity, decoded image Arc and pixel values, and image-card bounds remained stable. This hypothesis did not reproduce encoded-image flicker.
- **Neighboring behavior after the ancestor fix:** nine streaming tests, two text-selection tests, and five startup-layout tests passed. The rebuilt native HTML fixture passed Copy, Choose, slider, keyboard input, Reset, expand/collapse, paused input, retry, scroll, Escape, and source view. See `target/html-ancestor-*-tests.log` and `target/html-streaming-fix-native.log`.

- **Preview teardown/recreation after zoom and drag:** `python3 scripts/screenshot.py target/image-flicker-lifecycle.png --transcript image --image-interact` passed real wheel zoom, drag pan, double-click fit, and Escape. Four further open/close cycles produced 32 captured frames with identical blue-bar coordinates within each state. The existing asset decoded exactly once, with no image-state or geometry warnings. See `target/image-flicker-lifecycle.log` and `.preview-diagnostics.log`. This did not reproduce a preview-lifecycle flicker.
- **Live delivery of the ancestor fix:** the real Ctrl+R-equivalent rebuild-and-reload action succeeded at 08:10:32 UTC, activating UI generation 2 in the current desktop process. See `target/html-ancestor-live-reload.log`. The generation is process-local and does not supersede earlier generation numbers from a previous process.
- **Live recurrence while investigating:** the 08:06:40 UTC check on 2026-09-06 observed eleven image-state events and zero instrumented anomalies since the deployment baseline. See `target/image-flicker-hypothesis-followup.json`. Uninstrumented GPU-only flicker remains outside this negative evidence.

The feedback loop is closed for the reproduced cache defect and the tested rendering workflows. It remains open for the original intermittent symptom until it is captured or identified from an actual diagnostic occurrence. Absence of warnings is not proof that a GPU-only or otherwise uninstrumented flicker cannot occur.
