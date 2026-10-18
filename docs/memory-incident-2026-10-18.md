# Desktop memory incident investigation: 2026-10-18

## Evidence and limits

The supplied archive contains 15 UTF-8 text files (22,313,190 bytes expanded):
Desktop/server logs, macOS diagnostic reports, and runtime-memory JSONL.
It was inspected with `zipfile` in place, without extracting or executing its
contents. No absolute/traversal paths, symlinks, executable mode bits, encrypted
members, unexpected binary contents, or extreme expansion ratios were found.
This is an archive-structure/content inspection, not a malware-free guarantee.
Treat the logs as private: they contain session identifiers and local paths.

Archive SHA-256:
`28f025e26857ff20b47c0afbfa2202315fd884cf48975f70fc2ee8feabaecc9b`

Kernel logs confirm that macOS suspended Desktop processes at 11:35:27 and
11:46:32 IST on October 18 due to swap exhaustion. They do not identify the
retaining Rust allocation or establish the reported 90 GB figure. The first
recorded process lifetime before suspension is roughly 40 minutes, not seven.

The supplied client-memory counters are **TUI attribution, not native GPUI
Desktop attribution**. The October 18 client samples occur after the two
suspensions, so they cannot diagnose the affected Desktop heap. Likewise, the
included JetsamEvent predates the reported suspensions. High compression is
consistent with repeated data, but does not prove which structure retained it.

## Confirmed code-level risks

### Repeated model-catalog snapshots queued before consumption

Desktop's bridge previously sent every `RuntimeInfo` through an unbounded
channel. Each update contains the complete route catalog. Consumer-side
coalescing only discarded old catalogs after the UI resumed draining, so it did
not bound memory while the UI was stalled.

The producer mailbox now keeps only the latest pending `RuntimeInfo` per
session. Ordinary events retain FIFO order, and a replacement snapshot takes
its latest chronological position. Producers never block on the UI, avoiding a
new Cancel/Stop stall and moving a backlog into the SDK's event subscriber.
The last receiver drops queued payloads even if a network worker still holds a
sender. Ordinary event traffic remains lossless and unbounded; this is not a
complete process-memory cap.

The stalled-consumer regression now also asserts retained payload bytes. Its
4,000 snapshots contain 65,553,780 bytes of model strings in total, which the
old FIFO retained until consumption. The production mailbox retains exactly
two final strings totaling 32,778 bytes. This is a measured/asserted queue
payload bound, not a claim that process RSS fell by that amount.

### Edit-preview payload duplication

Each file card previously owned its own full tool-input/output strings. Header
rendering cloned that request again for click handlers. Multi-file diffs therefore
amplified the same payload by the file count and repeated allocation during
rendering. Shared immutable request strings remove these redundant copies while
preserving the full change-review action.

The preview-entity cache can still grow with distinct tool calls. This remains a
follow-up candidate, not proof of the supplied incident's root cause.

### Crash-checkpoint allocation and I/O churn

Recovery runs a serial loop with a one-second delay, not once per streaming
delta. Its background save is awaited, so the loop does not accumulate pending
workspace snapshots. However, every save previously read and deserialized the
entire preceding workspace simply to check its ownership token, and serialized
the next snapshot directly to an unbuffered file.

Ownership checks now deserialize only the small ownership fields through a
buffered reader, skipping document payloads. Writes are buffered and explicitly
flushed before the atomic rename. Existing generation locking, clean-quit
disarming, file privacy, and restoration behavior remain intact. Full snapshots
are still written periodically: this change does not claim to eliminate the
reported cumulative disk writes.

### Missing native macOS memory diagnostics

The host's process monitor was Linux-only. Native macOS memory samples are
needed because TUI attribution cannot account for Desktop's GPUI heap, renderer,
or compressed process footprint. The native monitor records resident and
physical-footprint samples without relying on allocator statistics.

## Validation and remaining reproduction

