# Swarm and Worktrees

The chat sidebar has a **Swarm / Worktrees** switch. Swarm is the default and keeps the existing shared-checkout agent hierarchy. Worktrees groups sessions by Git checkout for the active local project. The selected view survives workspace restoration and hot reload.

Switching views does not move sessions, change branches, or reconfigure running swarm agents. A swarm can still run inside a worktree.

## Open a checkout

1. Open a session in a local Git project, or choose that project as the default directory.
2. Select **Worktrees** in the left sidebar.
3. Click a branch to focus an open conversation in that checkout, or start a new conversation there. Existing conversations also appear below their checkout.
4. Use **Refresh** after adding or changing worktrees outside Desktop.

The list includes the main checkout, linked worktrees, detached HEADs, and Git's locked or unavailable entries. Unavailable and bare entries cannot start a session. Paths on SSH sessions are never treated as local paths. Remote worktree management is not supported yet.

## Create an isolated branch

Select **+ New worktree**, type a new branch name, and press **Enter**. Press **Esc** to cancel before creation starts.

Jcode creates a new branch from the active checkout's committed **HEAD**, then opens a local session in the new checkout. Staged, unstaged, and untracked changes remain in the original checkout. Existing branches and destination directories are never overwritten or reused.

Checkouts live beside the main repository in `<repository-name>-worktrees/<encoded-branch-name>`. Branch separators and other special characters are encoded to avoid directory-name collisions. For example, `feature/search` uses `feature%2Fsearch`.

Closing a session or switching to Swarm does not delete its worktree. Merge, remove, and prune worktrees with Git when you are ready. Desktop intentionally performs no automatic cleanup or force deletion.

## Development checks

- Shared Git operations: `cargo test -p jcode-sdk` in the Jcode repository.
- Sidebar workflow: `cargo test -p jcode-desktop-ui sidebar_worktrees --lib`.
- Offline visual fixture: `python3 scripts/screenshot.py target/ui-review.png --worktrees`.
