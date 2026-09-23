# Dark Neutral

Dark counterpart to **Light Neutral**. Setting ID `dark-neutral`, picker label
**Dark Neutral**. The older `neutral-dark` palette (blue-gray tinted) is unchanged.

## Design intent

- Untinted charcoal surfaces: canvas `#1a1a1a`, focused pane `#212121`,
  recessed chrome and inactive panes `#171717`.
- Raised gray prompt cards (`#303030`) without the prompt-age tint wash.
- Soft gray body ink (`#d6d6d6`), brighter heading and user ink (`#f3f3f3`).
- Neutral focus outline (`#8a8a8a`, over 3:1 against the composer).
- Blue only for links and selection. Syntax and status keep semantic colors.

## Verification

- `cargo test -p jcode-desktop-ui --lib theme -- --test-threads=1`: 26 passed,
  including contrast, pane focus, neutrality, and untinted prompt checks.
- Picker click test covers the new row (`theme-preset-15`).
- Real-app isolated rendering: `python3 scripts/screenshot.py --theme dark-neutral target/dark-neutral.png`.
