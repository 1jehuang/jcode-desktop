# Remote machine connections

Desktop's Machines picker connects normal native session panels through the shared
SDK and system OpenSSH. Selecting a default affects new session panels, not
existing panels or explicit local folder/terminal/help actions. Remote session
identities are namespaced and survive workspace snapshots.

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
  Results and screenshots are generated under the specified output directory.

The configured real `desktop` SSH target timed out, so it was not used as evidence
of a successful remote connection. No remote software was installed, no SSH trust
settings were changed, and no provider inference was used by the SSH acceptance.

The native acceptance exposed a mouse-focus gap in the host editor. Its wrapper
now explicitly focuses the host input, and the GPUI regression switches away
from it before clicking back and typing. Connection failure/retry is shown above
the machine list so it remains visible on smaller windows.
