# Recent-crash recovery

Launching Jcode Desktop after an unexpected exit automatically restores the last
workspace checkpoint when its owner is no longer running and the checkpoint is
at most 30 seconds old. A normal application quit removes the checkpoint, so
ordinary launches still start fresh. Launching while the app is already running
continues to show its existing window.

The UI checkpoints once per second. This bounds the usual unsaved layout/draft
window to about one second. The 30-second cutoff uses the last successful
checkpoint, not an OS-provided crash timestamp. A blocked UI or failed disk write
can make the checkpoint older, in which case recovery conservatively starts fresh.

Restoration preserves session panels, order, rows, widths, active panel, drafts,
and transcript scroll position through the existing workspace snapshot format.
Session histories reconnect to the runtime. Prompts are not resubmitted. Terminal
panels start new PTYs in their saved directories because host resource IDs cannot
survive a process exit.

Checkpoints live beside the learning state:

- `$XDG_STATE_HOME/jcode-desktop/crash-recovery.json`, when configured.
- `~/.local/state/jcode-desktop/crash-recovery.json` on Linux by default.
- `~/Library/Application Support/Jcode/crash-recovery.json` on macOS by default.

`--no-sidebar` and `--workspace` share a separate
`crash-recovery-no-sidebar.json`, matching their separate single-instance socket.
Files contain draft content and use owner-only permissions on Unix. Checkpoints
are atomically replaced under a file lock. A generation token prevents a retired
hot-reload worker or its quit callback from overwriting a newer generation.
Recovery is conservatively disabled for foreign processes on non-Unix platforms,
where process liveness detection has not yet been implemented.

Ctrl+R installs checkpointing into an already-running development host without a
restart. Existing in-memory hot-reload snapshots take precedence over disk
recovery. A launch cannot recover a crash that happened before a checkpoint-capable
UI was installed.

## Verification

```sh
cargo test -p jcode-desktop-ui --lib workspace::recovery -- --test-threads=1
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/accept-crash-recovery.py --help
```

The acceptance script uses an isolated home, runtime directory, and private Xvfb
display, never the user's running app or sessions.
