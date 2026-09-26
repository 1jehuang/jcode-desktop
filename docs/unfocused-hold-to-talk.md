# Unfocused hold-to-talk on Linux Wayland

Jcode Desktop can keep a Copilot hold active while another application has
focus. A compact native bottom-center layer-shell pill (160x28, status label
and 24 live microphone levels in one row) displays connection, listening and
transcription status without activating the chat window or reserving screen
space.

There is one indicator per hold, never two. When the Jcode window is focused,
the chat's own in-panel voice pill is the indicator and no OS pill appears.
The OS pill appears only while Jcode is unfocused. It mirrors the same panel
state, and it hands off live if focus changes mid-hold. After release the
transcript routes through Jev exactly like a focused hold (coding agent, quick
action, open one of the last 20 sessions, or keep in the draft). The OS pill
shows "Jev is choosing…" and then the decision, for example "Jev → Coding
agent" or "Jev unsure · Kept in draft", for five seconds.

## Enable explicitly

In the shared `~/.jcode/config.toml`:

```toml
[desktop.voice]
global_hold = true
global_devices = ["/dev/input/by-path/platform-i8042-serio-0-event-kbd"]
```

The example is the built-in keyboard on the Dell reviewed for this change.
Choose an explicit keyboard path for another machine. A standalone Desktop
config selected with `JCODE_DESKTOP_CONFIG` uses `[voice]` instead.
No devices are implicitly selected, and the feature defaults off.
`JCODE_DESKTOP_GLOBAL_VOICE=0` also disables it.

Requirements:

- A running native Wayland Jcode Desktop chat, not merely the Jcode daemon.
- A compositor supporting `zwlr_layer_shell_v1`.
- Existing read permission on the explicitly selected keyboard device.
  Jcode does not change device permissions, add groups, or install a privileged
  service. If access fails, focused in-app hold-to-talk remains available.
- A local active Wayland session reported by logind, with `LockedHint=false`.
- The existing Nari voice credentials and microphone setup.

Focus the desired Jcode chat once, switch to another app, and hold Copilot.
Release finishes the recording. Very short holds released during the initial
permission/connection checks are canceled. Multiple Jcode windows share an
exclusive capture lease, and the most recently focused eligible chat wins.
Focus changes during a hold do not move its transcript to another chat.
The main window, no-sidebar workspace, and standalone chat windows share the
same implementation. No host restart or daemon protocol change is needed.

## Input and safety boundaries

This is an explicitly configured evdev fallback for systems where a compositor
binding cannot deliver paired press/release events. It is not a claim that
all Wayland compositors support a global shortcut portal. The desktop portal
can expose the GlobalShortcuts interface without a working backend for the
active compositor. No portal permission prompt or compositor binding is
silently installed by this implementation.

The listener applies per-client kernel event masks before its first read and
flushes pre-mask queued events by changing the event clock. Only Copilot/F23
and the modifier keys required to recognize or reject its chord are selected.
Ordinary typing is not delivered to this client. There is no device grab,
key logging, synthetic input, or forwarding into the foreground app.
Devices already held when opened cannot start recording until released and
pressed again. Unsupported masks fail closed.

Device loss, event overflow, lost eligibility, registry failure, the 120-second
hold limit, and owner teardown cancel rather than finalize a capture. Capture
state is not recovered after UI reload or process restart. Cleanup is bound to
an exact attempt, so an old overlay cannot cancel a newer local recording.

A fresh bounded logind permission check precedes microphone startup and final
transcript insertion. During capture, checks repeat every 250ms with a 200ms
check deadline. Errors, inactive sessions, remote sessions, and reported locks
cancel capture. This is polling, not instantaneous lock notification, and
`LockedHint` depends on the screen locker reporting its state. Do not treat
this fallback as a guarantee about a locker that does not integrate with
logind. The input path never requests keyboard focus for the overlay.

## Verification

The full end-to-end check runs the production path, with no fixture:

```sh
python3 scripts/accept-global-voice-e2e.py target/voice-e2e \
  --binary target/debug/jcode-desktop \
  --sway-prefix target/headless-sway-tools --speech phrase.wav
```

It uses a private headless Sway with a focused foreign app. A virtual Copilot
key (`scripts/virtual_copilot_key.py`) is tagged as a joystick, so the live
compositor ignores it, and the script checks that. Jcode's capture stream is
pinned to a private virtual source, and the live PipeWire link is asserted
before any speech plays, so the real microphone is never used. Real Nari
transcription costs a few cents per run. The script asserts that the OS pill
shows while the foreign app keeps focus and that the draft grows by the full
transcript. It then focuses Jcode and asserts that a second hold shows no OS
pill but still inserts text. `global voice:` log lines record each decision,
and they log lengths only, never transcript text.

The narrower checks:

```sh
cargo test -p jcode-desktop-ui --lib voice
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 -m unittest discover -s scripts -p test_accept_global_voice_overlay.py
python3 scripts/screenshot.py target/global-voice-ui-review.png --preview-state voice-listening
python3 scripts/accept-global-voice-overlay.py target/global-voice-wayland \
  --binary target/debug/jcode-desktop \
  --sway-prefix target/headless-sway-tools
```

The unit tests use synthetic input events and private registration files, not
live input devices. Panel tests cover draft-only insertion, stale-attempt
isolation, release during connection, cancellation, and final permission denial.
The Wayland harness uses a private headless Sway display and the real native
layer-shell window. It checks foreground focus, unchanged target geometry,
visible waveform/status transitions, zero exclusive zone, no keyboard
interactivity, and removal on completion and process exit. Its offline fixture
cannot open the microphone or input listener. The required Xvfb render checks
the existing in-app voice design separately.

These checks do not establish physical Copilot firmware behavior or a live
microphone-to-Nari transcription on the user's current desktop. A physical
hold/release trial is a separate acceptance check, never simulated into the
user's active session.

## Holding Copilot over a Jcode CLI terminal

If the focused window is a terminal running the Jcode CLI (TUI), an unfocused
hold records in Desktop and sends the final transcript to that CLI session
instead of opening a new voice window. Delivery goes through the shared
server, like `jcode transcript --session <id>`, so the CLI submits it as a
prompt (it steers the turn if one is already running). The OS pill shows
Listening, Transcribing, Sending to CLI, and then "Sent to CLI" or the
reason it failed.

The session is found from the focused niri window. It uses the CLI's own
PID-to-session record (`~/.jcode/client_sessions`, written on focus), then the
window title, then the client's argv, then the last-focused CLI session. Any
other focused app keeps the new-window behavior. Resolution runs off the UI
thread, and a release before resolution finishes counts as a tap and is
ignored.
