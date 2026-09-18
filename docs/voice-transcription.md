# Subscription voice dictation

Choose **Voice** in a session's account/model toolbar, speak, then choose **Stop**.
The result is appended to the current draft. Review and edit it before sending.
Typing and attachments are preserved, and the insertion can be undone normally.
Voice capture is local even when the session runs over SSH.

Voice checks the signed-in local Jcode account's server-provided
`capabilities.voice_transcription` entitlement before opening the microphone.
No separate Groq API key is needed in Desktop. Older/unconfigured servers and
accounts without the capability fail closed, with a message in the composer.
Sign in using Accounts. Model-provider accounts alone do not grant this capability.

Audio is held in memory, then sent through the Jcode subscription service to
Groq's `whisper-large-v3-turbo`. Audio and unfinished recording state are not
saved in session history or hot-reload snapshots. A completed transcript is a
normal editable draft. Recording is explicit, never automatic. Cancel, panel
close, or application reload stops capture and discards pending results. Canceling
an upload cannot retract audio already received by the service.

A recording ends at five minutes or 10 MiB, whichever comes first. The backend
allows 10 attempts per minute and 100 per day per paid account, including failed
attempts. It does not charge the account's inference credit balance for voice.
On macOS, allow microphone access when prompted. Linux needs a supported default
ALSA/PipeWire/PulseAudio input. Windows uses the default input device.
Offline screenshots/previews never access a microphone or the voice service.

## Rollout

Desktop enables the shared `jcode-base` `voice-capture` feature. The API changes
live in `solosystems-backend/workers/api`. Deploy that service with its voice
migration and server-side `GROQ_API_KEY` secret before enabling production use.
Historical subscription records must acquire verified Stripe subscription status
as described in the backend rollout guide. Do not copy a user's personal Groq key
into the service or distribute a server key in Desktop builds.

This checkout's implementation has local automated coverage. Production activation
requires access to the subscription Cloudflare account and a service-owned Groq
credential. A live Groq transcription is not implied by local tests.

## Verification (2026-09-18)

- Backend: 234 tests passed, including 18 voice endpoint tests with signed
  subscription/invoice webhook events, real SQLite state, and mocked Groq.
- Shared client/capture: 13 tests passed, including HTTP contract checks and
  stop/cancel/drop ownership tests without opening a microphone.
- Desktop: panel tests cover voice draft insertion, undo, cancellation, offline
  isolation, and narrow-window controls. Packaged microphone metadata has two
  passing checks. Real app screenshots run on private Xvfb displays.
- The full dirty-checkout UI suite is not green. The preserved pre-voice test
  binary also has failures in unrelated workspace motion and changelog work.
  No claim is made that the full checkout passes.
- Live microphone capture, macOS/Windows permissions, and production Groq
  transcription still need end-to-end validation in an enabled deployment.
