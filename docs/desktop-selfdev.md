# Desktop agent self-development

Jcode Desktop has its own **agent self-development mode**, separate from Jcode
CLI/TUI selfdev and from the native window's hot-reload switch.

Open a session whose working directory is this checkout or any subdirectory.
The shared Jcode runtime recognizes the `jcode-desktop` Cargo package and Desktop
UI directory, even if the checkout is renamed or reached through a symlink.
It adds a Desktop-specific system prompt and exposes `desktop_selfdev` instead
of the CLI `selfdev` and TUI `debug_socket` tools. Restored sessions use the same
working-directory detection. Ordinary projects and CLI selfdev stay separate.

## Development loop

- `desktop_selfdev status` identifies the checkout and running Desktop host.
- `desktop_selfdev inspect` inspects the Desktop preview catalog without opening
  a window or stealing focus.
- `desktop_selfdev build` builds Desktop, not the Jcode CLI.
- `desktop_selfdev test` runs Desktop tests.
- `desktop_selfdev screenshot` uses the repository's isolated offline screenshot
  harness. Inspect the resulting image before judging visual changes.
- `desktop_selfdev reload` and `desktop_selfdev build-reload` request the native
  host's **Ctrl+R** rebuild-and-reload action. A request acknowledgement is not
  proof of completed activation. Check the running host's reload result.

The running Desktop host must support hot reload for activation. Debug source
builds enable it by default. Release source builds need `--hot-reload`.
UI reload preserves the workspace and host-owned terminals. Host executable,
startup, and ABI changes need a safe, state-preserving restart, not a UI swap.
Do not restart a host with active terminals: an ABI version bump cannot transfer
its PTYs to a new process. Preserve those resources or wait for a safe restart.

Do not use CLI `selfdev build` or daemon reload to deploy Desktop UI changes.
Changes to the shared agent mode itself live in the adjacent Jcode repository
and do require updating the shared runtime. A Desktop-only rebuild cannot update
the agent daemon's embedded prompt or tool implementations.
Do not promote or reload a shared daemon while any session is processing.
Validate a new runtime on private sockets first, then activate it during a safe
idle window. Testing an isolated daemon does not activate existing live sessions.

## Verification

The Jcode repository contains detection, prompt, tool-isolation, and restored
session tests. Its `scripts/test_desktop_selfdev.py` acceptance check exercises
session creation through the harness API and inspects the real daemon's prompt
and tools without model inference. Keep these checks distinct from visual
acceptance using `python3 scripts/screenshot.py target/ui-review.png`.

For the real Desktop tool-to-UI path on Linux:

```sh
python3 scripts/accept-desktop-selfdev.py --jcode ../jcode/target/selfdev/jcode
```

This builds matching host/UI binaries, starts a private daemon and offline
Desktop on Xvfb, invokes the real Desktop tool, and verifies a new UI generation
with the same host, window, sessions, and draft. It also exercises the tool's
fresh-build screenshot action. It never reloads the shared daemon or restarts
the user's Desktop host.
