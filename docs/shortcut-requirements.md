# Shortcut requirements and observed outcomes, 2026-09-06

## Acceptance environments

- **W:** `scripts/accept-wayland-shortcuts.py`, evidence in
  `target/settled-wayland`. Private headless Sway, actual global Super-key
  bindings, actual installed helper, actual `wtype`, native Wayland Desktop,
  real SDK daemon, and an actual Ctrl+R rebuild/reload. The only compatibility
  adapter translates the existing focused-window CLI query into real private
  Sway IPC. No keyboard transport is mocked. No `niri` process is invoked.
- **X:** `target/settled-focus`, 88 private-Xvfb navigation checkpoints, including one real hot reload.
- **K:** `all_super_shortcuts_have_handlers`, all 42 Linux Super bindings checked
  against registered actions with workspace-root and composer focus after
  rebinding. 84 binding/handler checks passed. This checks reachability, not
  Gmail/Todoist/provider service functionality.
- **H:** six helper-routing tests passed, including both Firefox app IDs, all
  four original Firefox commands, unrelated/missing focus, and invalid arguments.
- **L:** Jcode's hermetic managed-launcher tests and exact source-rendered shell
  commands. The generated commands are also used as real global bindings in W.

## Requirement-to-evidence map

| Requirement / changed public output | Concrete check and observed result |
| --- | --- |
| Super+H focuses left | W baseline stayed at slot 1. Fixed global H selected slot 0 with keyboard slot 0, before and after reload. |
| Super+L focuses right | W baseline stayed at slot 1. Fixed global L advanced to slots 1 then 2, with matching keyboard focus, before and after reload. |
| One keypress means one hop; edges do not wrap | W checked both boundary no-ops, reversals, and unchanged session order. Super remained held for 400 ms while the helper forwarded the key. X additionally checked consecutive movement with Super held. |
| Focus survives close, overview, empty strips, and reload | W closed a panel then global H reached a live composer. X checked overview entry/exit and reload, empty-strip transitions, and rapid close/navigation. All 88 checkpoints passed. |
| Stable native Desktop identity for helper routing | W made 27 real focused-window queries. Every query returned app_id `jcode-desktop`, including after UI reload. X checked native WM_CLASS in both generations. |
| Super+Enter creates one panel in the requested repository | W baseline created none. Fixed global Enter changed 3 panels to 4, selected the new slot 2, and daemon creation recorded `/home/jeremy/jcode-desktop`. |
| Ctrl+Alt+Enter forwards to the same pinned action | Actual installed helper emitted this chord through real wtype in W. Direct native alias tests and before/after restart cwd checks also passed in `target/settled-enter`. |
| Super+Q closes the focused panel, not Desktop | W baseline closed none. Fixed global Q changed 4 panels to 3, removed precisely the selected session from the visible panel list, and focused a surviving composer. |
| Closing the last panel must leave shortcuts usable | W closed all remaining panels with global Q, observed zero panels and root focus, then global Enter opened one focused panel in the requested repo. The app stayed alive. |
| Ctrl+Shift+W must not become Ctrl+W word deletion | Helper H asserts the distinct Shift modifier. W exercised actual Ctrl+Shift+W delivery and panel dismissal. Existing prompt-editing/keymap regressions passed unchanged. |
| Super+; creates a pinned panel rather than a terminal window | W executed Jcode's exact generated managed command behind a global semicolon binding. Panel count increased by one, new composer focused, daemon cwd was `/home/jeremy/jcode-desktop`. |
| Super+' creates a home panel rather than a terminal window | W executed the exact generated managed command behind a global apostrophe binding. Panel count increased by one, new composer focused, daemon cwd equaled the isolated HOME. |
| Other app launcher and self-dev behavior stays unchanged | L executed original fallback commands against recording stubs for non-Desktop, absent/failed focus, modified chords, and self-dev variants. All passed. Only the two regular user bindings were changed in the local config. |
| Firefox previous/next/new/close remain unchanged | H compared all eight app-ID/action combinations with the original Ctrl+PageUp/PageDown/T/W argument vectors. All passed. No user Firefox windows or tabs were touched. |
| Every advertised Super shortcut has a handler | Static audit found 42 bindings and exactly six global conflicts: H, L, Enter, Q, semicolon, apostrophe. K verified all 42 mappings on both focus paths. The six conflicts are covered by W. |
| Tutorial key claims remain accurate | Four keymap tests and eleven shortcut tests passed, including every taught shortcut, all advertised catalog mappings, prompt editing, and session directory selection. |
| Updated behavior reaches the user | Installed helper, requested repo pin, and generated launcher lines were rechecked against the tested configuration. At initial delivery, Desktop PID 585390 activated UI generation 1 after the Ctrl+R-equivalent request. That PID is historical, not a claim about the current process. Both managed launcher lines were installed with a backup. Latest keyboard behavior was exercised in isolated native sessions, not on the user's compositor. |

