# Inline HTML previews

Jcode Desktop recognizes **completed** fenced `html-preview` blocks as opt-in
interactive artifacts. Ordinary `html` fences, unfenced HTML, incomplete streamed
fences, and reasoning remain text/code. No HTML is interpreted by the host app.

````markdown
```html-preview
<!doctype html>
<style>body { font: 18px Inter, sans-serif; padding: 20px }</style>
<p style="font-family: 'Liberation Serif', serif">A real font specimen.</p>
<button onclick="this.textContent = 'Clicked!'">Try it</button>
```
````

Use self-contained HTML/CSS and optional inline JavaScript. Installed system
fonts are available by family name. Images and additional fonts must be supplied
as data URLs. External URLs, local paths, fetch, workers, forms, downloads, and
popups are not supported. Choosing something inside a document does **not**
change app settings or send a message. The user can tell the agent their choice.

## Surface

- Native message card with **Source**, **Copy**, **Expand/Collapse**, and
  **Pause/Retry** controls. Copy copies source, not browser-selected text.
- Browser-rendered pixels at 2x density, clipped and laid out by GPUI. A private
  offscreen WebKit view receives pointer, drag, basic keyboard, and scroll input.
- Click the preview to focus it and scroll its content. Escape leaves preview
  focus. App control/command shortcuts are not forwarded to generated content.
- No separate browser window is opened and GTK never joins GPUI's event loop.
- Rendering happens in short bursts after load and interaction, not continuously.
  This first implementation is for documents and controls, not video, games, or
  continuous animations. IME, native browser accessibility, browser text copying,
  and native popup widgets are not implemented.
- State lasts while the preview remains mounted. Pause retains the last frame.
  Retry, history remount, and app reload restart the document. HTML source remains
  part of the original transcript, so no SDK/protocol migration is needed.

## Isolation

The Rust host launches its embedded Python helper with `-I`, an allowlisted
environment, and piped JSON messages. No tokens, app sockets, user browser
profiles, or Wayland/X11 display sockets are inherited. The helper creates a
private Xvfb display, temporary HOME/XDG directories, and an ephemeral WebKit
context with the browser process sandbox enabled. There is no app RPC bridge.

Generated HTML is escaped into an opaque-origin `srcdoc` iframe with only
`allow-scripts`. Both the enclosing document and the generated document have
restrictive CSPs. The enclosing policy cannot be relaxed by generated markup.
Navigation policies reject external and file URLs. Permission requests,
downloads, popups, clipboard access, media capture, WebRTC, WebGL, local storage,
DNS prefetch, and hyperlink auditing are disabled or denied.

This is defense in depth against web content, not a claim that browser-engine
vulnerabilities are impossible. Keep WebKitGTK updated. There is no hostile-code
container with a hard per-preview memory quota yet.

Limits: 256 KiB UTF-8 source, three active previews, bounded command/frame queues,
1600×900 CSS viewport, 8 MiB PNG frames, initial-render and hung-snapshot timeouts.
Old decoded images are evicted. Unmount/pause stops the helper and its private
process group. Unsupported platforms or missing runtime dependencies show an
error card with source/copy still available. No sandbox-disabling fallback exists.

## Runtime

Currently Linux-only. Arch packages: `python-gobject`, `python-cairo`,
`webkit2gtk-4.1`, `xorg-server-xvfb`. The runtime is embedded in the UI library,
so installation does not require finding a source checkout. macOS/Windows need
an equivalent isolated backend before interactive rendering is enabled there.

## Verification

```sh
cargo test -p jcode-desktop-ui --lib html_preview
python3 scripts/test_html_preview.py
python3 scripts/screenshot.py target/html-preview.png --transcript html --html-interact
```

The Python suite drives the actual sandboxed WebKit helper, verifies visible
click/keyboard/slider/scroll/resize changes, checks attempted local/network access
against a loopback request counter, and tests envelope escaping/size limits.
The screenshot path renders the actual GPUI chat with the bundled
`assets/previews/font-pairings.html` artifact, on an isolated desktop, then drives
the native Copy, Choose, slider, keyboard, Reset, scrolling, Escape,
expand/collapse, paused-input, retry, and source controls. Clipboard checks use
only the private X11 display. The native interaction check additionally requires
`xdotool` and ImageMagick.

See [acceptance evidence](html-preview-acceptance.md) for the requirement-to-check
mapping, measured before/after results, delivered-artifact identity, and explicit
validation boundaries.
