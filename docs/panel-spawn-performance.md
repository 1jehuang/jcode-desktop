# New-panel latency, 2026-09-06

## Attribution

A fresh session through the running harness API, without sending a model prompt,
measured **0.021 ms socket connect, 1.658 ms hello, and 1804.314 ms create**.
Runtime diagnostics attributed 1358 ms to Agent construction (1512 ms total
setup) and only 10 ms to subscribe handling. The constructor synchronously
superseded the previous process telemetry session. That path flushed pending
telemetry over the network, imposing repeated 800 ms timeout budgets on opening
a new panel. This was not primarily a panel rendering cost.

The Jcode repository fix queues supersession telemetry instead. Explicit
shutdown/crash delivery retains its bounded blocking semantics. See `jcode/docs/SESSION_CREATION_LATENCY.md` for its regression evidence.
A matched private real-API experiment, with all HTTPS trapped by a slow loopback
proxy and no inference requests, measured warm session creation at **1670.033 ms
median before, 17.427 ms after** (three samples each, 98.96% lower). Agent telemetry
initialization fell from 1622–1625 ms to 0 ms in runtime diagnostics. Cold first
creation was measured separately and is not included in that comparison.

## Desktop changes

- Show and focus a native editable local draft immediately, before SDK creation.
- Attach the eventual session to the same panel/editor rather than replacing it.
- Correlate concurrent creations, retain drafts on failure, and never resurrect
  a closed draft or steal focus when an older creation completes.
- Re-create pending local drafts safely across UI reload, preserving the original
  directory and editor state.
- Log one `jcode desktop spawn: {JSON}` record per local creation attempt.
  `connect_ms` includes the SDK handshake, `create_ms` is runtime initialization,
  and `total_ms` is their sum. `session_id` joins successful records to panels.
  Connection failures have null `create_ms`. These are local diagnostic logs,
  not uploaded telemetry. No prompts, directories, error text or credentials
  enter these records.
- Only the original startup request keeps its existing retry behavior. Ordinary
  new-panel requests are single-attempt, including failures with ambiguous
  outcomes, to avoid creating duplicate sessions.

## Reproduction

```sh
python3 -m unittest discover -s scripts -p test_profile_panel_spawn.py -v
cargo test -p jcode-desktop-ui --lib spawn_profile -- --test-threads=1
cargo test -p jcode-desktop-ui --lib pending_tests -- --test-threads=1
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/profile-panel-spawn.py target/panel-spawn-normal --samples 8
python3 scripts/profile-panel-spawn.py target/panel-spawn-delayed \
  --samples 4 --create-delay 1.5 --verify-early-input
```

Use `--binary` and `--jcode` to pin before/after executables. Each output directory
must be new. The profiler launches real isolated daemon and API bridge processes,
a private Xvfb display, and the actual desktop binary with native keyboard input.
It never submits an inference request or inherits the user's credentials,
sockets, settings, or active display. The optional proxy forwards the real API
unchanged but delays only `CreateSession`, making the editor's independence from
backend readiness observable. Early-input acceptance checks native typing,
clipboard or rendered OCR text after attachment, focus, and unchanged panel
entity identity. On systems with Tesseract, the pending text is also checked in
pixels captured before attachment.

`panel_render_state_ms` ends at the app's next public rendered diagnostic state,
not GPU presentation or physical input-to-photon. `attached_ms` ends when that
state carries the real session ID. The profiler saves SDK stage records and PNG
screenshots alongside the complete samples. The first delayed pending screenshot
is captured before backend attachment.

## Baseline

Eight normal isolated creates with an explicit lazy provider and telemetry
disabled measured **39.47 ms median** to panel render state. This did not reproduce
the live constructor stall and must not be presented as the live baseline.

Four native creates with a controlled 1500 ms API delay measured **1527.56 ms
median** to panel render state before the immediate-draft change. This controlled
case isolates the frontend's waiting behavior from backend optimization.

Evidence is retained locally under `target/panel-spawn/`.

## After and delivery

The first native after-run, using the **same old backend binary** to separate UI
changes from runtime changes, measured **14.04 ms median** to panel render state
with the same 1500 ms API delay (four samples). All four typed drafts survived
attachment to the same editor. Without injected delay, eight creates measured
**10.24 ms median** to panel render state. These numbers do not include the
normal visual transition or establish physical presentation latency.

Pixel inspection then caught an additional first-frame problem: the old full-width
panel was still shrinking, clipping the new editor off the right edge even though
its native entity and focus were ready. Local draft creation now snaps sibling
widths and its target camera position as well. Separate first-frame GPUI geometry
tests cover full-width and overflowing strips rather than relying on panel counts.

The optimized backend binary is built and published to the local current channel.
The already-running shared daemon is deliberately **not restarted** while it has
active generations. Its backend speedup takes effect on the next safe daemon
restart. Immediate editable panels are independently deliverable via Desktop's
Ctrl+R reload, without interrupting those model streams.

### Final pixel-verified result

After the geometry correction, the same 1500 ms delayed real API and the same
baseline runtime binary gave **61.31 ms median** panel render-state latency
(samples 17.48, 68.96, 60.04, 62.58 ms), down from **1527.56 ms** before. This is
about **96% lower**, despite ongoing compiler contention. All four native runs
kept typed text and the same editor through attachment. OCR additionally verified
text in the fully visible pending composer before the first attachment. That PNG
was read and visually inspected, not accepted solely from diagnostic state.
The earlier 14 ms run excluded the final geometry correction and is retained as
intermediate evidence rather than substituted for this final result.

Evidence: `target/panel-spawn/final-delayed/results.json`, `pending-input.png`,
and `attached-input-*.png`. A fresh-session fixture from the prescribed screenshot
script also passed visual inspection at `target/panel-spawn/ui-review-empty.png`.
An earlier all-transcript fixture capture was black under load and was not counted
as successful visual evidence.

Regression mapping: seven GPUI pending-session tests cover immediate geometry at
1440/800 widths in both layouts, concurrent/out-of-order replies, closed drafts,
failed creation, reload with original directories and images, undo/selection, and
help-session correlation. Three Rust profile tests cover stage accounting and
retry policy. Three Python tests cover public state parsing, redirected log
collection, and transparent delayed-request forwarding. Backend coverage is the
62 telemetry unit tests plus production HTTP and real API before/after checks.
