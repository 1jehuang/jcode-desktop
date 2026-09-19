# Desktop auto-poke

Desktop now continues a locally submitted request when a successful turn ends
with actionable todos. It uses the shared Jcode continuation prompt and honors
`[features] auto_poke` in the Jcode configuration (default enabled), including
the shared configuration's environment overrides.

The CLI and TUI already implemented auto-poke. Desktop previously only drained
explicitly queued user prompts on completion. Desktop's implementation handles
incomplete todos, not the TUI's additional completion-confidence/gate checks or
its per-session `/poke` commands and keyboard toggle.

Safety rules:

- Only todos produced after the current local user request qualify. Merely
  opening a session, loading history, or restoring a window does not arm it.
- Only `pending` and `in_progress` todos without `blocked_by` entries qualify.
  Completed, cancelled, blocked, waiting, and unknown states are not retried.
- Explicitly queued user messages take priority. Automatic continuation never
  steers into an active turn or a pending submission.
- Stop/cancel, errors, failed sends, and unavailable connections suppress
  continuation. A new explicit user submission can start a fresh cycle.
- A given remaining-work state is poked only once per user request, including
  cycles back to previously seen states. There is also an eight-follow-up cap.
- Auto-pokes appear in the transcript using the shared core prompt text. They
  preserve the original cycle budget rather than counting as new user requests.

Tests live in `panel_auto_poke_tests.rs` and cover the policy plus the real
Panel completion/queue integration using GPUI's headless test context.
