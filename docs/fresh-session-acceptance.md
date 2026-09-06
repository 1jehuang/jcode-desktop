# Desktop first-prompt transition acceptance

This replaces the previous centered-then-bottom composer behavior. The goal is
continuity, not an animation hiding a layout jump: the editor keeps its size and
position while the submitted prompt starts at the top of the transcript. New
content consumes the blank space above the editor before moving it down.

## Observed before and after

Native X11 clicks and keyboard events drive the real GPUI Desktop application on
a private Xvfb display. Painted border pixels and OCR are independent of the
application's layout calculations. The fixture bridge is inert, so no model
request, account access, or live-session modification is involved.

| Window | Initial editor Y | Old submitted editor Y | New submitted editor Y | Submission movement |
| --- | --- | --- | --- | --- |
| 1440 × 1000 | 441–555 px | 935–976 px | 441–555 px | **494 px → 0 px** |
| 640 × 480 | 199–313 px | 415–456 px | 199–313 px | **216 px → 0 px** |

The editor remains 114 pixels tall rather than shrinking to 41 pixels. At the
wide size the first prompt's text moved from approximately Y879 to Y91. The
native wide growth sequence preserves the same width and height while moving
the editor down only as content fills the viewport, eventually settling at
Y846–960. Typing a followup without clicking proves keyboard focus is retained.

## Requirement-to-check mapping

- **No submission jump or shrink:** native exact before/after border comparison
  and `fresh_session_composer_is_centered_spacious_and_stable_while_typing`.
- **Prompt begins at the top:** native prompt OCR within the top transcript
  region, plus `fresh_session_tall_prompt_starts_at_top_then_follows_response`.
  The tall fixture uses actual paragraphs rather than collapsed markdown soft
  line breaks.
- **Move only as space is consumed:** native repeated submissions and
  `fresh_session_response_spends_space_before_moving_input`, at three window
  sizes. Short responses preserve bounds. Growth is downward and bounded by the
  footer, and incoming output restores ordinary tail following.
- **Every new submission remains visible:**
  `startup_each_native_submission_paints_the_newest_row` submits twenty prompts
  through the editor and checks the last row is above the composer. Native
  acceptance checks message 16 in the bottom transcript crop, not a pinned copy.
- **Manual scrolling does not jump to the tail:**
  `startup_tall_preview_manual_down_scroll_detaches_without_jumping_to_tail`
  checks both precise and discrete downward scrolling, then response arrival.
- **No history-loading timing dependence:**
  `startup_existing_history_has_identical_layout_before_or_after_welcome_paint`.
- **Reload preserves the committed initial layout and draft:**
  `startup_committed_layout_round_trips_reload_and_empty_history_loading_frame`.
  This checks serialized snapshots, an empty loading frame, then history arrival.
- **Resize and pinned-prompt changes settle without oscillation:**
  `startup_resize_and_repeated_notifications_preserve_settled_geometry` checks
  short/tall/narrow/wide sizes and eight repeated renders at each configuration.
- **Wrapped drafts clear their height immediately:**
  `submitting_wrapped_prompt_resets_height_before_the_next_paint` verifies that
  submit resets cached line count before another paint. Native narrow-window
  growth exercises the actual wrapped-input path.
- **No broad UI regression:** the complete final Desktop UI library run passed
  **382 tests**, with **0 failures and 7 ignored**. This is a full-suite result,
  not a count extrapolated from focused tests.

## Bugs caught while validating

A fixed-height, non-shrinking transcript initially caused a two-frame cycle when
pinning the offscreen prompt changed available height. Synchronous flex shrinking
now constrains the list in the same frame, and repeated-notification tests verify
that geometry settles.

Native narrow-window acceptance caught a cleared editor retaining a 294-pixel
wrapped height even after passive waits. Submit now resets `visual_line_count`
immediately instead of waiting for a paint to correct it.

Native tail OCR uses bounded passive retries under software-rendering/build
load. It does not type an extra character or scroll to make the expected row
appear. The crop excludes the pinned prompt. Geometry and focus assertions are
not relaxed by that retry.

## Reproduce

Use fresh output names. The script refuses to overwrite an existing capture.

```sh
CARGO_BUILD_JOBS=1 cargo test -p jcode-desktop-ui --lib -- --test-threads=1
CARGO_BUILD_JOBS=1 cargo build -p jcode-desktop
python3 scripts/screenshot.py target/transition-wide.png \
  --no-build --transcript empty --fresh-interact
python3 scripts/screenshot.py target/transition-small.png \
  --no-build --transcript empty --fresh-interact --size 640x480
```

Each native run writes before/typed/submitted/growth/followup PNGs and an
`.acceptance.json` file containing measured bounds, prompt position, OCR and
passive-settle attempts. Baseline captures are `target/desktop-transition-before*`.

## Delivery scope

The Desktop executable and UI library are rebuilt, not the Jcode terminal binary.
The original live process exited, so its refused reload was not counted as success.
A new real Desktop instance subsequently appeared. Sending the supported `R`
reload command to its default instance socket activated **UI generation 2** in
PID 2114668 without focusing or reopening a window. The activation log and the
loaded `jcode-desktop-ui-0002.so` mapping were checked. Its SHA-256 matches the
rebuilt `target/debug/libjcode_desktop_ui.so`:
`74cd62484778877e5b06d523e76a2edc44a754251fa3dfa30ebc1f58fb13b97b`.

Final native acceptance artifacts use `target/desktop-transition-accepted-small*`
and `target/desktop-transition-accepted-wide*`, including `.acceptance.json` and
submitted/growth/followup PNGs. Narrow-window growth settles at Y326–440 while
retaining the 114-pixel editor height. Message 16 remains visible above the editor,
and typing `Focus after growth` without clicking confirms focus survives growth.
