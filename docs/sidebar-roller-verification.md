# Sidebar hover switcher verification

Verified on 2026-09-12 with the production GPUI renderer on a private Xvfb display.

## Behavior

- Moving the pointer over the top-left roller opens a raised, readable pop-out.
- Six sidebar pages are shown together. Scrolling selects pages immediately.
- Wheel notches move one option each. Precise touchpad deltas accumulate at the existing 48px threshold and support either axis and reverse scrolling.
- Actions remain click-only. Passing todos, Todoist, email, folder, or new chat while scrolling never launches them.
- Moving from the header into the options keeps the pop-out open. Leaving it collapses it and retains the selected page.
- The pop-out does not reflow the sidebar or workspace, steal composer focus, or open during a stationary-pointer repaint or a drag.

## Checks

- `cargo test -p jcode-desktop-ui sidebar_roller -- --nocapture`: all 8 tests passed. Covers wrapping, precise scrolling, live selection, action safety, hover/collapse, layout stability, reload restoration, reduced motion, and folder/new-session click contracts.
- `python3 scripts/screenshot.py target/ui-review-roller.png --roller-interact --no-build`: passed against a freshly built binary. Native pointer hover reveals all six labels. Exactly four native wheel notches select Theme, with Midnight, Ocean, and Forest palette rows visible while the pop-out remains open. Pointer exit retains Theme, and clicking Chat restores the session list. Session identities and keyboard focus remain unchanged throughout.
- Visually inspected `target/ui-review-roller.png`. Labels are readable, the selected page is highlighted, and the raised card stays within the sidebar.
- Full serial UI suite: 746 passed, 8 ignored, 3 failures in concurrently changing code outside the switcher (tall-row Latest visibility, accounts-panel source reuse, and cross-strip animation boundaries). All sidebar roller, tutorial, sidebar selection, directory, and existing sidebar navigation checks passed.
- Sent the running host's Ctrl+R-equivalent `R` instance command. The host acknowledged it, rebuilt the UI, and logged `activated UI generation 1` for `target/release/libjcode_desktop_ui.so`. The activated library was newer than the switcher source and contained `Scroll to switch`. Evidence: `target/roller-live-reload.log`.

The screenshot runner refuses existing output paths, so the requested standard `target/ui-review.png` attempt was followed by the unique `target/ui-review-roller.png` artifact without overwriting other work.
