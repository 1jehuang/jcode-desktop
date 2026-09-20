# Nested swarm sidebar

Root sessions remain ordinary sidebar entries. Sessions with swarm ownership metadata are rendered inside their root's entry rather than duplicated as top-level history entries. Ordinary transcript forks do not imply swarm ownership.

The nested area uses two-line agent cards: the stable task label has its own line, with lifecycle status and cumulative edit counts beneath it. The summary distinguishes working, attention needed, done, ready, waiting and inactive agents. No running agents does **not** imply a completed swarm. Thinking/streaming count as working, and network waits count as blocked. Unknown lifecycle strings remain visible rather than being silently relabeled idle.

Two agents are shown by default. The preview prioritizes the focused session, failed/blocked work, running work, then ready/waiting agents. Selection is by session ID, never by a reordering list index. **Show all / Show fewer** expands the full depth-first ownership tree. Compact previews are flat because they may omit ancestors. Expanded trees use indentation, never decorative rails. Missing parents and invalid ownership cycles remain visible as independent entries.

Click an agent to open or focus its existing conversation. **Open lead** returns to the owning root without closing the agent's view. Open/viewing indicators distinguish mounted panels from lifecycle status. The **×** on an open agent only closes the view (`Unwatch`), not its work. Reopening during the close animation revives the same panel and draft and restores `Watch`, avoiding duplicate event recipients. Hover for the complete task label, reported status, actual parent and working directory.

## CLI comparison

The CLI/TUI separates stable agent identity from changing activity. Its calm chat cards emphasize task labels, while the full swarm view exposes live output, recent tool intents, todos and runtime details. Keyboard controls cycle swarm views and navigate members. Relevant implementations are `jcode-tui-render/src/swarm_gallery.rs`, `jcode-tui/src/tui/info_widget_swarm_gallery.rs`, and `jcode-tui/src/tui/app/remote/swarm_status_core.rs` in the adjacent Jcode checkout.

Desktop adopts the stable identity/status separation and safe inspect/detach interactions. Rich daemon `SwarmMemberStatus` data is not currently exposed as a typed swarm event through the harness SDK. This change does not invent live progress from persisted metadata or read daemon-only files as a Desktop-specific substitute. Conversation inspection continues to use the normal SDK watch path.

## Data

The SDK session API exposes optional `parent_session_id`, `agent_label`, and `swarm_status` fields. Ownership is sourced from durable swarm `report_back_to_session_id`, not the session transcript's fork parent. Local desktop refreshes use the shared SDK enrichment helper for compatibility with already-running older daemons. Remote parent IDs are namespaced to the same host as the child. Status describes the latest persisted swarm snapshot, not an independent liveness probe.

## Verification

Current regression commands:

```sh
cargo test -p jcode-desktop-ui sidebar_ --lib
python3 scripts/screenshot.py target/swarm-review.png --swarm
python3 scripts/screenshot.py target/swarm-native.png --swarm-interact --no-build
```

Use `--no-build` only after a current paired Desktop build. Native acceptance clicks the real UI on private Xvfb and cross-checks rendered labels against persisted navigation state. It covers expansion, opening the initially hidden Test runner, keeping it visible after collapse, return to the original lead, and closing only the agent view. Per-step PNGs and a `.swarm.json` report accompany the capture. GPUI tests also assert exact `Unwatch`/`Watch` commands, panel reuse, lifecycle aliases, prioritized previews and immediate reopen event delivery.

Initial implementation verification (historical):

- `cargo test -p jcode-desktop-ui sidebar_ --lib`: 62 passed, 1 manual profiler ignored.
- `cargo test -p jcode-desktop-ui harness:: --lib`: 40 passed, 1 real-model test ignored.
- `python3 -m unittest discover -s scripts -p 'test_screenshot.py'`: 9 passed.
- Grouping tests cover child/grandchild order, ordinary independent sessions, missing parents, self references and cycles.
- GPUI tests cover metadata enrichment through session refresh into real rendering, automatic row growth/shrink, compact previews, expansion, non-duplicated children and child click navigation.
- A remote-address test verifies the child and parent receive the same host namespace.
- `python3 scripts/screenshot.py target/ui-review.png --swarm`: real application rendered on private Xvfb and PNG visually inspected. No active desktop windows are used for testing.
- The running release desktop acknowledged its Ctrl+R rebuild/reload action and activated UI generation 4 in the original process. Subsequent session refreshes succeeded without a panic.

The session catalog remains bounded. A child whose parent is outside the loaded catalog stays visible independently until its parent is available.
