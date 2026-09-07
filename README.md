# Jcode Desktop

A native, high-performance spatial desktop client built on the Jcode SDK.

See [PRODUCT.md](PRODUCT.md) for the product vision and requirements.

## New panel shortcuts

These shortcuts use the default machine selected under **Machines**. When the
default is remote, new session panels start in that machine's home directory.
The local directory behavior below applies when **This computer** is the default.

- **Super+Enter** opens a new session panel in your fixed most-used directory,
  just like Super+;. **Ctrl+Alt+Enter** is its non-Super alias. Hints and the
  tutorial teach Enter. Super+N still opens a session in home.
- **Super+;** opens a session in your fixed most-used directory.
- **Super+'** opens a session in your home directory.
- **Super+T** opens a terminal panel.
- **Super+Q** closes the focused panel, not the application. **Ctrl+Shift+W**
  is its non-Super alias. Ctrl+W remains word deletion in the composer.

## Remote machines

Click **Machines** beneath the sidebar tabs, or open **Settings → Machines and
default new-panel location**. SSH aliases from `~/.ssh/config` and previously
entered hosts appear automatically. Click **Connect**, or enter an SSH alias or
`user@hostname` and press Enter. Connections open normal native chat panels, not
terminal wrappers. Local and remote panels can remain open together.

Click **Set default** beside a machine to use it for new session panels, the `+`
buttons, and the first panel after restart. This does not move existing sessions.
Choose **This computer → Set default** to switch back, or use its **Connect**
button for a one-off local session without changing your default. Explicit local
folder actions, terminal panels, and email/help sessions remain local. Remote
panels show an **SSH · host** label and retain their target across UI reloads.

SSH uses the system configuration and keys/agent with host verification enabled.
First establish access with `ssh your-host` in a terminal to trust the host and
set up key authentication. Desktop never prompts for passwords or disables host
verification. The remote must have a Jcode version supporting `jcode api --stdio`
on its PATH or under `~/.local/bin`/`~/.cargo/bin`. Desktop does not install or
upgrade remote software. Connection failures appear in Machines, with an
explicit startup retry that preserves your draft. Remote file browsing is via
chat rather than the local Files tab.

The preference is saved without replacing unrelated settings:

```toml
[desktop.workspace]
default_remote_host = "desktop" # empty string selects this computer
remote_hosts = ["desktop", "user@build-server"]
```

For a standalone `JCODE_DESKTOP_CONFIG`, use `[workspace]` instead.

### Local pinned directory

Use Cmd instead of Super on macOS. Click **Default directory** at the top left
to open a regular, focusable panel for the local directory used by
**Super+Enter** and **Super+;**. Switch between it and chats with the usual panel
shortcuts or tabs. Clicking the control again returns to the same picker, keeping
your unfinished search. The panel can also be moved, resized, or closed like a chat.
Click a directory in the usage-ranked list to select it immediately, or browse
or type a path and click **Set as default**. This saves the preference without
creating a session or changing existing sessions. Saving closes the picker. Cancel leaves
the current default unchanged. Missing paths and files cannot be saved as a
default directory. Ordinary **Open folder** still opens a new session instead
of changing this preference.

The directory is saved as `pinned_working_dir` in `[desktop.workspace]` and stays
fixed across usage changes, reloads, and restarts. If you have not selected one,
it is chosen once from usable session history. Until that history arrives,
Super+Enter and Super+; use home without pinning that fallback. You can also edit
the setting directly, or remove it and restart to choose from history again.
For a standalone `JCODE_DESKTOP_CONFIG`, use `[workspace]` instead. For example:

```toml
[desktop.workspace]
pinned_working_dir = "/path/to/your/project"
```

Global Firefox bindings can consume Super+Enter before Desktop receives it.
See [global shortcut compatibility](docs/global-focus-shortcuts.md) for the
helper that preserves Firefox behavior and forwards the chord to Desktop.
The managed Super+; and Super+' launchers also route into Desktop when it is
focused and retain their terminal-launch behavior in other applications.
See the [shortcut requirement checks](docs/shortcut-requirements.md) for the
native Wayland workflow and all 42 Super-binding registration checks.

