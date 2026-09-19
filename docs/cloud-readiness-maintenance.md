# Maintaining cloud readiness without hidden wakes

The earlier [latency recheck](cloud-session-latency.md) reproduced a 5.587-second delayed panel despite a live shared SSH connection. Transport reuse alone did not avoid the expired foreground VM check.

## Updated design

- An explicit cloud Connect runs `wake-ready`. This retains the full wake and bootstrap checks, then obtains a fresh safety receipt. Account identity can overlap other read-only AWS checks, but no start or SSH attempt happens before the safety checks pass.
- A receipt identifies the exact instance and boot, carries a bounded validity period of at most 30 seconds, and cannot outlive the guard heartbeat, remaining allowance or two-hour lease.
- While an actually connected cloud panel remains open, Desktop periodically calls `check-ready` in the background. This action only reads AWS identity, guard, ledger and instance state. It never starts or stops the VM, opens SSH, synchronizes credentials, or extends a lease.
- A successful check can replace readiness evidence only for the same boot and local configuration. Closed/disconnected panels, configuration changes, newer connection failures, stale replies and unsafe receipts cannot revive old readiness.
- New panels reuse a fresh receipt and the existing private SSH master, while retaining their own independent API connections. Using a receipt does not refresh its age.
- If evidence expires or validation fails, the next explicit Connect performs the normal full checks. There is no unsafe local fallback or promise of subsecond readiness during authentication, network or control-plane failures.
- Closing the last connected cloud panel stops background readiness maintenance. The existing private-master ownership rules still release the transport when its last owner closes.

## Cold-start visibility

The startup checklist now shows measured total elapsed time and time on the current step. A timer never marks a step complete. A stopped or pending VM stays on the VM boot step until the SSH/bootstrap probe succeeds. Completed and failed attempts freeze their elapsed timer.

A real VM boot and initial SSH/SSM setup still take time. Parallel read-only checks reduce avoidable serialized work, but this design does not claim to eliminate boot time or the separate initial bootstrap probe. Cold-start improvements must be reported from measurements, not inferred from a successful build.

## Scope and safety

The personal alpha remains an operator-configured AWS/SSH integration, not a managed customer-account cloud service. Unfinished model-credential synchronization is not enabled by this change: a helper must explicitly advertise support before Desktop can call that optional action. The current helper does not advertise it.

Validation must include delayed connections beyond the original 30-second window, normal prompt submission, loss/closure and late-result races, malformed or expired receipts, guard/allowance refusal, and helper process cleanup. Live AWS results require valid operator authentication and must be recorded separately from offline tests.

## Verification on September 19, 2026

| Requirement | Check and observed result |
| --- | --- |
| Avoid repeated foreground validation after 35/65 seconds | `cloud_background_checks_maintain_proof_beyond_35_and_65_seconds` passes with injected time and fresh same-boot receipts. Real latency measurement is still blocked on AWS authentication. This is not a measured subsecond guarantee. |
| Never wake or extend a lease in the background | Helper `check-ready` tests reject non-running instances, exhausted allowance, stale guards and invalid leases. All six calls are read-only. Tests verify overlapping reads and no start or SSH side effects. |
| Do not revive stale, disconnected or closed sessions | Tests cover connection-authority filtering, last-panel closure, late replies, new wakes, boot/config changes, transport failure and stale status arriving during cold startup. All passed. |
| Keep expiry safe across delay or suspend | Receipt parsing rejects invalid/future/expired data, and both monotonic and wall-clock expiry tests pass, including rollback and suspend. |
| Preserve independent panels and normal submission | Existing remote tests (23), host tests (40) and harness tests (60 passed, one ignored) pass. A new real provider response remains an authentication-gated acceptance step. |
| Explain cold startup honestly | Private-display loading, failure and 800×600 light-theme renders were inspected. Loading shows `2s elapsed · 2s on this step`; failure freezes its elapsed timer. Tests prove timers never complete a step and AWS access is not marked complete before identity succeeds. |
| Clean up helper subprocesses safely | Tests cover timeout, inherited output pipes, an exited but unreaped leader, and unrelated process-group survival. All passed. |

The scoped helper suite passed 39 tests. The task-only patch also compiled independently on an isolated archive, with all 45 cloud tests passing. The complete working-tree UI suite reported 1,152 passed, eight failed and nine ignored. An isolated archive of the pre-change `07ade90` baseline reproduced exactly the same eight failures (1,130 passed, eight failed, nine ignored): one obsolete sidebar selector assertion, one unfinished-card entity-borrow panic, and six pull/swipe expectations. These are not reported as passing or repaired by this cloud change.

The paired release build succeeded. No running Desktop host was available for hot reload, so the rebuilt application is ready for the next launch, not verified as active in a live window.

Real first/immediate/35-second/65-second connection and provider-response acceptance is **not complete**. AWS returned an expired-session error, and the browser login attempt timed out without authentication. No new VM start, provider request or credential synchronization was performed for this verification. Resume those native checks after renewing `jcode-personal`; do not substitute the older warm-cache measurements for validation of this implementation.
