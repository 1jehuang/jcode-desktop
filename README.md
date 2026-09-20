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

## Development

With Rust and a sibling [Jcode checkout](https://github.com/1jehuang/jcode):

```sh
cargo run -p jcode-desktop
```

Press **Ctrl+R** to rebuild and hot-reload UI changes.

Agent sessions opened in this checkout automatically use
[Desktop self-development mode](docs/desktop-selfdev.md), with a Desktop-specific
system prompt and `desktop_selfdev` tool, separate from CLI/TUI selfdev.
