# Compact worktree sidebar

## Reference and scope

Codex Desktop was not installed on the Linux development machine. The only
Codex launcher was a stale CLI launcher. The reference was therefore OpenAI's
published [project sidebar illustration](https://learn.chatgpt.com/docs/projects)
and [worktree documentation](https://learn.chatgpt.com/docs/environments/git-worktrees),
not an inspection of a running Codex app.

The adaptation keeps Jcode's existing explicit Git checkout model. It does not
add Codex's managed-worktree cleanup, handoff, or detached-thread lifecycle.

## Behavior

- A compact repository heading replaces the explanatory paragraph.
- Each checkout has a 30px header with a folder/branch icon. The primary checkout
  is labeled Local. Full branch names and paths are available on hover.
- The chevron collapses only that checkout's chats, without opening a session or
  changing the active checkout. Collapsed groups show their chat count.
- The header navigates to an existing panel in that checkout, or opens a local
  draft. The separate `+` always opens a new local chat, even with an SSH default.
- Chats are indented 28px rows, sorted using the existing saved/recency policy.
  They show the live panel title and activity, compact recency, and a subtle
  selected background. No decorative rails or accent bars are used.
- New local drafts appear before the backend replies. Archived, remote, and
  unrelated-directory history is excluded. Unavailable checkouts cannot open
  sessions. A Git worktree lock does not prevent chats.
- Known checkout navigation preserves the list and collapse state. Unknown
  directories are still queried through Git, including possible nested repos.
- Git creation, refresh, validation, cancellation, and Swarm mode are retained.
  No checkout or branch is removed, reset, or automatically cleaned up.

## Verification

```sh
cargo test -p jcode-desktop-ui sidebar_worktrees --lib
python3 scripts/screenshot.py target/ui-review.png --worktrees
```

The tests cover compact geometry, collapse without navigation, local new-chat
routing, immediate draft visibility, chat selection and reuse, recency/filtering,
unavailable checkouts, remote isolation, mode persistence, creation cancellation,
and real Git creation while preserving dirty files in the original checkout.

Observed on 2026-09-18:

- All 13 worktree tests passed. The broader `sidebar` test filter passed 90
  tests with two existing ignored tests.
- Inspected `target/worktree-sidebar-review.png` (warm neutral) and
  `target/worktree-sidebar-light-wide.png` (neutral light), both at 1440×1000.
  The 1000×700 capture correctly uses the existing compact navigation rail.
  The requested `target/ui-review.png` was already owned by another capture,
  so the final images use unique paths instead of overwriting it.
- The running desktop's Ctrl+R-equivalent `--reload-ui` action rebuilt the
  release plugin and activated UI generation 5 without restarting the host.
  Evidence is in `target/worktree-sidebar-live-reload.log`.
