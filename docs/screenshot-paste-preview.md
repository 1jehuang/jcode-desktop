# Screenshot paste preview

Pasting an image opens a transient, window-anchored preview beside the active
session. A usable right side wins, then the left. When neither side has 240px
available, the preview floats above the composer and stays within the viewport.
No session is inserted, resized, focused, or panned to make room. Neighboring
content can be covered briefly, but its layout and state are unchanged.

The preview holds for 900ms, then moves and shrinks into the image's measured
64×52 attachment slot over 420ms with smoothstep easing. The composer reserves
that final slot immediately, so the flight does not resize it. Image pixels use
contain sizing and the same cached image source throughout. Typing continues
without moving focus. A subsequent paste supersedes the animation while keeping
both attachments. Removal, submission, restore, and lost composer focus cancel
the transient preview. Hot reload restores attachments without replaying it.

## Verification

```sh
cargo test -p jcode-desktop-ui input::
python3 scripts/screenshot.py target/ui-review-paste-preview.png
python3 scripts/paste_preview_acceptance.py target/paste-right.png --no-build --side right
python3 scripts/paste_preview_acceptance.py target/paste-left.png --no-build --side left
python3 scripts/paste_preview_acceptance.py target/paste-compact.png --no-build --side compact
```

The acceptance wrapper reuses the production screenshot launcher and its private
Xvfb environment. It owns a synthetic clipboard image inside that isolated
display, sends native paste input, samples opening/flight/settled frames, checks
panel identity, width, focus and camera invariants, and reads the preserved draft
with OCR. It saves PNG frames and `.paste-evidence.json` next to the output.
No active-desktop clipboard, compositor controls, or network service is used.

The Rust tests cover right preference, left fallback, small viewports, timing
and exact landing geometry, repeated paste, removal, restore, lost focus,
submission, attachment payload preservation, and invariant composer bounds.

### Observed acceptance, 2026-09-12

- Input suite: 24 tests passed. Final focused preview suite: all 6 tests passed.
- Native right, left, and 800px-wide fallback runs passed with 54, 62, and 58
  sampled frames respectively. Each observed a held preview, intermediate
  shrinking/traveling frames, a stable 64×52 landing, unchanged panel/camera/focus,
  and the original draft after paste.
- Visually reviewed right opening/flight/settled, left opening, and compact
  opening PNGs under `target/paste-accept-{right3,left3,compact4}-*.png`.
- The live host's Ctrl+R-equivalent `--reload-ui` action rebuilt its release UI
  and logged successful activation of generation 3. Evidence is retained in
  `target/paste-preview-live-activation.log`.