## Concrete combined outcome

W passed **25 state checkpoints and 27 real focus queries**. The old helper
failed all four advertised global actions in the same native app: H/L left the
middle selected, Enter added nothing, and Q closed nothing. The fixed helper
passed all four. Both additional managed launcher commands passed. The full run
also verified the exact creation directories and recovery after closing the last
panel, not merely keymap registration or screenshot appearance.

The earlier exploratory Wayland run needed manual input while diagnosing test
keyboard/first-frame readiness. It was cancelled, its private processes were
cleaned up, and it is **not** counted as passing evidence. W is the subsequent
automated run with explicit virtual-keyboard readiness timing and a configurable
build wait. Its input and state assertions were not relaxed.

The active user's compositor was deliberately not queried or driven. Real
Wayland transport and global grabs were tested on isolated Sway, while the
host-specific focused-window query was translated. Optional connected services
were not invoked by the full keymap reachability audit.

## Cross-repository delivery

The Jcode managed-launcher fix is commit `82a93e6fb819869d2640df8983e4b50aaf3f83ca`.
Normal startup of the existing CLI does not regenerate an already-managed block
when tracking version is 1, which is the current local setting. A future explicit
hotkey reconfiguration should use a CLI rebuilt with this commit.

The supporting Jcode push also published three pre-existing Jeremy-authored
ancestors: `9aaa0ad8a`, `458af80d7`, and `0d6dd5252` (telemetry/concurrency changes
and their validation notes). Those changes were not authored or modified by
this shortcut task. Shared remote history was not rewritten.

## Whole-result recheck, 07:12–07:24 UTC

This historical rerun did not pass the then-current, concurrently modified
worktree. The main requirement map now points to the later passing rerun below.
At 07:12–07:24, the observations were:

- `target/whole-enter`: the old helper created zero panels. Each of nine new
  invocations created exactly one focused panel. Direct Enter, the forwarding
  chord, and the installed helper each used `/home/jeremy/jcode-desktop`, both
  before and after restart. Eight creations used that repository and the home
  control used isolated HOME. Independent comparison with raw daemon
  `ENV_SNAPSHOT` creation records matched all nine session IDs and directories,
  with zero mismatches (`independent-verification.json`). This used the successful
  07:12 build, not later uncompiled changes.
- The current user pin, installed helper bytes, and three source-generated
  managed launcher lines matched their expected values. Six helper tests and
  five hermetic source-generated launcher tests passed again.
- The initial shortcut-filtered UI run passed ten tests, including all 42 Super
  bindings on both focus paths. Later keymap/closing reruns could not finish
  compiling the concurrently edited tree.
- The fresh Xvfb navigation and Wayland global-grab runs did **not** pass.
  Startup/reload compilation was blocked in turn by incomplete remote-module
  integration, remote-test lifetimes, diff text measurement, missing pending
  modules/tests, and pending-method visibility. Cargo contention also delayed
  retries. These are recorded failures, not replacement evidence for the
  previously passing focus, close, identity, overview, and reload checks.

Every row in the requirement map had an assigned concrete check. At that time,
the full-tree focus/close/reload checks and remaining keymap tests still needed
rerunning once shared edits settled. The screenshot rerun first refused to
overwrite an existing image, which was preserved under a new name. No fresh
rendering result is claimed for this recheck.


## Completed native recheck, 07:35–07:55 UTC

The pending shortcut checks above subsequently passed. The earlier failures
remain recorded, but are superseded for keyboard behavior by these fresh runs:

- **W**, `target/settled-wayland`: 25 observed state checkpoints and 27 real
  focused-window queries. Baseline H/L/Enter/Q all did nothing. Fixed H/L selected
  exact neighbors, with boundary no-ops and matching composer focus, before and
  after real Ctrl+R reload. Enter changed 3 panels to 4 in the requested repo.
  Q removed precisely that panel, 4 to 3, without exiting. H still worked after
  closing. Semicolon and apostrophe each added exactly one focused panel in the
  repo and isolated HOME respectively. Closing all five panels left the app
  alive, and Enter reopened one repo panel. All four saved creation records
  were independently matched to raw daemon records, with zero mismatches.
- **X**, `target/settled-focus`: all 88 native checkpoints passed across a real
  reload, including held modifiers, direction changes, edge no-ops, overview,
  empty strips, and both closing-panel transitions. The separate linked-UI run
  also passed and its `navigation.png` was inspected: real panel content and
  the selected composer were visibly rendered.
