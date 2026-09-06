# Fresh-session composer acceptance

Verified on 2026-09-06 using the real desktop binary on a private Xvfb display.
Native X11 clicks and key events go through the application's platform input path.
The oracle measures painted border pixels and OCRs the actual typed and submitted
text. It does not use the application's layout calculations or GPUI test bounds.

## Observed improvement

| Window | Fresh input vertical bounds | Compact input vertical bounds | Input height | Moved upward |
| --- | --- | --- | --- | --- |
| 1440 × 1000 | 441–555 px | 935–976 px | 114 vs. 41 px, 2.78× larger | 494 px |
| 640 × 480 | 199–313 px | 415–456 px | 114 vs. 41 px, 2.78× larger | 216 px |

The fresh input centers were at 49.8% and 53.3% of window height, respectively.
Both were horizontally centered in the chat panel, within 1 pixel. Border corner
antialiasing is excluded from horizontal bounds, so these are painted-edge
measurements rather than logical CSS-style widths.

At both sizes the acceptance run observed:

1. Clicking the fresh input and typing `Fresh session typing works` visibly
   rendered that exact text. The input's painted bounds did not move.
2. Pressing Enter painted the submitted text in the transcript and moved the
   input to the bottom, where its height returned to 41 pixels.
3. Without another click, typing `Followup input retained` painted that text in
   the compact input. Keyboard focus survived the layout transition.

An initial run failed only because OCR read the small-font word `works` as
`uorks`. The follow-up probe was changed to an unambiguous phrase. Both full
runs then passed without weakening text, geometry, or focus assertions.

## Reproduce

Requires the screenshot script's Xvfb, Openbox, ImageMagick, and lavapipe setup,
plus Pillow, xdotool, and tesseract. Use new output names on repeat runs.

```sh
python3 scripts/screenshot.py target/fresh-native-1440.png \
  --transcript empty --fresh-interact
python3 scripts/screenshot.py target/fresh-native-640.png \
  --no-build --transcript empty --fresh-interact --size 640x480
```

Each run saves the initial, typed, submitted, and follow-up PNGs, OCR crops, and
an `.acceptance.json` with measured bounds and observed text. The verified run's
evidence is under `target/fresh-native-1440-v2*` and `target/fresh-native-640*`.

This uses offline fixture data and verifies local submission and layout, not a
remote model response. The focused GPUI tests additionally cover slash-menu
placement and resizing at 800 × 600. All ten input regression tests passed.
The running desktop's Ctrl+R-equivalent reload was separately confirmed by a new
mapped UI plugin and an `activated UI generation` diagnostic. Two unrelated
concurrent pinned-todo/prompt tests failed in the broader panel run.
