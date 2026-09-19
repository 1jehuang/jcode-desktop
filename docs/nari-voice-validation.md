# Nari desktop voice acceptance (2026-09-18)

This change replaces Desktop's stop-then-upload Groq dictation with native Nari
streaming and adds an explicit global Copilot-key route through the Desktop host.
The standalone Voice Lab is not needed at runtime. No hosted subscription service
was deployed and no Jcode agent daemon restart is required.

## Native provider measurements

Three authenticated calls through the new Rust `NariSession` API used Nari's
public 2.48-second mono16k PCM sample. Every run produced a live revision before
EOF and the final transcript **“Hello. Welcome to Nari Labs.”**

| Run | Connection setup | First revision from audio start | Final after EOF |
| --- | ---: | ---: | ---: |
| 1 | 356 ms | 1940 ms | 34 ms |
| 2 | 255 ms | 1783 ms | 42 ms |
| 3 | 236 ms | 1764 ms | 31 ms |

These are short-sample observations, not broad accuracy or latency benchmarks.
Final-after-EOF is not first-word latency. Nari is already processing while the
speaker is talking. The retained `nari_pcm` example requires explicit `--live`,
loads normal private credentials, bounds file size, and reports timings without
printing audio, transcripts, or keys.

A 38.48-second public-audio/silence/public-audio stream exercised duration
commit boundaries: 12 revisions, both sample phrases retained, final 32 ms after
EOF. This is a **protocol success, not an accuracy pass**. During the inserted
33.52 seconds of silence, the provider added “I'm not sure.” The client preserves
provider text rather than silently censoring it. Review dictated text before
sending, especially after long pauses. No untested silence filter was added.

## Requirement-to-check map

| Requirement / changed output | Check and observed result |
| --- | --- |
| Native Nari, independent of Voice Lab | Three real authenticated Rust API streams succeeded. The normal production Desktop also passed the complete CPAL → resampler → Nari → live preview → final draft flow with isolated public-audio input. No loopback prototype dependency. |
| Stream while speaking and finalize on Stop | Actual pre-EOF API revisions and a visible native live partial before the production CLI Stop. The correct complete phrase entered the draft after Stop. WS tests additionally cover byte order, revision/final protection, duration commits, and matching acknowledgement. |
| Bounded native capture and cancellation | Shared client suite passes with capture enabled (33 tests) and disabled (19 tests). Fake capture factory tests cover startup gating/cancellation and worker lifecycle without physical microphone access. Independent DSP review exercised output counts and spectral attenuation. |
| Keep drafts editable and never send automatically | Desktop panel tests exercise live preview replacement separately from typed draft, final append, undo, cancellation, error preservation, and unchanged conversation items. All 19 panel/workspace voice tests pass, including actual fallback-key and button routing to an existing recording owner in another chat. |
| Keep interim audio/text out of recovery | Panel snapshot test verifies no live transcript field/content. Voice state is not serialized. Last-target chat identity alone may persist. |
| Global Copilot ownership, target selection, repeat protection | Native host/workspace tests and production CLI acceptance recorded below. Raw in-app effective XF86Assistant and compositor raw F23 are intentionally distinguished. |
| Private credential setup | Existing dedicated Nari credential transferred directly to local `nari.env`, with mode 0600 verified. No key value entered chat, source, test evidence, or Git. No credits purchased. |
| Usable running build | Release activation and local compositor configuration recorded below. Physical Copilot/microphone trial remains user-attended. |

## Desktop end-to-end evidence

The final desktop voice suite passes **19 tests**, covering preview replacement,
final append and undo, cancellation, snapshot privacy, chat eligibility, Copilot
repeat suppression, modal guards, remembered-chat selection, and shared recording
ownership through the actual Ctrl+Shift+V and Voice button event paths. Five host
IPC tests and one named-action host dispatch test also pass. Two Python packaging
metadata tests pass.

The production debug host passed `scripts/accept-voice.py` on a private Xvfb
and Openbox display. After typing an unsent draft and minimizing the window,
`--toggle-voice` reached the existing host and rendered the expected microphone-
disabled preview status. The same window and socket remained, and the draft was
unchanged. Calling it without a host failed safely without launching anything.
The resulting screenshot was inspected. This exercises real native IPC and UI
routing, but intentionally does not claim physical microphone acceptance.

The paired release build passed. The unchanged production release then passed
`python3 scripts/accept-voice.py target/voice-release-verified --binary
 target/voice-release-bundle/jcode-desktop` and the required `scripts/screenshot.py`
render. Both resulting 1440×1000 screenshots were inspected: Voice Copilot is
visible, the draft remains intact, and the expected offline status is readable.
Evidence is in `target/voice-release-verified/result.json`, `after.png`, and
`target/voice-release.png`. The result explicitly records no microphone use and
no live compositor test.

