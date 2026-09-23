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

### Silent reconnect failure in older daemons

The presence of stop-reason support alone does not prove every interrupted turn
can emit a reason. Runtime `1d8388635` could drop a busy turn when Desktop attached
a successor connection and then retired the original connection. The log said
`Skipping destructive disconnect cleanup ... because another client is still attached`,
but the old lifecycle still lost its active turn. No structured stop event reached
the successor, and durable history could remain `Active`.

Runtime fix `ada84e8fc` retains the busy turn owner and completion receiver for the
successor. A UI reload should preserve that response rather than manufacture an
interruption notice. Updating only Desktop's UI cannot activate this daemon fix.

On 2026-09-21, the shared daemon was gracefully updated from `1d8388635` to
`697038bd9` using the immutable build `697038bd9-dirty-e64de54995f1`. Both the
server registry and the running process executable confirmed the new build after
handoff. The initiating Desktop session automatically resumed through the
runtime's recovery directive. This is deployment evidence, not merely a reload
request acknowledgement.

Before deployment, all ten isolated server disconnect tests passed:

```sh
cd /home/jeremy/jcode
cargo test --profile selfdev --test e2e disconnect:: -- --test-threads=1
```

In particular,
`desktop_busy_owner_disconnect_with_successor_finishes_original_turn` attaches a
successor while the provider is streaming, drops the original connection, and
checks that the successor receives the remaining output and completion. It also
checks that the provider is called only once, the session is no longer busy, and
the old connection does not close or crash it. These tests use an isolated real
server with a deterministic provider, not the user's live conversations.

This does not retroactively restore missing stop reasons in old history, and it
does not prove an end-to-end live provider/bridge/UI test of every stop category.

## Verification

Focused GPUI tests cover each reason, actual Escape handling, natural completion, legacy error deduplication, queued-prompt preservation, legacy statuses, active/idle connection loss, and rendered notices with no activity spinners. Bridge and sound-policy tests cover routing, worker activity, and abnormal-stop sound suppression.

Offline visual checks use the real application on private Xvfb:

```sh
python3 scripts/screenshot.py target/stop-reason-crash.png --preview-state crashed
python3 scripts/screenshot.py target/stop-reason-interrupted.png --preview-state interrupted
python3 scripts/screenshot.py target/stop-reason-crash-narrow.png --preview-state crashed --size 640x700
```
