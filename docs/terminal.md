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

This is not full Kitty graphics or Sixel support. Non-inline image transports,
advanced placement/animation features, and persistent image scrollback remain
outside the supported subset. Images follow live-screen scrolling, disappear
when scrolled off the top, and are hidden while viewing text history. Sixel
payload recognition is not Sixel rendering. Cursor rendering is currently steady
rather than blinking. Text scrollback is configurable, defaulting to 10,000 lines.

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

## Verification

```
cargo test -p jcode-desktop-ui --lib terminal:: -- --test-threads=1
cargo test -p jcode-desktop --bin jcode-desktop host::resources::tests -- --test-threads=1
cargo build -p jcode-desktop
python3 scripts/terminal_acceptance.py target/terminal-handterm --require-debug-state
python3 scripts/screenshot.py target/ui-review.png --no-build
```

The terminal acceptance harness runs the real Desktop and shell on a private
Xvfb display. It exercises ANSI colors/cursor movement, native input, alternate
screen restoration, Kitty image pixels and deletion. Read the generated PNGs,
not only JSON assertions. Never use niri for verification.
