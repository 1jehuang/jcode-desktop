# Streaming panel feedback acceptance

Verified on 2026-09-05 for `c32b71c`.

## User-visible result

Working and streaming panels now have a small eight-step spinner beside their
title, an accent-tinted header and status row, and an activity label. Reasoning
shows **Thinking**, response text shows **Responding**, and tool activity retains
**Running tools**. Idle panels return to their normal styling. Reduced motion
keeps a static activity marker.

## Concrete acceptance evidence

The real desktop executable was rendered on the screenshot harness's private
Xvfb/Openbox display, with one streaming panel beside one idle panel. This was
not a mock renderer. The fixture is available with:

```sh
python3 scripts/screenshot.py target/streaming-review.png --transcript streaming --panels 2
python3 scripts/screenshot.py target/streaming-review-light.png --transcript streaming --panels 2 --theme neutral-light
```

Both screenshots were read and visually reviewed. To verify actual movement
rather than merely the presence of a spinner-shaped image, eight framebuffer
captures were sampled per theme, with a 200ms pause between captures. RGB crops
were compared pixel by pixel. At 1440x1000 in folder-tabs layout, the crops were
`14x14+288+58` for the spinner, `420x14+312+58` for the active title, and
`420x14+864+58` for the idle title.

| Observation | Warm neutral | Neutral light |
| --- | --- | --- |
| Distinct spinner frames out of eight | 7 | 4 |
| Changed spinner pixels on every consecutive comparison | 32 | 32 |
| Changed active-title pixels on every comparison | 0 | 0 |
| Changed idle-title pixels on every comparison | 0 | 0 |
| Active header RGB at (280,52) | (49,44,40) | (236,235,231) |
| Idle header RGB at (860,52) | (37,34,31) | (248,247,244) |

This confirms that the active panel communicates ongoing activity through both
motion and a measurable color difference, while neither title jitters and the
idle panel remains still. Local measurement artifacts are
`target/streaming-motion-acceptance.json` and
`target/streaming-motion-light-acceptance.json`, alongside the captured frames.

The existing 2-second screenshot settle delay initially produced black frames
under concurrent build/memory pressure. Repeating the same isolated harness
with a 15-second settle delay produced the reviewed images and measurements.

## Lifecycle and motion policy

`cargo test -j 1 -p jcode-desktop-ui --lib panel::activity -- --test-threads=1`
passed all three tests:

- Real GPUI event/render checks show the spinner and label for working,
  streaming, and tool activity, and remove both after the idle event.
  Reasoning/text events select Thinking/Responding. Connected and error states
  do not animate.
- The clock test proves only one timer is armed, one 125ms tick advances the
  frame once, a hidden spinner does not keep ticking, and reduced motion does
  not arm a timer.
- The opacity test proves the highlight rotates through all eight dots and
  wraps correctly.

The full UI suite returned 328 passed, 3 failed, and 6 ignored. All three failures
also reproduced in the pre-change test executable built at 15:14:52, before
the activity edits: email scrolling, restored history scroll offset, and the
touchpad gesture reticle. They are not counted as passing verification.

## Live delivery boundary

The feature was built, verified in the real isolated app, committed, and pushed.
The running user's host rejected the rebuild-and-reload request with
`hot reload is disabled; launch with --hot-reload`. It was not terminated:
restarting could discard drafts or workspace state. Applying the update to that
specific running instance remains deferred pending restart approval, not
reported as successful live delivery.
