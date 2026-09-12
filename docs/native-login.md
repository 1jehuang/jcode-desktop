# Native login and model recovery

Desktop provides a **Connect account** button beside the session identity. `/login`
and `/login <provider>` open the same native dialog. Browser sign-in, device-code
sign-in, and API-key entries come from the SDK's capability-filtered shared provider
catalog. The dialog has mouse-operated provider choices, browser launch, clipboard
paste, completion, back, and cancel controls. It does not run login as an agent prompt.

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
