# Nested swarm sidebar

Root sessions remain ordinary sidebar entries. Sessions with swarm ownership metadata are rendered inside their root's entry rather than duplicated as top-level history entries. Ordinary transcript forks do not imply swarm ownership.

The nested area shows the agent count, working count, task labels and lifecycle statuses. Two agents are shown by default, with Show more / Show less for larger groups. Child clicks activate that session without activating or closing the parent. Multi-level swarms use depth-first ordering and indentation. Missing parents and invalid ownership cycles remain visible as independent entries.

## Data

The SDK session API exposes optional `parent_session_id`, `agent_label`, and `swarm_status` fields. Ownership is sourced from durable swarm `report_back_to_session_id`, not the session transcript's fork parent. Local desktop refreshes use the shared SDK enrichment helper for compatibility with already-running older daemons. Remote parent IDs are namespaced to the same host as the child. Status describes the latest persisted swarm snapshot, not an independent liveness probe.

## Verification

- `cargo test -p jcode-desktop-ui sidebar_ --lib`: 62 passed, 1 manual profiler ignored.
- `cargo test -p jcode-desktop-ui harness:: --lib`: 40 passed, 1 real-model test ignored.
- `python3 -m unittest discover -s scripts -p 'test_screenshot.py'`: 9 passed.
- Grouping tests cover child/grandchild order, ordinary independent sessions, missing parents, self references and cycles.
- GPUI tests cover metadata enrichment through session refresh into real rendering, automatic row growth/shrink, compact previews, expansion, non-duplicated children and child click navigation.
- A remote-address test verifies the child and parent receive the same host namespace.
- `python3 scripts/screenshot.py target/ui-review.png --swarm`: real application rendered on private Xvfb and PNG visually inspected. No active desktop windows are used for testing.
- The running release desktop acknowledged its Ctrl+R rebuild/reload action and activated UI generation 4 in the original process. Subsequent session refreshes succeeded without a panic.

The session catalog remains bounded. A child whose parent is outside the loaded catalog stays visible independently until its parent is available.
