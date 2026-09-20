# Jcode Desktop

A native desktop client for [Jcode](https://github.com/1jehuang/jcode), with AI coding sessions, terminals, and remote machines in one workspace.

**[Download](https://jcode.sh/desktop)**

- [User and developer guide](docs/desktop-guide.md)
- [Product vision](PRODUCT.md)

## Single-panel windows

Run `jcode-desktop --single-panel` for one chat panel in its own native window,
without the workspace sidebar, tabs, or navigation. Each invocation opens a new
independent window, even with other Jcode windows already open.
See [single-panel mode](docs/single-panel.md) for controls and details.

## Voice input

Click the microphone icon beside the composer, or hover over it to see the
platform shortcut. Press again to stop recording or cancel a pending request.

- **Copilot keyboards:** the physical Copilot key toggles voice. On Linux,
  global activation requires a compositor binding from `Super+Shift+F23` to
  `jcode-desktop --toggle-voice` with key repeat disabled.
- **macOS:** **Command+Shift+M** toggles voice, including from another app when
  the native global shortcut is available. The in-app shortcut remains available
  if global registration conflicts with another app.
- **Composer fallback:** **Ctrl+Shift+V**.

Audio streams to Nari. Recognized requests can open a recent session. Other
speech becomes an editable draft and is never sent automatically.

## Development

With Rust and a sibling [Jcode checkout](https://github.com/1jehuang/jcode):

```sh
cargo run -p jcode-desktop
```

Press **Ctrl+R** to rebuild and hot-reload UI changes.

Agent sessions opened in this checkout automatically use
[Desktop self-development mode](docs/desktop-selfdev.md), with a Desktop-specific
system prompt and `desktop_selfdev` tool, separate from CLI/TUI selfdev.
