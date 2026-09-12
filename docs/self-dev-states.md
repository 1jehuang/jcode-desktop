# Self-development panel states

Self-development previews let an agent or developer open a panel in a named UI
state without making a model request, invalidating a token, or changing a real
session. These are real `Panel` instances using the normal event reducers and
renderers, not HTML mockups or copied error cards.

## Open a state in the running app

Run Desktop in self-development mode (`--hot-reload`, the debug-checkout default,
or `JCODE_DESKTOP_UI`). After changing UI code, use **Ctrl+R** to rebuild and reload.
The equivalent noninteractive request is `target/debug/jcode-desktop --reload-ui`.

```sh
python3 scripts/preview-state.py --list
python3 scripts/preview-state.py login-error
python3 scripts/preview-state.py model-access-error
python3 scripts/preview-state.py login-dialog-error
python3 scripts/preview-state.py --reset login-error
```

Commands return JSON. Add `--pid PID` when multiple self-dev instances are running.
Opening always adds and focuses a new panel, preserving existing panels. Reset
restores the newest open preview with that state, retaining its identity. It fails
rather than changing a live session if no matching preview exists. Each panel
has an **Offline preview** badge and a **Reset** button. Close it normally.

| State ID | UI under test |
| --- | --- |
| `empty` | Empty session/composer |
| `streaming` | Processing status and a partial assistant response |
| `login-error` | Authentication failure with native login/model recovery |
| `model-access-error` | Model unavailable for the current account |
| `rate-limit` | Provider usage/rate-limit recovery |
| `disconnected` | Connection failure and disconnected session status |
| `login-dialog-error` | Native sign-in dialog with an error |

Previews are transient. They are omitted from hot-reload and crash-recovery
snapshots, session history, and runtime subscriptions. Reopen the desired state
after a reload. They cannot be forked into real sessions.

## Isolation and control API

A preview owns an inert bridge, not the workspace's real runtime connection.
Authentication provider choices are canned. OAuth allocation, browser launch,
credential submission, prompt submission, and integration/terminal slash commands
are blocked in preview panels. Login and model recovery can be inspected without
changing accounts. Do not enter real secrets in test fixtures.

The control endpoint is Unix-only and disabled in normal packaged runs. Explicit
`JCODE_DESKTOP_SELF_DEV=1` enables it for isolated acceptance harnesses. Screenshot
runs do not inherit the development default. `--no-hot-reload` disables implicit
self-dev mode. The endpoint uses a private user-owned directory and a mode-0600
Unix socket under `$XDG_RUNTIME_DIR/jcode-desktop-preview/`,
with a private `jcode-desktop-preview-UID` directory in the system temporary
directory when XDG runtime is absent (for example, on macOS). Requests are bounded,
time-limited, newline-delimited JSON:

```json
{"command":"open","state":"login-error"}
```

`list` returns the catalog and `reset` accepts a state ID. Unknown commands, fields,
and state IDs are rejected. The server acknowledges an open/reset only after the
workspace has applied it, not merely after enqueueing a request. Socket ownership
and shutdown belong to the UI generation, so Ctrl+R does not require a host ABI
change. Windows can still build the UI and fixtures but has no Unix control socket.

## Test a state independently

```sh
cargo test -p jcode-desktop-ui --lib preview
python3 -m unittest discover -s scripts -p test_screenshot.py
python3 -m unittest discover -s scripts -p test_preview_state.py
python3 scripts/screenshot.py target/ui-review.png --preview-state login-error
python3 scripts/screenshot.py target/preview-model-error.png --preview-state model-access-error
python3 scripts/screenshot.py target/preview-acceptance.png --preview-state login-error --preview-interact
```

Screenshot commands build the app and render it on a private Xvfb display with an
allowlisted environment and temporary credential/configuration homes. Use
`--no-build` only when the binary is current. The interaction run uses the public
CLI to list/open/reset every state, inspects rendered pixels, clicks native
recovery controls, and verifies that unrelated panels remain present.

## Adding states

1. Add a stable, kebab-case `PreviewState` variant, ID, title, and catalog entry in
   `preview_state.rs`. This enum is the discoverable scenario contract, not a
   replacement for production session state.
2. Seed it in `panel_preview.rs` through ordinary `ApiEvent` transitions whenever
   possible. Clear unrelated fixture content first. Never connect a real backend
   or add a separate renderer just for the preview.
3. Add an individually named GPUI test asserting the visible state, reset behavior,
   and absence of backend effects. Test interactive actions, not just construction.
4. Add its ID to the screenshot CLI choices and acceptance catalog, plus any
   state-specific pixel/native interaction assertions.
5. Run the focused tests and real screenshot workflow, then reload the live app.

The separation is deliberate: typed scenario, deterministic setup, real UI,
isolated effects, and observable assertions. It provides a reusable foundation
for additional tool, permission, model-picker, loading, and empty-result states.

## Verification evidence (2026-09-12)

- Focused Rust preview suite: 31 passed, including per-state GPUI tests,
  native model/login interactions, cancellation, request validation, mode gating,
  socket permissions, bounded shutdown, and snapshot exclusion.
- Real private-display acceptance: all seven catalog states opened, rendered,
  reset in place, and closed without replacing the original panel. Authentication
  recovery opened the actual model picker and login dialog. Unknown state IDs
  were rejected without workspace mutation.
- Evidence: `target/preview-acceptance-v1-acceptance.json` and its per-state PNGs.
  Authentication and model-access screenshots were visually inspected.
- The running desktop successfully rebuilt/reloaded and exposed the seven-state
  catalog through its live self-dev endpoint.
