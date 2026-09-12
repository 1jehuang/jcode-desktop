# Desktop onboarding simulator

Press **Alt+9** to open or close a local onboarding rehearsal. The commands
`/onboarding-sim` and `/onboarding-preview` open the same Desktop UI and are
intercepted before a prompt can reach the runtime.

The four stages are Welcome, Connect your AI, Choose a project, and Ready.
Account connection, sign-in failure/retry, and project selection use demo data.
They never read credentials, launch authentication, create a directory, save
configuration, or create a runtime session. This is a simulator, not an automatic
first-run gate or a replacement for the Accounts panel.

Use **Enter** to continue, **Left** to go back, **Escape** to exit, or the visible
Back, Restart, and Exit controls. Skipping account/project setup is supported.
Exiting restores the existing workspace and composer draft. Opening again starts
at Welcome. A native UI reload preserves the current simulator stage and choices.
The workspace's normal panel actions are not mounted during the simulation.
Asynchronous focus restoration stays on the simulator rather than hidden panels.

## Verification

```sh
cargo test -p jcode-desktop-ui onboarding_simulator -- --nocapture
python3 scripts/screenshot.py target/ui-review.png --onboarding-interact
```

The two GPUI tests dispatch the production Alt+9 binding and slash command,
exercise success, error/retry, back, restart, skip, finish, toggle and Escape,
round-trip a real workspace snapshot, and verify that the original draft/layout
is unchanged and no runtime commands were sent.

The native acceptance uses the real application on a private Xvfb display with
isolated offline fixture data. X11 keys and OCR-located mouse clicks walk all four
screens, retry a simulated failure, select the demo project, finish, toggle and
exit. It verifies that configuration bytes are unchanged and no demo directory
was created. Per-stage PNGs and a JSON report are saved beside the screenshot.

The broader dirty-worktree UI suite was also attempted: 834 passed, 22 failed,
and 8 were ignored. Failures concern existing sidebar, panel geometry, activity,
and motion tests outside the simulator. The complete suite is not green, and
those concurrent changes are not included in this feature's commit.
