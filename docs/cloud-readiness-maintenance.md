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
