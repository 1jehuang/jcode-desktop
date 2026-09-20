# Response stop reasons

Desktop shows a transcript notice for abnormal response endings, without adding one for natural completion.

- `TurnStopped` distinguishes interruption, runtime failure, a caught session panic, provider guardrail, limit reached, and unknown future reasons. Details and any provider stop code remain visible. Failure details retain the native account/model recovery controls and copy action.
- Escape and `/cancel` immediately show that the user requested a stop. A confirmed cancellation replaces that provisional notice.
- Losing an active session connection reports **response outcome unknown**, not a confirmed crash. An idle disconnection does not add a response-stop notice.
- Partial text and reasoning are retained. Unfinished tool rows stop spinning without claiming successful completion. Detached background tasks remain independent.
- Abnormal stops pause queued follow-ups and automatic continuation. A following legacy `error` or `turn_done` does not duplicate the same failure or play a success sound.
- Remote events retain the correct SSH session namespace.

## Compatibility

Detailed reasons require the runtime and harness bridge supporting the SDK's `turn_stop_reasons` capability. Older runtimes still get local cancellation, recognized terminal-status, error, and transport-loss handling. These structured events are live notifications, not persisted history: reopening a session cannot reconstruct an earlier precise reason. A dead transport alone never proves that the agent crashed.

The daemon and bridge must be updated separately from Desktop's state-preserving UI reload. UI work does not restart the user's runtime or terminate their active sessions.

## Verification

Focused GPUI tests cover each reason, actual Escape handling, natural completion, legacy error deduplication, queued-prompt preservation, legacy statuses, active/idle connection loss, and rendered notices with no activity spinners. Bridge and sound-policy tests cover routing, worker activity, and abnormal-stop sound suppression.

Offline visual checks use the real application on private Xvfb:

```sh
python3 scripts/screenshot.py target/stop-reason-crash.png --preview-state crashed
python3 scripts/screenshot.py target/stop-reason-interrupted.png --preview-state interrupted
python3 scripts/screenshot.py target/stop-reason-crash-narrow.png --preview-state crashed --size 640x700
```