- `target/settled-enter`: old helper created zero. Nine real commands each
  created exactly one focused panel. Direct Enter, forwarded Enter, and installed
  helper used the requested repo before and after restart. Eight creations used
  the repo and the home control used isolated HOME. Every session ID/directory
  matched raw daemon creation records, with zero mismatches.
- **K**: the freshly compiled UI test executable passed 11 shortcut tests, four
  keymap tests, and three closing-navigation tests. This includes all 42 Super
  bindings on both focus paths. Reusing the compiled executable avoided another
  redundant Cargo queue, without substituting or copying test implementations.
- **H/L**: six helper and five hermetic launcher tests passed. Installed helper,
  current pin, and all generated launcher lines still match their expected
  source/configuration. No user compositor was driven.

The Wayland runner now accepts a positive `--build-timeout` (default unchanged:
180 seconds). The passing run used 600 seconds for build/activation waits only.
Input/state assertion timeouts were not relaxed. Zero and negative values were
verified to fail before starting processes. The complete run took 135 seconds.

Offline fixture screenshots are **not** passing visual evidence: the standard
build attempt hit Cargo contention, and two already-built fixture captures
returned black images despite exit 0. Both images were read and rejected.
Native real-session navigation rendering was visibly verified separately.
This fixture-capture limitation does not replace or invalidate the successful
native keyboard, focus, session-creation, close, and reload observations above.

### Scope of the earlier recheck

The six intercepted shortcuts have end-to-end behavioral evidence, and the
navigation suite covers additional movement/overview/closing paths. The
42-binding audit proves handler reachability, not every action's complete
behavior. It does not establish connected-service outcomes, untouched platform
variants, or direct operation on the user's compositor. The offline fixture
capture problem also remains unresolved and is not described as a passing
rendering check.

## Expanded action outcomes

The [42-binding behavior matrix](super-key-behavior-matrix.md) separates actual
outcomes from registration coverage. Nine additional public GPUI keystroke tests
passed three consecutive runs from both workspace and composer focus. Native
extension checks additionally exercise movement, width, service-panel reuse,
real terminal creation, and a new home-directory session.

This broader check found a real close regression: after dismissing a newly
created panel, the visible and keyboard focus moved to a survivor but the row's
remembered focus became empty. Commit `05e4886` updates that memory immediately.
The new close regression and all other closing tests passed, the extended native
check advanced beyond the formerly failing checkpoint, and all six intercepted
global shortcuts passed again in `target/extended-global-close` (25 state
checkpoints, 27 focused-window queries, including a real UI reload). The running
Desktop was restored and the Ctrl+R rebuild/reload path activated successfully.

The same extended run exposed a separate SDK/daemon boundary bug: Super+Space
cannot fork a new empty session whose snapshot has never been saved. The UI
emitted the correct request, but the daemon returned a missing-file error.
Runtime commit `4e7009930` fixes both the missing-parent fallback and persistence
of an otherwise empty fork child. The untouched empty root remains lazily saved.
Eleven scoped core tests and three base persistence-policy tests passed. One
unrelated swarm-toggle expectation failed in both the new binary and an older
pre-change test binary and was not changed.

With the freshly linked daemon, `target/super-all-runtime` passed all 86 native
checkpoints. The parent snapshot did not exist, yet Super+Space created exactly
one focused child whose saved parent ID and inherited directory matched, with
two persisted messages including the fork notice. Super+Shift+Q then exited the
real app with status 0. Its final screenshot was read and visibly rendered two
real panels and the selected composer. Nine GPUI behavior tests and five closing
tests also passed again in the latest compiled UI test binary.

The native runner's `--extended-shortcuts` opt-in writes additional state
checkpoints, actual home-session creation records, fork lineage, and a native
Quit exit-code record. `--jcode-binary` selects an explicit runtime executable
for cross-repository validation. Missing paths and directories are rejected
before artifacts or processes are created. The coverage regression checks each
of the 42 documented chords against its actual registered action, not just a
count of rows. Service credentials stay isolated, and no provider inference or
mail/task mutations are requested.

The hot-plugin repeat `target/super-all-reload` exposed another integration
boundary before its explicit Ctrl+R: startup plugin replacement detached and
removed the original unsaved idle Agent. The restored panel still displayed its
ID, but history never loaded and no fork request reached the daemon. That run
is not a pass. The native extension now requires successful history attachment
for every original real session, so merely restoring visible IDs cannot satisfy
acceptance. A bounded shared-runtime reconnect grace is being verified rather
than eagerly persisting every empty root session.