Native shortcut acceptance (Linux, after building Desktop):
`python3 scripts/accept-shortcuts.py target/shortcuts-acceptance`.
This uses private Xvfb and a real isolated Jcode daemon, verifies session
directories and keyboard focus, then changes history and restarts to check the
fixed directory. It makes no model requests. Use `--bridge /path/to/jcode-harness-api-bridge`
to test a newly built companion bridge rather than the installed version.
Add `--check-attachments` to verify the companion's explicit/default create
directories, persisted-session reattachment, and typed errors for missing cwd.
This writes a requirement-by-requirement `api-attachment-acceptance.json` report.

## Configuration

Workspaces 1–4 have consistent blue, teal, amber, and purple accents. Session
tabs carry their workspace number, and the selected tab has a filled badge and
stronger border. The optional minimap uses the same colors and clickable numbers.

The sidebar's **Theme** tab selects a palette and saves it automatically.
Choose from Warm neutral, Warm studio, Neutral dark, Neutral light, Midnight
(ink blue), Ocean (teal), Forest (sage), Plum (mauve), Rose dawn (blush paper),
and Parchment (cream and sepia). Code, input, and status colors follow the palette.
The separate **Settings** tab toggles the workspace minimap and pressed-shortcut
display for the current window. Scroll the sidebar tab strip to reach more tabs.
Theme cycling also remains available with Super+Shift+T (Cmd+Shift+T on macOS).

Jcode Desktop reads the `[desktop]` section of Jcode's shared
`~/.jcode/config.toml` at startup (`$JCODE_HOME/config.toml` when `JCODE_HOME`
is set). `JCODE_DESKTOP_CONFIG` can point to a standalone file containing the
same settings without the outer `[desktop]` prefix. Restart Desktop after
editing it.

All settings are optional and retain the current UI defaults when omitted:

```toml
[desktop.appearance]
ui_font = "Inter"
mono_font = "JetBrainsMono Nerd Font"
text_scale = 1.0                 # 0.75 through 2.0
reduce_motion = false

[desktop.appearance.colors]
bg = "#090909"                  # #RRGGBB or #RRGGBBAA
panel_bg = "#111111"
text = "#dcdcd7"
text_dim = "#8c8c8c"
accent = "#ba8bff"
user_accent = "#8ab4f8"
ai_accent = "#81c784"
error = "#ff6464"
warn = "#ffc864"
ok = "#64c864"

[desktop.workspace]
sidebar = true
showcase_keys = true
coaching_hints = true
session_refresh_seconds = 2      # 1 through 300

[desktop.terminal]
scrollback_lines = 10000         # 100 through 1,000,000
```

Every semantic color can be overridden. In addition to the example above,
the supported keys are `canvas_dot`, `panel_border`, `panel_border_focus`,
`panel_border_idle`, `header_bg`, `text_user`, `accent_dim`, `user_bg`,
`tool_bg`, `tool_text`, `reasoning`, `reasoning_bg`, `text_faint`,
`tool_border`, `error_bg`, `code_bg`, `code_text`, `inline_code_bg`,
`code_border`, `code_header_bg`, `code_gutter`, `code_keyword`, `code_string`,
`code_comment`, `code_number`, `code_type`, `code_punct`, `accent_muted`,
`quote_bg`, `table_stripe`, `input_bg`, `input_border`, `cursor`, `selection`,
`heading`, `link`, `minimap_track`, `minimap_track_active`,
`minimap_viewport`, `minimap_panel`, `minimap_panel_busy`, and `minimap_bg`.
Unknown or malformed color values are ignored with a diagnostic rather than
preventing the app from starting.

## Inline HTML previews (Linux)

Completed `html-preview` fenced blocks render interactive HTML/CSS/JavaScript
inside chat. Use them for font comparisons and self-contained UI demonstrations.
Ordinary `html` blocks remain source code. Preview cards include source, copy,
expand, and pause controls. Generated content has no network, file, or app access.

This optional backend requires Python GI, Cairo, WebKitGTK 4.1, and Xvfb. See
[HTML previews](docs/html-previews.md) for syntax, isolation, limits, and the
bundled interactive font sampler. Choosing a font in a preview does not change
chat settings.

## Headless screenshots (Linux)

Capture the real GPUI application without using the active desktop:

```sh
python3 scripts/screenshot.py target/ui-review.png
# Reuse an already-built binary:
python3 scripts/screenshot.py target/ui-review-wide.png --no-build --size 1920x1080
```

