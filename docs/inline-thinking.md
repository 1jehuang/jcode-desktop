# Inline thinking presentation

Desktop reasoning now uses the transcript's normal left alignment and a smaller,
muted theme-aware font. It has no role caption, thinking label, disclosure
controls, outer background, or border. Live, settled, and restored thoughts use
the same renderer and retain their full text. Markdown stays selectable, and
thinking headings and list markers use compact, muted styling.

Markdown inline spans now resolve their default style during GPUI layout rather
than capturing the window's style before the parent is applied. This makes the
reasoning color actually reach the glyphs and preserves inherited heading weights.

## Verification

- `cargo test -j 2 -p jcode-desktop-ui --lib -- reasoning markdown::tests --test-threads=1`:
  30 passed. Includes first/restored/adjacent/live reasoning role suppression,
  full text through the reasoning-done event, identical live/settled dimensions,
  compact headings, and selecting/copying rendered Markdown without its delimiters.
- `python3 -m unittest discover -s scripts -p test_screenshot.py -q`: 7 passed.
- The full built UI suite: 320 passed, 5 failed, 6 ignored. The failures were
  `email_inbox_moves_when_the_user_scrolls`,
  `restored_scroll_is_not_replaced_when_history_reattaches`, both
  `workspace::hover_scroll_tests` cases, and
  `a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`.
  These exercise scrolling rather than the changed reasoning renderer.
  Concurrent scrolling work was present in the checkout. No clean full-suite
  claim is made, and no before/after baseline was run for those failures.

Reproduce the isolated visual checks without touching the user's desktop:

```sh
python3 scripts/screenshot.py target/reasoning-inline-dark.png --transcript reasoning
python3 scripts/screenshot.py target/reasoning-inline-light.png --transcript reasoning --theme neutral-light --no-build
```

The real-app dark/light screenshots were inspected. The first light-theme
capture revealed the inherited warm reasoning color was too faint (2.49:1).
Neutral light/dark now use their own secondary-text color for reasoning.
The corrected light color measures 5.64:1 against the panel background.
`every_preset_keeps_semantic_text_legible` now also covers reasoning.

Final build and visual verification succeeded with `CARGO_BUILD_JOBS=1`.
Inspected artifacts: `target/reasoning-inline-light-final.png` and
`target/reasoning-inline-dark-final.png`. They show complete
Markdown content without thinking chrome, with dimmed text distinct from the
answer. Selection itself is covered by the passing GPUI interaction test above.
Two attempts to rerun the expanded tests after the palette-only change were
externally terminated with SIGTERM during rustc. Those attempts are not passes.

The running desktop was launched without hot reload. A restart requires user
approval because unsent drafts and host-owned terminal state could be lost.
