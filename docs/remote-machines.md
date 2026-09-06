# Remote machine connections

Desktop's Machines picker connects normal native session panels through the shared
SDK and system OpenSSH. Selecting a default affects new session panels, not
existing panels or explicit local folder/terminal/help actions. Remote session
identities are namespaced and survive workspace snapshots.

## Implementation commits

- Desktop picker/routing/preferences: `165766c`.
- Shared SDK and `api --stdio`: Jcode `797481a264f7ae6039d1a4babea38d19919d8d2a`.

## Verification (2026-09-06)

- Remote suite: 29 passing tests across preferences, bounded SSH alias discovery,
  namespace validation, SDK socket-pair routing, create/fork/reconnect, native
  GPUI picker/shortcuts, draft preservation/retry, snapshots, and local filesystem
  isolation. The real project test executable was used.
- Local regression subsets: 38 harness tests passed (one existing ignored test),
  13 startup tests passed, and 9 configuration tests passed.
- Shared SDK: real localhost OpenSSH against a freshly built `jcode api --stdio`
  passed authentication, handshake, create, no-reply context persistence, list,
  disconnect/reattach, strict host-key rejection, shared daemon survival, and
  daemon EOF while stdin remains open. The shared API-server suite passed 84 tests.
- Native picker acceptance: `python3 scripts/accept-machines.py
  target/machines-acceptance` uses private Xvfb, isolated configuration/home, and
  an inert runtime. It exercises saved defaults, process restart, hostname entry,
  and switching back to this computer without touching a real remote machine.
  All five checks passed: picker controls, persisted remote default, restoration
  after process restart, typed host plus Enter, and switching back to local.
  Evidence: `target/machines-acceptance/report.json` and companion screenshots.
- `cargo build -p jcode-desktop -p jcode-desktop-ui` passed. The normal
  `--reload-ui` path activated the newly built UI generation in the running app.
  `target/machines-final-review.png` was captured and visually inspected.

The configured real `desktop` SSH target timed out, so it was not used as evidence
of a successful remote connection. No remote software was installed, no SSH trust
settings were changed, and no provider inference was used by the SSH acceptance.

The host editor explicitly supports mouse refocus. Its GPUI regression switches
away before clicking back and typing. Native automation initially clicked the
word SSH in the help paragraph rather than the field, then encountered ambiguous
placeholder OCR. Targeting the field relative to its Connect via SSH button fixed
the test, with ordinary global X11 keyboard input unchanged. Connection failure
and retry are above the machine list for smaller windows.

The final shared SDK rerun passed all 47 checks: 23 unit tests, 21 integration
tests, and 3 doctests. Additional root CLI tests timed out during compilation,
not test execution. Direct fresh-CLI flag/help checks and the native SSH workflow
were verified separately.
