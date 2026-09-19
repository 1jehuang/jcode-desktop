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

The native fixtures use an inert bridge. Actual queued delivery is checked with
recorded bridge commands in the GPUI tests, not a live provider request. Cloud
credentials and remote availability are not repaired or bypassed by this change.

An earlier full-suite run had 996 passes, 9 ignored tests, and 8 failures in
changelog identity, panel surface, pull-preview, and swipe tests. A baseline build
comparison was cancelled to avoid delaying delivery, so those failures are not
claimed to be pre-existing or resolved. The final focused suite above is green.
