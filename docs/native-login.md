# Native login and model recovery

Desktop provides a **Connect account** button beside the session identity. Clicking
it opens and focuses a dedicated **Accounts** panel immediately to the right of the
conversation, without replacing its transcript or draft. Clicking again reuses the
same source's accounts panel. Close or Escape returns to the source conversation.
Accounts panels are transient and never become harness sessions or saved logins in
reload snapshots. `/login` and `/login <provider>` still open the native dialog in
the current panel. Browser sign-in, device-code
sign-in, and API-key entries come from the SDK's capability-filtered shared provider
catalog. The dialog has mouse-operated provider choices, browser launch, clipboard
paste, completion, back, and cancel controls. It does not run login as an agent prompt.

Each provider has a colored dot, tinted badge and border, and a text status:

- **Green, Connected:** available credentials and a recent successful runtime check.
- **Amber, Not verified:** credentials exist but a recent working check is absent.
- **Red, Expired / Needs attention:** expired credentials or a recorded check/refresh failure.
- **Gray, Not connected:** the provider has no configured credentials.
- **Checking / Status unavailable:** loading or a failed/unknown health lookup, never green.

Opening the panel and **Refresh status** read `jcode auth doctor --json` in a bounded
background process. They do not run `--validate`, send model requests, or change
credentials. Colors reflect saved checks, not a live connectivity guarantee. Stale
validation failures and successes are unverified rather than presented as current.

Authentication, unavailable-model, and quota errors render native model choices and
an account action where relevant. Terminal-only login/model instructions are replaced
with desktop guidance, while **Copy details** retains the original diagnostic. Empty
model lists provide **Connect account** and **Refresh choices** rather than another
slash-command instruction. Selecting a model preserves the draft and never silently
replays a prompt.

## Privacy and lifecycle

- Login input is masked, has no history/snapshot integration, and cannot be copied
  out through the field. IME readback is masked too.
- OAuth callbacks go to the CLI over stdin, never command-line arguments or chat.
- Each flow has a unique ID. Cancelling closes the owned process and cleans up only
  that pending flow, not saved accounts or other clients' pending logins.
- Login state is deliberately absent from reload and crash-recovery snapshots.
- Successful login wakes account refresh and requests fresh runtime/model routes.
- Remote panels explicitly refuse local credential provisioning. Local login does
  not imply authentication on the SSH host.
- Only methods supported by the installed CLI/SDK are offered. Jcode hosted login
  currently uses the API-key entry, not the CLI's interactive device flow.

## Verification

```sh
cargo test -p jcode-desktop-ui --lib login
cargo test -p jcode-desktop-ui --lib recovery_
python3 -m unittest discover -s scripts -p test_login_acceptance.py
python3 scripts/screenshot.py target/login-review.png --transcript empty --login-interact
```

The private Xvfb test operates the real app with mouse clicks and a dummy clipboard
secret, verifies masked pixels, and checks that closing restores the original draft.
GPUI tests cover direct `/login`, unknown providers, empty keys, remote refusal,
secret focus/snapshot isolation, successful-login refresh, and clickable error
recovery. The sibling SDK suite covers subprocess arguments/stdin, cancellation,
malformed responses, and real installed-CLI Claude/OpenAI begin/cancel in isolated
credential homes. No browser authorization or real credential exchange is performed
by these tests.

### Accounts panel verification (2026-09-12)

| Requirement | Check |
| --- | --- |
| Open beside the conversation and focus the new panel | Workspace `accounts_panel_*` tests and native Xvfb navigation state verify adjacent slot, keyboard focus, retained source identity/history, and width demotion. |
| Keep the draft and close cleanly | Native clipboard/masking test closes the accounts panel and verifies the original draft and focus. GPUI tests cover Close, Escape, Back, repeat-click reuse, and post-login source model selection. |
| Color working accounts without overstating health | `connection_health_does_not_confuse_saved_credentials_with_working_accounts` checks successful, failed, stale, expired, missing and unknown reports. `accounts-review-2-providers.png` visually shows green, amber, red and gray badges. Native acceptance also measures green/red pixels. |
| Reach providers in the scrollable list | Native acceptance scrolls to OpenAI API, opens its masked key input, pastes a dummy key, and checks dot-sized glyphs. No credential submission occurs. |
| Remain safe across remote, preview and reload boundaries | Dedicated-panel tests cover remote refusal and transient snapshot omission. Preview account panels inherit the source's offline guard. |
| Use the actual installed health contract | Read-only `jcode auth doctor --json` returned available-provider reports with structured `validation_detail.success` and `stale` fields. Missing providers in this configured-only report are unconnected, while a failed report remains unknown. |

The real offline interaction result is retained at `target/accounts-review-2.login.json`.
The login-filtered UI suite passed 18 tests and the Python acceptance helper passed
5 tests. These checks do not claim a live sign-in or model connectivity test.

The final three dedicated-panel tests also passed, including offline preview and
pending-session runtime guards. The running desktop activated UI generation 5 via
its Ctrl+R rebuild path at 10:00:34 UTC (`target/accounts-live-activation.log`).
An earlier full UI run passed 732 tests with 8 ignored. A later run against the
concurrently changing shared tree had 718 passes, 22 failures in other sidebar,
tutorial and rendering tests, and 8 ignored. All dedicated accounts and login tests
passed in that run. The later full suite is not claimed as passing.