Two initial canonical acceptance attempts and one screenshot attempt timed out
before the first state dump. The same immutable executable later passed two
scratch diagnostic runs and both canonical checks without a production change.
The first-paint startup failure's cause is unconfirmed, not treated as fixed.
Those failures did not involve provider requests or microphone access.

The live Desktop host was upgraded during concurrent Desktop work. Its executable
SHA-256 matched the verified release (`bfa52657295e0718b305687e3e94859a2e2aa9895b331f2b2c013d916bce6655`).
The host log confirmed recovery and UI activation. The loaded UI library also
matched the current release library byte-for-byte and contained the named voice
route. No extra restart or unattended voice toggle was needed.

One `Super+Shift+F23` binding was added to this machine's compositor configuration,
with `repeat=false`, `allow-inhibiting=false`, and the normal Desktop launcher plus
`--toggle-voice`. A backup was retained outside the repository and a byte-for-byte
comparison confirmed all prior configuration remained. No compositor command or
keyboard monitoring was used. Registration and physical-key delivery remain
unverified until the user's intentional trial. The private credential file's
owner-only `0600` permissions were rechecked.

Shared backend commits: `84805d6b7`, `8d5953314`. Desktop implementation commits:
`c8da32f`, `c7d933d`, `779d37e`. The lock refresh adds no new package versions and
removes stale unused entries while recording the shared client's rustls dependency.

## Remaining limits

No unattended physical microphone recording or keyboard monitoring was performed.
The user had already accepted the browser prototype, but the actual hardware microphone and physical global key still require an intentional
user trial. The normal software capture-to-draft pipeline was exercised with a
real ALSA virtual device carrying only the public sample, as detailed below.
Wayland activation requests can be denied by compositor focus policy. The global
voice command can still reach the last chat even when Desktop is not foreground.
Linux is the exercised platform. macOS/Windows microphone permissions and native
global shortcut registration are not claimed tested by this change.


## Full native input-to-draft acceptance follow-through

`target/voice-native-cloud/result.json` records a successful **normal, non-preview
production Desktop** run through its actual public controls and integration
boundaries. The executable is the same immutable release hash recorded above.
The test did not substitute the provider, transport, CPAL capture, resampler,
voice state machine, or draft insertion logic.

1. A private Xvfb/Openbox display and bubblewrap sandbox ran the production host.
   The host had a private home/config/runtime, no shared daemon sockets, an empty
   application PATH, and no mounted `/dev/snd`. Physical recording was impossible.
2. The real `--toggle-voice` command first exercised the native missing-key error.
   The existing typed draft remained unchanged, and no audio/provider stream opened.
3. A process-only ALSA file/null device supplied the SHA-pinned official public
   sample. A regular file avoided the short-read problems observed with an input
   FIFO. Draining the output FIFO at the native rate paced the device. The exact
   mechanism had first passed an offline, network-disabled production CPAL probe:
   44.1 kHz float stereo to 16 kHz mono, 56,000 normalized samples in about
   3.492 seconds, 0.999467 waveform correlation, and bounded Drop cancellation.
4. Only the existing Nari-only key file was copied into the private config. One
   authenticated stream sent **6.7 seconds** of public sample plus bounded silence.
   The live screenshot shows **“Hello.”** below the still-unchanged typed draft.
5. Production CLI Stop was invoked 6.516 seconds after the source pump began.
   The final visible draft was exactly:
   **“Native dictation review: Hello. Welcome to Nari Labs.”**
   The prefix was preserved once, the same startup draft and host socket remained,
   and prompt history/queue showed no automatic sending.
6. The copied credential was deleted and its absence independently verified.
   The isolated app and display processes ended. No physical microphone or live
   desktop input was used. Both live and final screenshots were independently read.

The retained opt-in runner is `scripts/accept-voice-native.py`, with
`scripts/voice_virtual_alsa.py` providing the restricted public-audio device.
Cloud operation requires explicit arguments and has independent capture bounds.
Preparation-only mode uses no credentials and no provider requests. This test
validates the complete native software path, not a benchmark of human speech or
an assertion that physical keyboard/microphone registration has been tested.

The isolated X11 window initially remained unmapped on some attempts. Evidence
showed no `WM_STATE` and an empty window-manager client list while checkpointing
continued. The runner explicitly mapped/activated only its private test window
and records `private_x11_map_requested: true`. This was a test-environment
intervention, **not a claimed production fix**. No live compositor was controlled.
The visible elapsed counter includes source/readiness/inspection delays, so the
6.516-second stop timing is not reported as provider recognition latency.
