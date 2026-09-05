# HTML preview acceptance evidence

Observed on Linux, 2026-09-05. These are individual acceptance observations, not
an inference from an aggregate passing-test count. The native tests drive the
real GPUI application on an isolated X11 display and read only its clipboard.

## Reproduce

```sh
cargo test -p jcode-desktop-ui --lib html_preview
cargo test -p jcode-desktop-ui --lib markdown::tests
python3 scripts/test_html_preview.py
python3 scripts/screenshot.py target/html-preview-traced-acceptance.png --transcript html --html-interact
```

The screenshot runner preserves per-action PNGs next to the initial capture.

| Requirement / changed output | Check | Observed result |
| --- | --- | --- |
| HTML renders inline rather than as code | Native fixture screenshot | 1117×420 CSS-pixel browser surface inside the assistant message |
| Font choices genuinely render differently | Browser test_font_categories_render_distinct_glyphs | Same text measures 739 / 710 / 873 physical pixels in sans / serif / mono at 2× density |
| Choose pairing updates the preview | Native -chosen screenshot | 2493 status-region pixels changed |
| Slider changes font size | Native -interactive screenshot vs -chosen | 17843 text-region pixels changed |
| Keyboard reaches the focused control | Native -keyboard screenshot | 17316 text-region pixels changed after Right |
| Reset restores the original text size | Native -reset screenshot vs -chosen | 0 text-region pixels differ |
| Copy exposes exact source | Native Copy + isolated Gtk clipboard read | Clipboard equals the complete artifact source, not a screenshot or partial string |
| Expand and Collapse resize the card | Native -expanded / -collapsed screenshots | 700 / 420 CSS-pixel surface heights |
| Pause retains the frame and ignores input | Native -paused-input screenshot | 0 browser-surface pixels changed after clicking the paused slider |
| Retry restarts the document | Native -restarted screenshot | 18420 text-region pixels changed back to the default size |
| Scroll reaches browser content | Native -scrolled screenshot | 14798 text-region pixels changed; further font pairings become visible |
| Escape releases browser scroll focus | Native -escaped screenshot | 0 browser-surface pixels changed after additional wheel input |
| Source displays code rather than executing it | Native -source screenshot | No white browser surface remains |
| Basic JavaScript button / text input / resizing work | Browser test_renders_real_fonts_click_keyboard_resize_and_scroll | Click turns page green, typing turns it red, frame changes 1600×840 → 1200×1000, scrolling reveals blue footer |
| Explicit embedded images render | Browser test_explicit_data_image_is_rendered | Expected image pixel is blue |
| Generated content cannot access parent, storage, network or files | Browser test_document_cannot_escape_or_make_network_requests | All five attempted capabilities blocked; loopback request counter remains 0 |
| Only complete explicit fences opt in | Rust html_previews_require_an_explicit_complete_fence | Complete backtick/tilde preview fences accepted; html, raw HTML, incomplete and mismatched fences stay code/text |
| Oversized source is rejected | Rust rejects_large_documents_before_starting_a_process and Python test_size_limit_is_utf8_bytes | Over-256-KiB input rejected before spawning; UTF-8 bytes, not character count |
| Input tracks scaled layouts | Rust input_tracks_scaled_browser_surface | Wide and narrow surface coordinates map to the bounded browser viewport |
| The delivered chooser is the tested chooser | Saved final-answer artifact comparison | Exact normalized-source equality; 3002 UTF-8 bytes, SHA-256 below |
| Approved restart activates the implementation | Live process/socket/startup-log checks | New UI generation activated, sessions loaded; later reopened live executable contains the renderer |

Delivered artifact SHA-256 (surrounding whitespace normalized):
`360871a01dfa4d2eea0baaa2d2d38421a9c5c7b248ef9132292e413f0272543b`.

## What improved

Initial native pointer input used a canvas origin at the bottom of the surface,
so clicks arrived with negative Y coordinates. Anchoring the measurement canvas
to its top-left made the slider operate. Parent capture-phase wheel handlers
initially swallowed preview scrolling. Focus-aware hitbox occlusion now routes
scrolling to HTML and Escape returns it to the transcript. Before/after screenshots
and native actions verify both fixes, rather than relying on code inspection.

The expanded acceptance test also exposed a test false positive: comparing the
slider against the pre-selection screenshot counted selection layout changes.
It now compares against the chosen-but-not-resized baseline. The independent
slider, keyboard, and Reset observations above verify the corrected assertion.

## Boundaries

These checks do not establish subjective font preference, long-duration memory
stability, protection against browser-engine vulnerabilities, or non-Linux
backend support. The live desktop initially activated hot reload, but a later
low-memory termination required reopening. The subsequent live process was
verified at approximately 116–154 MB RSS during a short observation window.
No broad memory-leak fix is claimed, and the user's newly reopened session was
not restarted again merely to enable development hot reload.

The delivered artifact was matched to the tested document. Its rendering was
verified in the real isolated chat workflow, not by claiming a screenshot of the
user's current live conversation. Choosing a pairing intentionally does not
apply settings or send a message. Unsupported-platform and missing-dependency
error cards, IME, browser accessibility, video and continuous animation are not
covered by this acceptance run.

## Final whole-result rerun after mapping

All mapped checks were rerun together on 2026-09-05, finishing at 22:46:13 UTC,
after the requirement mapping and expanded controls tests were committed.
No mapped suite was skipped, aborted, or terminated. This is the HTML-preview
acceptance matrix, not a claim that unrelated palette or complete desktop test
suites passed.

- Explicit completed-fence handling, UTF-8/source-size rejection, position-specific
  identities, and wide/narrow viewport input scaling all passed again.
- All Markdown regression cases passed again, including the new opt-in behavior.
- Browser button/typing color changes, actual frame resizing, blue scroll footer,
  explicit data-image blue pixels, sandbox denial with zero loopback requests,
  envelope escaping, and the font sampler all passed again. Sans/serif/mono
  glyph widths remained 739/710/873 physical pixels.
- Copy again returned the exact document through the isolated clipboard.
- Delivered HTML identity was rechecked against the saved assistant answer:
  the same 3002-byte artifact and SHA-256 above matched.
- The main desktop instance socket was listening, and its live executable
  contained the renderer. Observed RSS was 181928 KiB. No extra restart occurred.

Fresh native observations, generated from `target/html-preview-final-matrix*.png`:

| Output/check | Fresh observed value |
| --- | --- |
| inline_surface_size | [1117, 420] |
| choose_status_changed_pixels | 2493 |
| slider_text_changed_pixels | 17843 |
| keyboard_text_changed_pixels | 17316 |
| reset_vs_original_text_changed_pixels | 0 |
| expanded_height | 700 |
| collapsed_height | 420 |
| paused_input_changed_pixels | 0 |
| retry_text_changed_pixels | 18420 |
| scroll_text_changed_pixels | 15415 |
| escape_then_wheel_changed_pixels | 0 |
| source_view_has_browser_surface | False |

The positive slider/keyboard/scroll changes and zero Reset/Pause/Escape changes
confirm both the intended interaction and preservation behavior. The generated
JSON measurements are in `target/html-preview-final-matrix.json`; the live
identity observations are in `target/html-preview-final-live-identity.json`.
The limitations in the preceding section remain unchanged.
