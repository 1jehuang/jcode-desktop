# Voice routing and the Jev decision card

Start/stop local voice with Copilot, Ctrl+Shift+V, or the Voice button. Nari streams the transcription. After the final transcript arrives, Jev distinguishes coding-agent work from bounded immediate UI navigation.

- Questions, coding instructions, discussion, ordinary dictation, and mixed coding/navigation requests go to the recording's conversation. Only the spoken utterance is sent, or queued while the agent is busy. Existing typed text and attachments are not submitted.
- Explicit standalone requests can create a new empty conversation, switch to the next/previous conversation, or open an existing matching session.
- Session matching uses the **20 most recent non-archived sessions** in the Desktop catalogue at recording start, sorted by last interaction rather than pinned order. Pending drafts are excluded and IDs deduplicated before the cap. There is no older-session search or transcript retrieval.
- Jev receives the spoken transcript and candidate titles, friendly names, and working directories. Session IDs remain local.
- Uncertain/unmatched requests, invalid responses, unavailable credentials, and provider failures leave the transcript in the original editable draft with an explanatory message. Existing typed text is preserved.
- Cancel, a newer recording, or closing the source panel prevents a late result from changing the workspace. Audio and pending transcription/routing evidence are not persisted in UI snapshots.

## Visible questions and decisions

The recording waveform remains a small bottom-of-window pill. Once the final transcript is ready, a bounded, scrollable **Jev decision card** replaces it without changing the composer layout. The card stays until dismissed or a new recording begins.

It shows the transcript, the actual question-specific instructions, the possible routes/candidate matches, each returned probability of Yes, and the resulting action or error. During routing, unanswered questions show `Waiting…`. Errors never invent answers. Expand **full questions and candidates** to inspect the exact complete instructions, Yes/No criteria, and the offered session metadata. Navigation carries the evidence to the destination conversation.

These are independent **Noul** questions, not a multiple-choice distribution. Percentages do not add up to 100%, and they are not Jev Choice's separate confidence field. Preview fixtures are explicitly labelled `Preview` and use deterministic example answers.

## Question bounds and safety

There are seven fixed questions (`coding_agent`, `navigation`, `quick_action`, `new_session`, `next_session`, `previous_session`, `uncertain`) plus one question per candidate. At 20 candidates this is 27 questions.

The shared client bounds each request to 24 questions. The classifier therefore sends bounded batches of at most 24 questions, each with the identical complete transcript/candidate state. **All batches must succeed and all answers must validate before any routing decision is returned.** No candidate is dropped and no partial answer authorizes an action. This fixes the former 25–27-question local rejection for 18–20 candidates without weakening the transport bound.

Coding-agent probability >= 0.8 takes precedence. Immediate actions require both the relevant intent-family and selected action/match probabilities >= 0.8, with every competing question <= 0.2. Otherwise the utterance stays in the draft.

## Provider configuration

Classification reuses Jcode's shared Jev client, credentials, provider selection, entitlement checks, timeouts, and bounded responses. `agents.memory_jev_provider` or `JCODE_MEMORY_JEV_PROVIDER` selects the route. No separate Desktop credential store is introduced. A provider failure never silently switches accounts or spends another provider's balance.

HTTP 402 means the selected provider's credits or account spending limit are exhausted. This is distinct from local question validation. Restore that account's access or explicitly configure another authorized provider, then retry. Desktop preserves the transcript on this failure.

## Verification

`cargo test -p jcode-desktop-ui --lib voice` covers routing evidence, draft preservation, cancellation/stale results, navigation, full-20-candidate card bounds, expansion/dismissal, and composer stability. Shared classifier and transport tests live in `/home/jeremy/jcode/crates/jcode-base/src/voice_intent.rs` and `jev.rs`, including real local HTTP mocks that exercise the 24/3 split and failures in a later batch. Opt-in live Jev tests require working provider access.

Offline screenshots:

```sh
python3 scripts/screenshot.py target/voice-routing.png --preview-state voice-routing
python3 scripts/screenshot.py target/voice-decision.png --preview-state voice-coding-agent
```

These render the real UI with offline fixtures. They do not capture microphone audio or prove live speech-recognition accuracy.
