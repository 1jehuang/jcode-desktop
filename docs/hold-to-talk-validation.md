# Hold-to-talk repair (2026-09-20)

## Failure found

The local compositor binding spawned `jcode-desktop --toggle-voice`, which targets
only the default main-workspace instance. At diagnosis, that socket did not exist:
the running Desktop instances were standalone chat windows with per-process
sockets. The command could not reach the chat the user was using. Static keymap
inspection alone in the earlier validation had not tested that routing boundary.

The installed compositor supports press-triggered spawn bindings, not paired
press/release bindings. Its consumed-key releases are suppressed. Replacing one
toggle command with another command therefore cannot implement push-to-talk.
The existing raw F23 mapping itself is not established to be incorrect.

## Repair

- Remove only the old Copilot spawn binding from the local config, retaining a
  timestamped backup. The key now reaches the focused Jcode window directly.
- Handle native key down and up in workspace, standalone and modal surfaces.
  Repeats do nothing. Release finds the original held capture without refocusing.
  Losing window activation ends locally held capture as a safety measure.
- Hold captures stream interim text, then append final dictation to the existing
  draft. They never invoke session-navigation classification or auto-send.
- Releasing while connecting cancels the attempt. Late startup completion is
  rejected by attempt identity, cancellation and phase guards.
- Clicking the microphone and Ctrl+Shift+V remain start/stop toggles.
- macOS Carbon and optional main-host CLI integration now expose separate press
  and release edges, with repeated presses and duplicate releases suppressed.
  Native host additions take effect on host startup, not through UI hot reload.

## Evidence and limits

- Final integrated voice suite: 36 tests passed, including the standalone-window
  key-down/key-up regression.
  Includes native key events, repeat suppression, release ownership after tab
  switches, click-recording isolation, startup cancellation, live text and drafts.
- Host suite: 46 tests passed. Actual compiled CLI was also exercised against
  isolated IPC sockets for press/release bytes and safe missing-host failure.
- Current debug build rendered `target/hold-voice-review.png` on a private Xvfb
  display. The live transcription overlay and microphone were visually inspected.
  This fixture represents click recording, not a physical hold.
- No physical microphone recording, keyboard monitoring or compositor command was
  used. Physical Copilot delivery and native macOS runtime remain user/platform
  acceptance checks. Linux hold-to-talk requires a focused Jcode window and ends
  if focus leaves it, rather than claiming unsupported global press/release.

The existing Ctrl+R-equivalent IPC reload was sent to eight standalone hosts.
Two windows closed during the work. After builds completed, all six surviving
hosts mapped new UI generations containing the hold-to-talk implementation. No
chat host was terminated or restarted by this change. Focus-loss safety was
reviewed in the production activation observer, not simulated by the test harness.
