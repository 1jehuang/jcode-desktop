# Cloud session latency: end-to-end recheck

Date: 2026-09-19, approximately 06:31–06:33 UTC.

**The few-hundred-millisecond result applies to eligible immediate reuse, not every new panel on a running VM.** A fresh native recheck reproduced a multi-second delay after the 30-second readiness cache expired, even while earlier cloud panels and their shared SSH master remained connected.

## Measured current behavior

The actual release executable was run on a private Xvfb display with isolated Desktop state, the installed cloud helper, and the real configured cloud VM. There was no fixture, mock server, application rebuild, VM restart, or change to cloud safety policy. The VM was already running before the first click.

Session readiness means native Connect dispatch through remote session creation and the real History update (`history_loaded=true`). It is not just panel appearance or a session-list response.

| Native action | Editable pending panel observed | Session and history ready | Additional work |
| --- | ---: | ---: | --- |
| First connection in this Desktop instance | 18 ms | 9.137 s | One readiness helper call, one SSH master and one Desktop SSM proxy |
| Second connection, clicked 4.367 s after first readiness | 19 ms | 389 ms | One independent API channel on the existing master, no readiness helper call or new proxy |
| Third connection after a deliberate 35.2 s pause | 21 ms | 5.587 s | Another readiness helper call, but still the same master and no new Desktop proxy |

All three sessions had distinct remote session IDs on the same configured VM. Earlier sessions remained attached with their identities and loaded history preserved. These are observations from one current three-connection run, not latency percentiles or a general SLA.

After timing, a normal native Enter submission on the first panel produced the real Nova Micro reply `CLOUD_VERIFY_OK`, cleared the composer, and saved both the user message and assistant response. Their remote journal timestamps were 1.033 seconds apart. The screenshot and saved journal independently confirmed the response, with no tools, account changes, or credential transfer. The journal read used one additional shared SSH channel after all connection timings. All test-owned panels were then closed natively, their channels/master disappeared, and no isolated test processes remained. The existing VM was not stopped.

Executable SHA-256: `1b83e2a2ad5dd15729764667861c0968bfceaa868d7d4cf8fe10cb571d87102f`.
Local detailed artifacts: `target/warm-transport-20260919/fresh-r3/result.json`, native screenshots, transport invocation logs and navigation timeline. Earlier harness attempts with a too-long isolated socket path or incorrect post-insertion click coordinates were excluded from these measurements, not reported as application failures.

## Why a running VM can still take seconds

There are two separate reuse mechanisms:

1. **VM readiness evidence:** `workspace_cloud_alpha.rs` retains a successful check for 30 seconds. Reusing it does not extend its deadline. Expiry makes the foreground Connect path run `wake` again, even when the VM is running. This helper validates account identity, guard health, allowance and host ownership/state, then probes SSH/bootstrap readiness. It does not start another VM when the existing VM is running.
2. **Authenticated SSH transport:** `harness_transport.rs` reuses a private master while its owners remain live, creating an independent API connection for each panel. This reuse worked on both the second and third requests. It cannot remove work that occurs before the API connection is requested.

The third measurement distinguishes those mechanisms: helper calls increased, while the Desktop master/proxy counts did not. Helper-internal bootstrap SSH is separate from the Desktop transport counters.

## What is and is not resolved

Transport reuse is working. The remaining foreground readiness recheck is **not resolved** by that optimization, so an unqualified claim that subsequent sessions always take about 380 ms is incorrect. Closing the last owning panel, starting a new Desktop instance, changing connection identity, or rotating a capacity shard can also require another transport setup.

This verification does not extend the readiness TTL, disable guard/allowance checks, or periodically call `wake` in the background. Periodic `wake` could restart a VM intentionally stopped by a guard or idle shutdown. A safe future optimization must preserve current safety checks and avoid unintended wakeups, for example a separately designed non-starting readiness validation path. Existing read-only `status` and transport liveness alone do not perform the full guard validation.

The personal alpha still uses the workstation operator's AWS/SSH setup. This test does not establish a managed customer-account cloud service or validate the user's previously closed Desktop instance.
