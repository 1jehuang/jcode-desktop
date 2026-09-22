# Direct TypeSafe voice routing

Desktop's final transcription enters `voice_intent::classify_with_report` in the
shared `jcode-base` crate. Voice has its own `JevClient::for_voice()` route and no
longer inherits the memory provider setting.

## Authentication and routing

- `JCODE_VOICE_JEV_PROVIDER=auto` selects an existing Jcode account credential,
  otherwise a TypeSafe credential. It never selects OpenRouter or AI/ML API.
- `JCODE_VOICE_JEV_PROVIDER=typesafe` explicitly selects the direct
  `https://api.typesafe.ai/v1/systemone` endpoint with model `jev-latest`.
- TypeSafe authentication uses `TYPESAFE_API_KEY` or `typesafe.env` in Jcode's
  config directory. Keep the file private (0600), outside the repository.
- `JCODE_VOICE_JEV_PROVIDER=jcode` uses the authenticated account gateway. Its
  existing `memory_jev` entitlement protects the typed Noul request contract.
  The production gateway is configured to call TypeSafe directly.
- Authentication, entitlement, billing, or transport errors never retry against
  another provider or account. A server-owned key must not be copied into a
  Desktop installation. Use a separate local key for direct access.

Pure UI commands are not coding work merely because they mention a session or
because a candidate title contains a coding topic. For mixed navigation and coding,
the prompt still instructs Jev to prefer the coding agent. Selection chooses the highest
validated concrete-outcome score without a confidence floor, competing-score
ceiling, or action-family gate. `uncertain` competes as its own outcome. Failed
requests and winning uncertain outcomes keep the transcript in the draft rather
than executing an action. See [the complete action catalog](jev-actions.md).

## Live acceptance

```sh
cargo test -p jcode-desktop-ui live_voice_transcript_to_jev_to_panel \
  -- --ignored --nocapture --test-threads=1
```

This opt-in test makes real inference requests using the same credential lookup
and asynchronous routing code as Desktop. It supplies synthetic text at the
production final-transcription boundary, then checks the actual UI completion:

1. Open an offered conversation by its title.
2. Request a new conversation through the bounded quick-action event.
3. Route a mixed navigation/coding request to the coding agent, not navigation.
4. Retain all returned question probabilities and preserve the existing draft.
5. Emit only one intended action or coding submission.

The bridge records outgoing messages rather than contacting a coding daemon.
No real conversation, physical microphone, native window, or user's draft is
changed by this test. It validates transcription-result through live Jev through
UI/action dispatch, not audio capture or Nari transcription.

Offline visual check:

```sh
python3 scripts/screenshot.py target/voice-direct-api-review.png \
  --preview-state voice-coding-agent --panels 1 --layout-mode folder_tabs
```

The screenshot uses fixture probabilities, not a live inference response. Keep
that visual check distinct from the live acceptance above.

## Verification on 2026-09-22

- Shared transport and voice regression suites: 50 passed.
- Desktop voice regression suite: 76 passed, opt-in network test excluded.
- Shared live fixtures: three consecutive passes of 18 classifications each,
  including batched 20-candidate requests, mixed coding/navigation, negation,
  unsupported commands, and ambiguous or unmatched navigation.
- Desktop live acceptance: two consecutive passes of all three workflows,
  with nine validated answers per request and exact action/draft assertions.
- The selected provider was `typesafe`, model `jev-latest`, with ambient memory
  configuration unchanged and the existing OpenRouter credential still present.
- A separate local voice API key was stored outside the repository with mode
  0600. The backend key, subscription entitlements, and billing were not changed.
