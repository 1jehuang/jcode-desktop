# Interactive startup

## Behavior

The first conversation panel is created locally before the runtime connects.
It starts at its final width with keyboard focus in the composer. Typing,
selection, undo history, and attachments stay in the same input entity when the
runtime session arrives. Enter retains the draft until sending is available.
A late creation reply does not steal focus or reopen a closed panel.

Development launches no longer run Cargo before opening the window. They show
the linked UI first, build in the background using normal Cargo freshness, and
use the existing snapshot handoff to activate a newer UI. Explicit Ctrl+R still
forces a fresh build. Startup and explicit rebuilds share an overlap guard.
Plugin activation no longer brings an unfocused application to the foreground.

Persisted sidebar scanning also runs on the session-list worker rather than
holding up session creation. Startup creation retries transient connection
failures while the draft remains editable.

## Reproduce

```sh
cargo test -p jcode-desktop-ui --lib startup_ -- --test-threads=1
cargo test --bin jcode-desktop -- --test-threads=1
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/accept-startup.py target/startup-acceptance
python3 scripts/screenshot.py target/startup-ui-review.png --no-build
```

Use new output paths for the two visual runners. The startup runner uses a
private Xvfb display, private runtime/home, and gated Jcode/Cargo launchers. It
checks real native typing while both services are blocked, then session
attachment and the automatic UI-generation handoff without submitting a prompt
or invoking inference.

## Verification, 2026-09-05

- All four GPUI startup tests passed. Coverage exercises focused editing before connection, retained
  text after early Enter, correlated in-place promotion, snapshot restoration,
  normal creation responses arriving out of order, and closed/closing panels.
- The offline screenshot runner rendered the real app and the image was
  inspected at `target/startup-ui-review.png`.
- The installed `.desktop` launcher selects the newest repository binary and
  executes it directly with `--hot-reload`. There is no launcher-side build.
- The running desktop accepted a rebuild-and-reload request through its private
  instance socket and logged activation through UI generation 4. The rebuilt host's
  pre-window behavior takes effect on the next normal launch, without restarting
  or discarding the user's current workspace.

The initial full UI-suite run passed 340 tests, failed four other surface checks,
and ignored six opt-in tests. The failures were the email wheel-scroll check,
minimap live-state indicator check, restored transcript scroll check, and
vertical-swipe reticle check. These checks are outside the startup paths and
concurrent UI work was present. This is not a claim that the entire checkout's
UI suite is green. See `target/startup-full-ui-tests.log`.

### Final native result

`target/startup-final-acceptance/verified-summary.json` records a successful
whole workflow: both startup gates held, native typing and early Enter, real
session attachment, and an automatic background rebuild/reload. The same draft
and keyboard focus survived every stage. Composer-only OCR checks of all four
captured frames passed, and the pending/reloaded screenshots were inspected.
No model prompt was sent. All 19 host tests also passed, including the new
rebuild overlap guard and existing same-window reload/rollback/resource tests.

Earlier isolated runs reached the focused panel 1.12s and 1.11s after process
launch. The final run under heavy concurrent build and filesystem load took
6.00s. These are software-rendered Xvfb measurements, not a promise of zero-time
platform initialization. Crucially, the composer was usable while Cargo and the
runtime were deliberately blocked, and remained usable during the final build
queue, which delayed the completed handoff until 255s into the run.

Final checks were interrupted by a harness restart, concurrent edits, and a
full-disk linker failure. Removing only unmapped temporary test plugin copies
restored build capacity. The completed host test run, native reload acceptance,
and live UI-generation activation above are the recovery evidence.
