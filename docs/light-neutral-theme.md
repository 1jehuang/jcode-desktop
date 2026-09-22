# Light Neutral

Replaces the initial ChatGPT Light interpretation, retaining `chatgpt-light` as a
read-compatible alias for the canonical `light-neutral` setting. The picker label
is **Light Neutral**. The older, warm `neutral-light` palette is unchanged.

## Design intent

The reference's character comes from restrained color usage, not merely sampled
hex values. The original implementation spread blue across controls and focus
outlines, and inherited violet prompt-age washes. This revision uses:

- White conversation and composer surfaces, on near-white neutral chrome.
- Soft gray prompt cards without the age-rainbow wash.
- Charcoal control accents and neutral selected-control fills (`#e9eaea`).
- Distinct dark heading/user ink and softer body/secondary ink.
- Neutral keyboard-focus outlines with at least 3:1 contrast against white.
- Blue for links and text selection, not general interface decoration.
- Existing readable syntax and status colors, without desaturating code or errors.

Font preferences and layout are unchanged. Other presets retain their existing
prompt-age colors. Tint strength is interpolated during theme transitions.

## Verification

- `cargo test -p jcode-desktop-ui --lib theme -- --test-threads=1`: 25 passed.
- `python3 -m unittest discover -s scripts -p test_screenshot.py`: 11 passed.
- Real-app isolated rendering: `target/light-neutral-redesign.png`, reviewed.
- Legacy saved ID rendering: `target/light-neutral-legacy-selection.png`.
  Pixel checks matched the canonical preset at content, prompt, chrome, and
  composer samples (`#ffffff`, `#f4f4f4`, `#f4f4f4`, `#ffffff`).
- Regression coverage includes name/ID compatibility, picker selection, keyboard
  cycling, contrast across all presets, untinted prompt cards, and transitions.
