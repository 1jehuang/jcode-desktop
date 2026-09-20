# Session resume panel

Open the dedicated session browser with `/resume`, `/sessions`, `/session`, or **Alt+R**. It uses a searchable session list and a details pane, with **All**, **Active**, and **Saved** filters.

- Type to search titles, working folders, session IDs, status, and agent labels. Multiple words must all match.
- Use **Up/Down** or **Page Up/Page Down** to select a session, then **Enter** to resume. Clicking a result resumes it directly.
- Use **Ctrl+1**, **Ctrl+2**, and **Ctrl+3** to select the filters.
- **Escape** or **Back** closes the browser without switching conversations. Opening it with Alt+R leaves the composer draft intact.

In standalone `--single-panel` windows, resuming keeps the same single-panel presentation. The resumed conversation becomes the primary chat, so returning from a utility view returns to that conversation. Previously open chats and their drafts remain reachable through the browser.

The browser is a transient workspace surface, not a daemon session. Browsing and canceling do not create a conversation or submit a prompt.

## Verification

Run the focused Rust tests and isolated native acceptance script against a current build:

```sh
cargo test -p jcode-desktop-ui --lib workspace::resume
python3 scripts/verify-resume-panel.py target/resume-panel.png
```

The script runs the real Desktop renderer with offline fixture sessions on private Xvfb displays, exercises both workspace and single-panel presentation, and retains screenshots and navigation evidence. It never controls the live desktop.