## Final whole-result verification, 08:39–08:41 UTC

The reload failure above is fixed by runtime commits `8d3d36ec3` (bounded idle
reconnect retention) and `3fb613468` (SDK attachment using the daemon's
authoritative live/persisted target directory). There is no guessed temporary
directory, eager saving of all empty roots, or Desktop-only workaround.
Unknown targets produce correlated API errors.

- `target/super-all-final` passed **172 native checkpoints**, including one
  explicit Ctrl+R reload. Every original real session loaded history in both
  generations. All extended movement, width, creation, service reuse, terminal,
  fork, closing, and native Quit checks completed. Both forks used the same
  genuinely unsaved parent and persisted the exact parent ID, inherited HOME,
  and two messages. Both lineage records independently matched the saved child
  JSON. Quit exited 0. The final image was read and visibly rendered real panels
  with the selected composer.
- `target/super-global-final` passed **25 global-key state checkpoints and 27
  real focused-window queries** against the final runtime. The old helper still
  dropped H/L/Enter/Q. The installed helper selected exact neighbors, created
  repo panels, closed exactly the selected panel without exiting, and recovered
  from an empty workspace. Semicolon and apostrophe used repo and HOME as
  configured. All four creation records independently matched raw daemon
  records and their expected directories, with zero mismatches.
- Final scoped runtime checks passed: 85 bridge tests, eight target-directory
  checks, 11 cleanup/grace checks, 11 client-action checks, and three base
  persistence checks. Desktop's nine public GPUI action tests and five closing
  tests passed. The six installed-helper regressions, three behavior-map/CLI/
  attachment-oracle tests, and seven navigation-oracle tests also passed.

### Verified live activation

The corrected runtime, bridge, and Desktop UI are now active. The earlier
activation warning mistook an old recovery checkpoint for a live Desktop:
its owner had exited, and the two referenced sessions were no longer present
in the daemon. That checkpoint was backed up, not discarded. Fresh preflight
confirmed all seven actually connected clients had valid saved snapshots and
no background tasks were running before the non-forced daemon reload.

Observed delivery on 2026-09-06 (UTC):

- **08:47:** promoted the verified runtime and completed a non-forced shared
  daemon reload. All seven original connected clients reconnected with their
  exact session identities. The running executable matched the accepted hash.
- **08:52:** replaced the API bridge after checking all 15 panels in the newly
  running Desktop were idle. The corrected runtime's reconnect grace retained
  all 15 session identities through the bridge handoff, with drafts unchanged.
  The running bridge matched the accepted hash.
- **08:53:** the live Desktop accepted its Ctrl+R-equivalent socket action and
  activated UI generation 2. All 15 panel identities and their order, drafts,
  and layout were preserved, and every session reattached successfully.

The immutable accepted executables and updated acceptance manifest are under:

```text
~/.jcode/builds/versions/3fb613468-debug-575bc278a882/
  jcode
  jcode-harness-api-bridge
  acceptance.json
```

SHA-256:

```text
jcode: 575bc278a8822b803f3eae5883b01321010437c0c109d8e1793e75ba99e4be1f
bridge: 5384ad72b431d84d21acf0e9fdcb216541058829b0572bc0943d5adfad2ca31f
```

The shared-server channel and Desktop companion links select these artifacts.
Stable/current launcher channels were not changed. Private activation records
in `~/.jcode/scratch/shortcut-activation/` contain `daemon-activation.json`,
`bridge-activation.json`, and `desktop-reload.json`. They retain exact identity
comparisons without publishing session contents in this repository. Artifact
hashes establish provenance because development build metadata can retain an
older Git hash.

## Post-deployment public-interface rerun, 08:56–08:57 UTC

`target/shortcuts-public-recheck-0856` reran the complete extended native
workflow using the exact deployed immutable runtime. It passed with four real
sessions and one real Ctrl+R rebuild/reload. Both generations exercised native
focus (including held modifiers and edges), row movement, overview, widths,
home-session creation, Gmail/Todoist opening and reuse without credentials,
real terminal creation, unsaved-parent fork persistence, and selected-panel
closing. Every original session reattached before subsequent actions. Native
Quit exited successfully. Both saved fork children independently matched their
recorded parent identity and working directory. The final screenshot was read
and showed rendered real panels with the selected composer. The three mapping,
attachment-oracle, and invalid-runtime-path regression tests also passed.

This is an additional real app/SDK/daemon acceptance run, not a substitution of
recording-bridge tests for native behavior. Previously documented recording-host
boundaries for help/provider requests and connected external services remain.
