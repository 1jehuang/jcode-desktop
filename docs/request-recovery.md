# Desktop request recovery

Desktop recovery is split between the shared SDK (`jcode-sdk::turn_retry`), the
GPUI-free session worker, and the panel lifecycle. These layers must agree on
when a turn is running, waiting to retry, cancelled, or permanently stopped.

## Recovery invariants

| Situation | Desktop behavior | Regression coverage |
| --- | --- | --- |
| Transient provider/network failure | Bounded automatic continuation with backoff. Continue with a hidden system reminder instead of duplicating the original prompt. | SDK `sdk_tests/turn_retry.rs`, harness `harness_turn_retry_tests.rs` |
| Rate limit | Honor exact nested JSON `retry_after_seconds` and supported textual units. Ignore sibling account limits and malformed structured values. Long waits are left for manual recovery. | SDK structured-rate-limit and day-hint tests |
| Invalid request, credentials, exhausted quota, or failover offer | Do not automatically resend an identical deterministic failure or cross a provider-switch consent boundary. | SDK permanent-failure and failover tests |
| Retry budget exhausted | Stop automatic recovery and explain how to continue manually. An explicit native Resume action sends a fresh continuation, never the original payload, and preserves the draft and queued-user priority. | Harness exhaustion socket test and panel recovery click tests |
| User cancels during retry | Disarm pending continuation immediately. Buffered failure events from the cancelled request cannot restart it. | SDK cancellation tests and harness late-error socket test |
| Countdown cancelled or real output starts | Settle the retry spinner without incorrectly ending a live response. Refresh countdown labels while waiting. | Panel activity-state tests and harness countdown test |
| Queued follow-ups after failed turn | Pause after failure. Resume when automatic continuation actually starts, including streaming without `MessageAccepted`. | `panel_queue_tests.rs` |
| Initial history fetch fails | Retry read-only history loading with a bounded watchdog. Successful or unsolicited history cancels the watchdog. Exhaustion explains that live updates remain connected. | `harness_history.rs`, `harness_history_recovery_tests.rs` |
| Reconnecting remote worker receives Stop | Wake immediately rather than sleeping through the reconnect delay. | Remote reconnect socket tests |
| Malformed or oversized protocol frame | Retain the SDK disconnect cause and stop automatic reconnection. Bound each NDJSON line at the TUI's 256 MiB ceiling. A truncated line from a dropped transport stays reconnectable, including a partial UTF-8 character. | Shared API framing tests, SDK socket tests, harness protocol recovery tests |
| Provider-switch offer | Explain the proposed provider and estimated resend size. Opening model choices is not consent to switch, and choosing a route does not silently replay a failed request. | Panel failover click and clipboard tests |
| Cancel/background control request fails | Report the failure instead of silently claiming success. Cancellation still disarms local automatic retries. | Session worker control paths |

## Important boundaries

- Reconnecting a transport is not permission to replay an ambiguously delivered
  prompt. Session recovery directives and existing acceptance/busy reconciliation
  own that decision.
- Crash continuation is server-directed. A cached crash badge or a dropped socket
  alone does not authorize another model request.
- A connection loss is provisional. It must not mark unacknowledged input as
  accepted, erase its transcript text, or turn into a confirmed runtime crash.
- SDK calls on the session worker are currently synchronous and bounded by their
  configured request timeout (30 seconds by default). The reconnect delay is
  interruptible, but a command cannot preempt a synchronous request already in
  flight. Fully asynchronous bootstrap/control requests are separate work.
- Desktop does not copy every TUI feature. In particular, the TUI's OS-network
  change listener/probe wait, long rate-limit holding policy, completion-confidence
  gates, and automatic resend after accepting a cross-provider fallback are not
  equivalent to the desktop's bounded retry policy and manual route choice.
  Do not describe these as complete parity.

## Validation

Use `justrust test -p jcode-sdk` from the adjacent Jcode checkout, followed by
`justrust test -p jcode-desktop-harness` and `justrust test --workspace` here.
Visual inspection uses the private Xvfb fixture script, not the active desktop.
The running development application receives verified changes through its
Ctrl+R rebuild-and-reload path (also exposed by `--reload-ui`). A compiled but
not loaded plugin does not establish runtime delivery.
