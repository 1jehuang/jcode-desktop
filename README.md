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

Hover over the microphone icon beside the composer to see the platform shortcut.
Hold the voice key to transcribe live, then release it to let Jev route the
utterance. Releasing while the microphone is still connecting cancels that attempt.

- **Copilot keyboards:** hold the physical Copilot key in the focused Jcode
  window, including standalone chat windows. On Linux, leave the key unbound in
  the compositor so both press and release reach that window. Losing focus ends
  local capture safely. A spawn-only global toggle binding cannot implement
  push-to-talk and may target the wrong window.
- **macOS:** hold **Command+Shift+M**. The native global shortcut forwards both
  edges when available. If another app owns it, the focused-window handler remains.
- **Click or composer fallback:** click the microphone or press **Ctrl+Shift+V**
  to start/stop recording without holding a key.

Audio streams to Nari. After local transcription, Jev sends or queues agent work,
performs a supported navigation action, or preserves uncertain input in the draft.
A persistent decision card shows the questions, Yes probabilities, and result.
Existing typed text and attachments are preserved. See
[voice routing](docs/voice-session-navigation.md) for provider setup and limits.
Explicit `--voice-press` and `--voice-release` commands are available for
integrations that can deliver both edges to an existing main host.

## Development

With Rust and a sibling [Jcode checkout](https://github.com/1jehuang/jcode):

```sh
cargo run -p jcode-desktop
```

Press **Ctrl+R** to rebuild and hot-reload UI changes.

Agent sessions opened in this checkout automatically use
[Desktop self-development mode](docs/desktop-selfdev.md), with a Desktop-specific
system prompt and `desktop_selfdev` tool, separate from CLI/TUI selfdev.