Requires `Xvfb`, Openbox, ImageMagick's `import`, and Mesa lavapipe (`vulkan-swrast`
on Arch, `mesa-vulkan-drivers` on Debian/Ubuntu). Rendering explicitly uses
CPU Vulkan rather than the desktop GPU. The script builds the app,
allocates a private X11 display, waits for a rendered workspace, captures a PNG,
and terminates its own processes. Existing output files are not overwritten.
It uses an allowlisted environment, temporary HOME/XDG/Jcode directories, no
desktop D-Bus connection, and an offline sample transcript. It does not access
Niri, the active display, live sessions, account credentials, or user settings.
The fixture renders production UI, but is not a capture of the live window or
evidence of live compositor performance. `JCODE_DESKTOP_SCREENSHOT=1` selects
the offline fixture internally. Prefer the script, which also supplies isolation.

## Updating on Linux

Type `/update` in Desktop to check and update a managed x86_64 archive install
under `~/.local/opt/jcode-desktop`, including its bundled runtime. Downloads are
SHA-256 checked and installed into a new version directory. The launcher changes
atomically, old versions are retained, and the app asks you to quit and reopen
when ready. It does not terminate active work or update a separate CLI install.
System/`.deb` installs must use their original installer. In a source checkout,
`/update` requests the same current-checkout rebuild as Ctrl+R, without changing
Git files. See [Linux updates](docs/linux-updates.md) for supported layouts,
security boundaries, error recovery, and verification.

## FPS counter

The FPS readout is built into the background strip above the tabs, not drawn
on top of them. It samples the presentation intervals of actual animation frames
at most four times per second. Deliberate idle gaps do not count as slow frames:
`FPS · idle` means no new animated frames were presented in the last sample.
Sampling only happens on existing redraws, so the counter never creates an idle
animation loop. The last sample remains visible until the next redraw.

Verify the backing strip, tab clicks, panel moves, and keyboard focus across four
native workspaces on a private display with:

```sh
python3 scripts/screenshot.py target/fps-header-review.png --panels 4 --fps-header-interact
```

## Native UI hot reload

The executable is a small, stable GPUI host. The application UI lives in the
`jcode-desktop-ui` crate, which is linked into normal builds and can also be
built as a development `cdylib`. This keeps GPUI's event loop and native window
alive while replacing the real workspace root, rather than launching a demo or
a second window.

```sh
cargo build -p jcode-desktop-ui
cargo run -p jcode-desktop -- --hot-reload
```

Debug builds launched from an available source checkout enable hot reload by
default, including `cargo run -p jcode-desktop` and direct `target/debug`
launches. Use `--no-hot-reload` to opt out. Release builds and offline screenshot
fixtures keep the linked UI by default. `--hot-reload [plugin-path]` or
`JCODE_DESKTOP_UI` can explicitly select a plugin, while `--no-hot-reload`
overrides both. Hot reload is an explicit rebuild action, not a file watcher.

Press **Ctrl+R** after changing UI code to rebuild `jcode-desktop-ui` and reload
the latest version from the current checkout. **Ctrl+Shift+R** performs the same
operation. The host checks the ABI, API-table size, pinned GPUI revision, and
state schema before activation. A failed build or load leaves the current root
intact. Press **F6** to roll back to the previous activated generation.
Workspace panels, strip layout, focus, drafts and attachments, transcript scroll
offsets, overlays, and folder picker state cross the handoff. Terminal processes
and PTY streams are owned by the host and reattached by resource ID.

The same-window invariant has a headless regression test, so it does not open or
replace a compositor window during verification:

```sh
cargo test -p jcode-desktop successful_reload_reuses_the_original_native_window
```

Old dynamic libraries stay mapped until process exit because GPUI entities and
callbacks may still contain their code pointers. Hot reload is therefore a
development workflow. Release builds use the same UI through the linked API and
remain a single `jcode-desktop` executable, so existing app packaging is
unchanged.

## Desktop releases

`desktop-v*` tags build Linux x86_64, Windows x86_64, and the existing secure
universal macOS release. Linux ships versioned `.tar.gz` and `.deb` artifacts;
Windows ships a versioned ZIP. Every package bundles the desktop executable,
Jcode CLI, harness bridge, and application artwork. The Linux package also
installs a freedesktop launcher. The Windows ZIP includes an external execution
manifest with DPI, long-path, and supported-OS metadata; executable icon resource
embedding remains a future signing-time enhancement.

