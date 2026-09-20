# Voice session navigation

Start and stop voice with Copilot, Ctrl+Shift+V, or the Voice button. Nari transcribes the recording. Once the final transcript arrives, Jev distinguishes normal dictation from a request to open an existing Jcode session.

- “Open my conversation about database migrations” opens or focuses the matching session.
- Ordinary instructions, including instructions about implementing session switching, remain an editable draft. Nothing is submitted automatically.
- Candidate sessions are the **20 most recent non-archived sessions** in the Desktop session catalogue at recording start, sorted by last interaction rather than pinned order. Pending drafts are excluded and IDs deduplicated before the cap. No older session search or transcript retrieval is performed for voice matching.
- Jev receives the spoken transcript and those candidates' titles, friendly session names and working directories. Session IDs remain local. Shared `jcode_base::voice_intent` also rejects inputs with more than 20 candidates.
- Navigation requires confident explicit navigation intent and exactly one confident match. Ambiguous or unmatched requests, invalid responses and unavailable credentials leave the transcript in the original draft with an explanatory message. Existing typed text is preserved.
- Cancel, a newer recording, or closing the source panel prevents a late result from changing the workspace. Voice audio and pending transcript/routing state are not persisted in UI snapshots.

## Provider configuration

Classification reuses Jcode's shared Jev client and existing credentials, provider selection, entitlement checks, timeouts and bounded responses. The `agents.memory_jev_provider` setting or `JCODE_MEMORY_JEV_PROVIDER` override selects the route. No separate Desktop credential store is introduced. A provider failure never silently switches accounts or spends another provider's balance.

## Verification

`cargo test -p jcode-desktop-ui --lib voice` covers the exact 20-session boundary, pinned ordering, existing-panel reuse, history opening, draft preservation, cancellation and stale results. Shared classifier tests live in `jcode-base/src/voice_intent.rs`, including an opt-in live Jev fixture for navigation, dictation, negation, ambiguous and unmatched requests.

Offline visual inspection uses `python3 scripts/screenshot.py target/voice-review.png`. It does not capture microphone audio or prove speech recognition accuracy.