Regression tests cover stalled-consumer catalog replacement, ordinary-event
ordering, wakeups/closure, receiver lifetime cleanup, and lightweight ownership
checks with an 8 MiB ignored document. Existing recovery tests cover stale
writers, private files, clean quit, and dead-host resources. Shared preview
payload tests verify pointer sharing rather than just content equality.

Observed checks in this investigation:

- Hosted `justrust test --workspace`: 1,866 passed, 13 ignored. One unrelated
  Mermaid viewport test failed under concurrent test load and passed when the
  runner retried it in isolation.
- Host diagnostics tests: nine passed on Linux. The real macOS process probe
  is platform-gated and was not executed on this Linux machine.
- Rebuilt the native Desktop binary. Private-Xvfb crash-recovery acceptance
  passed SIGKILL restoration, draft/focus/layout preservation, expiration,
  and clean-quit disarming.
- The older timed-edit screenshot probe failed its hard-coded geometry check
  on the current compact layout. The native screenshot rendered both file
  cards, and the direct multi-file sharing and review-action tests passed.
  Do not count that older visual probe as passing.
- A 4,800-row GPUI regression processes 200 streaming deltas, checks exactly
  one live row, unchanged-frame cache reuse, and replacement with one row.
  This checks retained model structure, not macOS heap or footprint.

### Follow-up through real public interfaces

A separate native run uses the actual Desktop binary, daemon, and API bridge
on private sockets and a private Xvfb display, with no screenshot-mode fixtures,
mock server, credentials, or model inference. Sessions are created and populated
through `create_session` and context-only `send_message` requests, then opened
through the native `/resume` picker. The workload reads runtime information
1,000 times, writes 100 real context-only messages, and closes/reopens the
session to verify all 101 messages load in Desktop. A native draft is typed
afterward to check responsiveness. Context-only writes do not live-replay another
client's input, so the close/reopen is necessary and explicitly verified.

Observed result: the real native workflow loaded all 101 messages and accepted
the complete 37-character unsent draft after the workload. The recorded native
RSS started at 327,188 KiB and ended at 331,340 KiB, including the additional
history load. Eleven settled samples over 10.0 seconds ranged from 330,948 to
331,340 KiB. This limited Linux run does not establish a long-duration plateau
for large streaming sessions. Two external server reloads interrupted the
driver; final native input verification resumed on the same isolated processes
and the processes were then cleaned up.

Local evidence is in
`target/memory-real-api-resumed-20261018/evidence.json`,
`native-final-state.json`, and `native-final.png`. The earlier native navigation
script also exposed outdated startup assumptions (expecting automatically
created server sessions rather than draft panels); its run is not counted as
passing. The successful follow-up explicitly creates real sessions through
the API and opens them using the native picker.

This native run does **not** reproduce streaming catalog replacement under a
stalled UI. The attempt to change an actual model was rejected for missing
`OPENROUTER_API_KEY` in the isolated environment. No credentials were borrowed
and no authentication controls were bypassed. The smaller supported workload
checks real transport, persistence, native session loading, and input only.
The original Mac and its large transcripts are not available in this session.
Thus end-to-end validation of the reported 90 GB streaming failure remains
externally blocked, not passed by substituting the component regression.

Linux checks and tests cannot prove that macOS compressed footprint is stable.
A final incident reproduction still requires the affected Mac (or a comparable
macOS machine), its large sessions, and concurrent streaming:

1. Open a 4,000+ message session plus another streaming session and the relevant
   side-panel documents, without executing arbitrary content from shared logs.
2. Record native Desktop process RSS and physical footprint over at least
   10 minutes, including periods where rendering falls behind event production.
3. Compare continued growth versus a settled plateau, and check whether the
   process remains responsive to Cancel/Stop.
4. If growth remains, capture an allocation profile and inspect ordinary SDK
   event backlogs, distinct edit-preview entities, and GPUI/platform allocations.

The changes fix concrete amplification paths. They do not establish a unique
root cause for the historical 90 GB report or justify calling that incident
fully reproduced and resolved.