The supported release matrix is intentionally explicit:

| Platform | Architectures | Window system / minimum version | Release validation |
| --- | --- | --- | --- |
| Linux | x86-64 | Wayland and X11 | Package contract, bundled CLI, and packaged-app launch on headless Weston and Xvfb |
| Windows | x86-64 | Windows 10 and 11 | Package contract, bundled CLI, and packaged-app launch |
| macOS | Apple silicon and Intel | macOS 13 or newer | Workspace/package checks, universal binaries, signing, notarization, DMG install, bundled CLI, and installed-app launch |

Other architectures, older operating systems, and alternative Linux package
formats are not advertised as supported until their release artifacts and smoke
tests exist. This keeps “supported” tied to a repeatable acceptance check rather
than compilation alone.

The macOS workflow remains the release owner: it fails closed on tagged builds,
performs signing and notarization, and creates the GitHub prerelease. The Linux
and Windows workflow waits for that prerelease before uploading, avoiding parallel
release creation races.

Local packaging requires a sibling Jcode checkout (or `JCODE_REPO`):

```sh
VERSION=0.1.0-beta.1 ./scripts/package-linux.sh
python3 scripts/verify-release-package.py dist/linux/Jcode-0.1.0-beta.1-linux-x86_64.tar.gz
```

```powershell
$env:VERSION='0.1.0-beta.1'; ./scripts/package-windows.ps1
python scripts/verify-release-package.py dist/windows/Jcode-0.1.0-beta.1-windows-x86_64.zip
```

Package contract tests run with `python3 -m unittest tests/test_verify_release_package.py`.

## macOS beta

Jcode Desktop supports Apple Silicon and Intel Macs running macOS 13 or newer.
The release bundle includes the Jcode CLI and harness bridge, so Finder launches
do not depend on Homebrew, shell startup files, or a separately installed CLI.

Download the latest DMG from the [beta download page][releases], open it, and
drag Jcode onto the Applications link. The download page identifies the current
beta's signing status and any required Gatekeeper steps. New tagged releases
fail closed unless they are Developer ID signed and notarized; credentialed
tagged releases also check daily for Ed25519-signed updates through Sparkle.

[releases]: https://jcode.sh/desktop

### Build the app locally

Requirements: Xcode command-line tools, stable Rust, and a sibling checkout of
[`1jehuang/jcode`](https://github.com/1jehuang/jcode). Then run:

```sh
./scripts/package-macos.sh
open dist/macos/Jcode.app
```

Set `JCODE_REPO=/path/to/jcode` when the runtime repository is elsewhere. The
script builds a universal application, signs it ad hoc for local testing, and
creates ZIP and DMG artifacts under `dist/macos/`.

### Publish a beta

Push a `desktop-v*` tag to run `.github/workflows/macos-beta.yml`. Tagged
releases fail closed unless all of these signing and notarization secrets exist:

- `APPLE_CERTIFICATE_P12` and `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`, and `APPLE_API_ISSUER_ID`
- `SPARKLE_PUBLIC_KEY` and `SPARKLE_PRIVATE_KEY`

Generate the update signing key once on a trusted Mac with Sparkle's
`generate_keys` tool (staged by `scripts/fetch-sparkle.sh`). Store the printed
public key as `SPARKLE_PUBLIC_KEY`, export the private key with
`generate_keys -x private-key-file`, and store that file's contents as
`SPARKLE_PRIVATE_KEY`. Keep an offline backup. Existing installations trust
that public key, so rotating or losing it requires following Sparkle's key
rotation procedure rather than simply generating a replacement.

Manual workflow runs may still produce an ad-hoc signed artifact when every
Apple credential is absent. That artifact is for testing only and is never
published as a tagged release. The package script verifies bundle metadata,
universal executables, hardened-runtime signatures, entitlements, the bundled
CLI, notarization tickets when present, and the DMG's Applications link.

For tagged releases, CI also signs the final notarized ZIP with Sparkle's
Ed25519 key and publishes `appcast.xml` at the stable `desktop-updates` release.
The app embeds only the public key and an HTTPS feed URL. Both the Sparkle
signature and the app's Developer ID signature must validate before an update
can be installed. The pinned Sparkle archive is checksum-verified during
packaging. Local ad-hoc packages omit update metadata and do not check the
production feed.
