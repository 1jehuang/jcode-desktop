# Enter during session startup

The reported live panel was `startup://draft`, still connecting to the configured
remote machine. `Panel::connect_input` disabled submission for that placeholder,
and `PromptInput::submit_prompt` silently returned on Enter. Remote/cloud failures
were only visible inside Machines. The normal connected-editor native acceptance
passed before this change, so this was a readiness/recovery bug, not a key binding
or hot-reload action identity failure.

## Behavior

- Enter and Ctrl+Enter queue ordinary startup prompts visibly. Nothing is sent to
  a placeholder ID. The queue drains once the real session and history are ready.
- Queue text, images, and waiting state survive snapshots. Repeated readiness
  events cannot send the same queue twice. Removing all prompts clears waiting state.
- Slash commands remain in the editor with a readiness explanation. A subsequent
  explicit Enter after connection uses the normal command routing.
- Pending panels show connection progress/errors inline. Retry is explicit.
  **Use this computer** keeps the same editor, draft, and queue, does not change
  the saved default, and uses a fresh request ID to reject late remote replies.
  There is no automatic local fallback.
- Cloud startup shows a **Jcode Cloud VM** label and a subdued cloud background.
  The identity remains visible on failure, disappears on explicit local recovery,
  and is not shown for ordinary SSH targets. The background does not intercept
  prompt input.

## Verification (2026-09-18)

- 37 focused GPUI tests passed across `workspace::remote_tests`,
  `workspace::pending_tests`, `workspace::startup_tests`, `panel::queue::tests`,
  and `input::pending_session_tests`. Evidence: `target/enter-final-focused-tests.log`.
- Ten screenshot-harness unit tests passed.
- The rebuilt release passed private-Xvfb `scripts/screenshot.py` acceptance with
  `--transcript empty --pending-interact` and separately `--fresh-interact`.
  Native keyboard/mouse input and pixels verify queuing, editor clearing, retained
  focus, retry, explicit local recovery, and ordinary prompt submission. Artifacts:
  `target/enter-release-pending*` and `target/enter-release-fresh*`.
- The running release host's Ctrl+R-equivalent action rebuilt and loaded generation
  2. Its newly mapped library matched the rebuilt plugin SHA-256. A new-generation
  checkpoint confirmed the same process, panel, focus, and unchanged draft.
  Evidence: `target/enter-live-reload.json`. No live prompt was submitted for testing.

The native fixtures use an inert bridge. A separate real-runtime acceptance run
below closes the submission/persistence boundary without that fixture bridge.
Cloud credentials and remote availability are not repaired or bypassed.

An earlier full-suite run had 996 passes, 9 ignored tests, and 8 failures in
changelog identity, panel surface, pull-preview, and swipe tests. A baseline build
comparison was cancelled to avoid delaying delivery, so those failures are not
claimed to be pre-existing or resolved. The final focused suite above is green.

## Real native submission acceptance

`scripts/accept-startup-submit.py` launches the production Desktop, real Jcode
daemon and API bridge, and system SSH on private Xvfb with an isolated HOME.
It does not enable screenshot fixture mode. A reserved, non-listening loopback
port produces a genuine SSH connection refusal without contacting a remote host.

- **Before:** the original running host executable was preserved as
  `target/enter-old-desktop`. Native Enter left the exact prompt in the editor,
  verified by native clipboard, with no visible queue or local session.
  Evidence: `target/enter-real-old-v2/result.json`, `silent-startup.png`,
  `retained-editor.txt`, and `ssh-stderr.log` in that directory.
- **After:** native Enter visibly queued the prompt and cleared the editor.
  Clicking **Use this computer** created a real local session in the same panel,
  and the daemon accepted and durably saved that queued prompt exactly once.
  A subsequent ordinary Enter submitted a second prompt exactly once. Both
  appeared in the conversation, and focus stayed in the editor. Native close
  checkpointed the daemon's append journal before final durable assertions.
  Evidence: `target/enter-real-v4/result.json` and `final.png`. The final release
  containing the cloud visuals passed the same 18 checks again in
  `target/enter-real-final/result.json`, with both prompts saved exactly once.

```sh
python3 scripts/accept-startup-submit.py target/enter-real-check --cli /home/jeremy/jcode/target/debug/jcode
python3 scripts/accept-startup-submit.py target/enter-old-check --cli /home/jeremy/jcode/target/debug/jcode --desktop target/enter-old-desktop --expect-silent-startup
```

Each output directory must be new. These runs deliberately have no provider
credentials and use loopback HTTP proxies. Model generation reports an account
authentication error as expected. The verified acceptance boundary is actual
prompt submission and durable runtime persistence, not a generated response or
successful cloud provisioning. No live user session or credentials are used.

## Cloud visual follow-up

The expanded focused suite passes 38 tests, including cloud identity during
connection/failure, Enter queuing with the background present, draft preservation
and cloud identity removal on local recovery, and absence for ordinary SSH.
The screenshot harness passes 11 unit tests. Evidence: `target/cloud-startup-tests.log`.

Actual app renders were inspected on a private Xvfb display:

```sh
python3 scripts/screenshot.py target/cloud-startup-connecting.png --transcript empty --cloud-startup connecting
python3 scripts/screenshot.py target/cloud-startup-failed.png --no-build --transcript empty --cloud-startup failed
python3 scripts/screenshot.py target/cloud-startup-light-small.png --no-build --transcript empty --cloud-startup connecting --theme paper --size 800x600
```

These fixtures exercise the real renderer without provisioning a cloud VM.
The live Ctrl+R-equivalent rebuild loaded generation 3. Its mapped plugin matched
the rebuilt library digest, with the same process, panel count, editor draft, and
focus retained. Evidence: `target/cloud-live-reload.json`.
