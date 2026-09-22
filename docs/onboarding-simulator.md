# Real first-run onboarding rehearsal

**Alt+Shift+5** runs `scripts/onboarding-desktop.py`. **Alt+9**, `/onboarding-sim`,
and `/onboarding-preview` call the same launcher from Desktop. Each invocation
opens a **separate production Desktop window with a new disposable profile**.
The normal Desktop workspace, drafts, accounts and settings are not replaced.

There is no separate onboarding mock anymore. The new process follows the normal
startup path: the real optional Jcode account welcome and email sign-in, the beta
notice, and the real workspace/provider account setup. Editing production
onboarding changes the rehearsal too. The launcher selects the most recently
built local Desktop executable. Build your changes, then reopen the rehearsal.
It uses `--no-hot-reload`, not screenshot fixtures or a reconstructed UI.
The newest installed or adjacent `jcode/target/{debug,release,selfdev}/jcode`
companion is used without installing it or restarting the normal runtime. A bundled
sibling companion takes precedence, matching production Desktop. For a specific
build, use `--binary /path/to/jcode-desktop --jcode-binary /path/to/jcode`.

## Data isolation and real behavior

- A new private profile supplies `HOME`, `JCODE_HOME`, all writable XDG paths,
  the working directory, and independent Desktop, daemon and harness sockets.
- Environment inheritance is allowlisted. API keys, provider directory overrides,
  SSH agents, proxy credentials, configuration overrides, fixture flags and the
  user's session bus are not inherited. Executables and the display connection
  are shared, not existing credential/configuration/session files.
- Sign-in and provider connections are **real**, not simulated successes. Any
  credentials you explicitly enter are written to the disposable profile.
  Signing in can create real server-side account/session state. Closing the
  rehearsal does not revoke server-side authorization or undo account actions.
- Web links open Firefox with `--no-remote` and a fresh profile per requested
  browser window. Existing browser cookies and SSO sessions are not reused.
  Firefox must be installed to use browser sign-in. Browser-launch failures
  never fall back to your normal browser or desktop portal.
- Closing the rehearsal window stops its supervised runtime/browser descendants
  and deletes only that invocation's profile. Reopening starts from scratch.
  Repeated shortcut presses do not close or reset previous windows.
- This is **profile isolation, not an OS filesystem/network sandbox**. Explicitly
  selecting a real folder still accesses that folder. Public startup requests
  and user-initiated sign-ins remain production behavior. External programs that
  hard-code paths rather than honor these environment settings are not confined.

The isolated window uses the same controls as the real app: **Escape skips the
optional account welcome**, it does not exit a fake walkthrough. Close the window
to finish the rehearsal and discard its data. The ordinary Desktop remains intact.

Linux with `/proc`, `pidfd` and child-subreaper support is currently required by
this developer launcher. Abnormal termination of the supervisor can leave its
private runtime-directory profile behind until logout. Never point cleanup at a
real home directory.

## Verification

```sh
python3 -m unittest discover -s scripts -p test_onboarding_desktop.py -v
cargo test -p jcode-desktop-ui onboarding_simulator -- --nocapture
python3 scripts/screenshot.py target/onboarding-real-review.png --onboarding-interact
```

The Python tests check contaminated-parent environment filtering, fresh independent
launches, browser routing, authenticated private readiness, startup failure and
cleanup of detached descendants without touching parent data. GPUI tests verify
shortcut/command routing, visible launcher failures, unchanged workspace state
and safe deserialization of legacy fake-simulator snapshots.

The native acceptance launches the actual production binary on private Xvfb,
without `JCODE_DESKTOP_SCREENSHOT` or fixture account states. It checks Welcome,
Skip, the beta notice, fresh workspace state, private runtime endpoints and
profile cleanup. No sign-in or model submission is initiated by that test.
The existing `--account-sign-in-interact` fixture test remains useful for bounded
offline waiting/cancel UI coverage, but is not evidence of end-to-end real auth.
