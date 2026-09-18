# Embedded terminal

Jcode Desktop uses the owned [Handterm](https://github.com/1jehuang/handterm)
`handterm-common` crate, pinned to a Git revision in Cargo.toml and Cargo.lock.
Do not copy or fork the terminal engine into Desktop. Fix parsing, protocols,
image storage, keyboard encoding, and core performance in Handterm first.

## Ownership and data flow

```
GPUI input -> Handterm keyboard/mouse encoding -> host PTY -> shell
GPUI canvas <- styled cells and image placements <- Handterm <- host output
```

- `src/host/resources.rs` owns native PTYs, reader/waiter threads, process
  lifecycle, resizing, and a bounded 4 MiB raw-output replay buffer.
- `crates/jcode-desktop-ui/src/terminal.rs` connects the host to Handterm,
  handles panel input, selection, clipboard gestures, focus, and scrolling.
- `terminal_keys.rs` adapts GPUI keystrokes to the shared keyboard encoder.
  Printable text uses GPUI's IME path. Shell Control keys remain available,
  except Desktop's Ctrl+R reload shortcut. Super shortcuts remain workspace-owned.
- `terminal_paint.rs` measures the actual monospace font and draws fixed-grid
  runs, backgrounds, cursor, and decoded Kitty images through GPUI. It does not
  bring in Handterm's standalone windowing, FreeType, or wgpu renderer.
- Image textures use Desktop's host-wide image IDs. Texture identity survives
  ordinary repaints. Replacement, deletion, panel closure, and generation disposal
  release obsolete GPU resources.

## Input and protocols

- Styled ANSI text, indexed/true colors, wide characters and grapheme cells.
- Terminal backgrounds follow the workspace's active/inactive pane surfaces,
  including padding, default cells, and status messages. Explicit ANSI cell
  backgrounds retain their own colors.
- OSC 10/11 foreground/background queries report Desktop's actual `TEXT` and
  pane surface colors, synchronized before parsing output, including after
  focus changes, replay/reset, and theme changes. Jcode uses the background reply
  at startup to select its light or dark palette. Already-running Jcode clients
  retain their startup choice and must be relaunched after switching Desktop
  between light and dark themes.
- Application-cursor keys and Handterm's shared legacy/Kitty keyboard encoder.
  GPUI does not expose all physical/keypad distinctions available to standalone
  Handterm, so those mappings are not claimed to be identical.
- Terminal mouse reporting, focus reporting, and scrollback wheel input.
  Hold Shift to select or scroll locally when an application captures the mouse.
- Drag selection, Ctrl+Shift+C / Ctrl+Shift+V and Shift+Insert.
  macOS also supports Cmd+C / Cmd+V. Ctrl+C still sends SIGINT through the PTY.
- Bracketed paste follows the engine's mode. Pasted ESC characters are stripped
  so clipboard content cannot inject a bracketed-paste terminator.
- Kitty inline RGB/RGBA/PNG images, chunking, compression, placement and deletion,
  within the shared engine's memory limits.
- Remote OSC52 clipboard writes are not automatically honored. Explicit user
  copy/paste gestures remain the clipboard permission boundary.

This is not full Kitty graphics or Sixel support. Non-inline image transports
and advanced placement/animation features remain outside the supported subset.
Sixel payload recognition is not Sixel rendering. Cursor rendering is currently
steady rather than blinking.

## Image scrollback

Images stay attached to their output as full-screen main-buffer scrolling moves
it into text history. Wheel scrolling projects those shared anchors into the
viewport. Partially visible images are cropped, not stretched or moved to the
viewport edge. Alternate-screen and scroll-region operations do not create fake
main-buffer history.

The shared Handterm engine owns retention, projection, deletion, and main/alternate
buffer isolation. Desktop only paints the projected geometry. Its texture cache
uses a separate image-content generation, so scrolling does not repeatedly hash
or upload the retained image pixels.

Retention follows the configured text scrollback (10,000 lines by default), with
Handterm's additional image limits: 16 MiB decoded per image, 64 MiB decoded total,
1,024 stored images, and 4,096 placements across the active and saved main buffer.
When text-history eviction removes an image's final placement, its pixels can be
released. Explicitly uploaded-but-unplaced images retain their normal semantics.
Image deletion also removes historical references, so scrolling cannot resurrect
an image that was explicitly deleted.

Resizing preserves history rows and their image anchors. Text columns are padded
or truncated to the new width, matching the engine's live-grid policy, rather
than fully reflowed. The Handterm internal placement wire format uses signed row
anchors and requires matching peer revisions. Desktop's host ABI is unchanged.

## Hot reload

PTY ownership stays in the executable while UI generations are replaced.
Panel snapshots store the terminal resource ID and consumed output cursor.
A replacement engine replays retained bytes without sending historical query
replies, then answers bytes after the snapshot cursor. Crash recovery clears
both fields because resource IDs/cursors are local to a host process.

The replay buffer is bounded, not a complete serialized emulator checkpoint.
A retention gap resets the emulator and replays the available tail. Very long
sessions can therefore lose terminal/image state established before that tail.
Resizing during replay is likewise not a historical resize-event log.

The existing C ABI is preserved. A reserved empty `terminal_read(0, 0)` probe
reports whether the host delegates query replies to the engine. A live UI upgrade
on an older host filters only replies to probes that host already answers, so
existing shells need not be killed for this migration. New hosts set
`TERM_PROGRAM=jcode-desktop`, `COLORTERM=truecolor`, and leave protocol replies to
Handterm. Loading a pre-Handterm UI into a new host is not supported.
An older host still answers OSC 11 with its hardcoded black background. A UI
hot reload cannot replace that host-side responder. Restart Desktop onto the
new host to enable accurate color queries, after saving any terminal work.

## Verification

```
cargo test -p jcode-desktop-ui --lib terminal:: -- --test-threads=1
cargo test -p jcode-desktop --bin jcode-desktop host::resources::tests -- --test-threads=1
cargo build -p jcode-desktop
python3 scripts/terminal_acceptance.py target/terminal-handterm --require-debug-state
python3 scripts/terminal_acceptance.py target/terminal-light --theme neutral-light --require-debug-state
python3 scripts/screenshot.py target/ui-review.png --no-build
```

The terminal acceptance harness runs the real Desktop and shell on a private
Xvfb display. It exercises ANSI colors/cursor movement, native input, alternate
screen restoration, Kitty image pixels and deletion, native-wheel image/text
scrollback alignment, exact partial-image cropping, and deletion while offscreen.
Read the generated PNGs, not only JSON assertions. Never use niri for verification.
