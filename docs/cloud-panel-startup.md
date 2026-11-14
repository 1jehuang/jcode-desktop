# Managed cloud panel startup

Desktop keeps cloud session creation separate from cloud machine provisioning.
Each panel still gets a fresh API client and a distinct session. Attached clients
are never reused for a different panel.

On Unix platforms, `RemoteTransports` now reuses a live account-authorized SSH
master. After a successful connection it also prepares one unattached API stream
in the background, over that master. The next panel consumes that stream once,
then prepares its replacement. Preparation does not create a session, request a
model response, or provision another machine.

## Bounds and invalidation

- At most one ready spare and one preparation can belong to the current cache
  generation. Preparation happens outside the cloud mutex.
- A ready spare expires after 60 seconds. A weak-reference timer drops it even
  if the user never opens another panel. The cache otherwise holds weak master
  ownership, so it does not retain idle masters indefinitely.
- Every foreground selection verifies the exact hosted account and API base,
  plus a process-local fingerprint of the synchronized model settings and
  credentials. Changed inputs invalidate the spare and master selection.
- Generation checks reject preparation completed after invalidation or master
  replacement. A known disconnected spare is not consumed.
- Each master has a conservative cumulative budget of eight API channels,
  including background preparation. Consuming an already prepared stream does
  not reserve another channel.
- Failed best-effort credential synchronization does not publish a reusable
  master. Foreground errors do not automatically retry ambiguous operations.

Windows retains its existing isolated SSH connection path.

## Verification, November 13, 2026 (Pacific time)

The ignored acceptance test
`managed_cloud_live_warm_panels_use_distinct_sessions` was built on the hosted
builder and executed locally against the signed-in account, without transferring
local credentials to the builder. It created two distinct sessions and sent no
model prompt. It waits for actual spare readiness, not a fixed sleep.

Observed warm-host measurements in that test:

| Path | API transport | Session ready |
| --- | ---: | ---: |
| Reused SSH master, before API preconnection | 2.860 s | 2.993 s |
| Ready preconnected API stream | 12.7 ms | 133.4 ms |

These are one-machine observations, not a latency guarantee or cold-VM boot
measurement. The test bypasses production credential synchronization and
per-session default-model application, as other offline harness tests do.
Production still applies local model defaults to every newly created session.

Deterministic tests cover account/configuration changes, channel capacity,
expired owners, one-shot spare consumption, timer expiry, stale preparation,
and failed preparation. Full workspace validation passed 1,928 tests, with 14
manual/live tests ignored.

The running development host accepted the feature as UI generation 6 using its
unchanged shared API crate version. The packaged 0.7.0 release uses consistent
0.7.0 package versions and requires the normal new-host launch for its new shared
crate identity. The running host was not restarted or its ABI guard weakened.
