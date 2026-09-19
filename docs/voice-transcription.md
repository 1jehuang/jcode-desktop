# Nari streaming voice dictation

Press the **Copilot key** or choose **Voice** in a chat's toolbar to start dictation.
Press again or choose **Stop** to finish. **Ctrl+Shift+V** remains available in the
composer. A live, revisable transcript appears above the composer while audio is
streaming. The final result is appended to the current draft as one undoable edit.
It is never submitted automatically. Typing and attachments are preserved.

## Global Copilot activation

The native Desktop host accepts `jcode-desktop --toggle-voice` through its private
instance socket. A global operating-system shortcut can invoke that command from
another app. Desktop requests window activation and routes the toggle to the active
recording, or otherwise the last active chat. Wayland may deny foreground focus,
but the toggle still reaches Desktop. It does not dictate into another
application. A recording remains owned by the chat where it began even when
selection changes. Non-chat document, terminal, and settings panels are excluded.

The command requires an already-running compatible Desktop host. It does not
launch a new recording implicitly during startup or crash recovery. On Wayland,
global keyboard shortcuts belong to the compositor, so the compositor forwards
the Copilot press and Desktop owns recording, networking, and transcript insertion.
The compositor binding must disable key repeat. Native in-app Copilot handling
also suppresses repeated held-key events. See `global-voice-shortcut.md` for setup.

## Credentials and privacy

Desktop connects directly to Nari with a personal API key. Set `NARI_API_KEY`, or
save `NARI_API_KEY=...` in the existing Jcode private-config convention at
`~/.config/jcode/nari.env` (respecting `XDG_CONFIG_HOME`). Restrict that file to the
user, for example mode `0600` on Unix. Never put it in the repository. This is
separate from Jcode subscription voice entitlement and is billed to the Nari
account. No key is compiled into Desktop or shared with other users.

Voice capture is local even for an SSH-backed chat. The microphone opens only
after an explicit toggle and successful Nari session setup. Audio streams as
16 kHz mono PCM16, with bounded in-memory buffering and anti-alias resampling
from the device rate. It is not recorded to disk. Nari may receive audio before
you press Stop, unlike the former Groq batch path.

Audio, pending requests, and interim transcripts are not saved in session history
or hot-reload snapshots. Only the final, editable draft persists normally.
Cancel, panel close, or reload cancels capture and discards pending results.
Cancellation cannot retract audio already received by Nari. Failed requests are
not automatically retried or sent to a different transcription provider.

Capture is bounded to five minutes. Stop flushes the final audio before committing
and waits for the matching acknowledgement and outstanding final utterances.
On macOS, grant microphone access when prompted. Linux needs a supported default
ALSA/PipeWire/PulseAudio input. Windows uses its default input device.
Offline screenshots and previews never access a microphone or Nari.

The shared Jcode subscription/Groq batch API remains available to other clients.
Changing Desktop's voice provider does not deploy or modify that hosted service.

## Verification

See [Nari voice validation](nari-voice-validation.md) for measured native-provider
latency, requirement-to-check evidence, and remaining hardware limitations. Long
silent stretches can produce stray provider text, so review the draft before sending.
