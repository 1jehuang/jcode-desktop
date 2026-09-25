## What's new

### Jcode Desktop 0.3.3

Voice everywhere, a redesigned first launch, and a project-aware sidebar

#### Themes

- Hold-to-talk voice works even when Desktop is unfocused, with an OS-level pill that shows what was heard and sent.
- A redesigned first launch with inline email sign-in, login import, and a live chat replay.
- A sidebar organized by project, with branches, worktrees, and swarm threads nested inside.

#### Highlights

- Global voice with a compact audio-reactive pill, a stop-and-send button, and Shift+Copilot to open a new voice window.
- Split-page onboarding: inline email code sign-in, per-row login import, theme hover preview, and pill buttons throughout.
- Sidebar sessions grouped by Git project, with running daemon sessions spinning even without an open panel.
- A redesigned composer with model, method, effort, and voice pills, a location line, and typewriter example prompts.
- Single-panel windows share one host process, open sessions directly with `--session=<id>`, and fork into a new window with Super+Space.
- New Glass, Pure Black, Dark Neutral, Light Neutral, and ChatGPT Light themes.

#### Improvements

- An orchestration panel with live sessions, running state, and todos.
- Gmail tool calls render as email cards for drafts, sends, reads, and threads.
- A resume picker with real conversation previews, `/save` labels, and markers for sessions working now or open in Desktop.
- The footer shows a context ring gauge, API session cost, and live tool elapsed time.
- Queued prompts have send-now and recall actions. Long prompts and pinned reminders collapse by default.
- Merged edits, unified-diff patches, and replacements render as diffs. Code, tool output, tasks, and documents are selectable.
- Accounts can redeem banked OpenAI and Claude usage resets after a confirmed review. The Cloud button connects to managed Jcode Cloud.

#### Fixes

- Session reconnects back off instead of retrying every 300 ms, and a dead harness API bridge is respawned.
- Lower memory use: fewer per-window threads, retained heap returned, and hot-reload copies staged off tmpfs.
- Live transcripts, drafts, and the resume picker survive Desktop UI reloads.
- Transcript follow resumes after scrolling back to the bottom, and selection no longer drags past text bounds.

### Jcode Desktop 0.3.1

Clearer navigation and richer session feedback

#### Themes

- Clearer navigation for sessions and swarms, richer document panels, and more visible response progress.
- Desktop-wide update visibility and a release workflow with native-build and installation checks.

#### Highlights

- Compact window chrome, immediately visible live sidebar sessions, and tighter adjacent session rows.
- Background tasks appear as compact inline status rows, with clearer shortcut pictograms and special-key icons.
- Corrected bundled-runtime build for the native release pipeline. Version 0.3.1 supersedes the immutable 0.3.0 tag, whose native runtime compilation failed.
- Nested swarm navigation with clearer indentation, stable session selection, and bounded sidebar rendering.
- Native Thinking Orbs, eagerly streamed tool names and arguments, and clear explanations for interrupted or failed responses.
- A CLI-style resume panel, numbered prompt cards, compact queued prompts, and Shift+Space to return to the latest output.

#### Improvements

- Improved Markdown and PDF panels, text selection, and inline images.
- Workspace-wide version and update status. Development builds retain the intended release version, such as `0.3.1-dev.12`, instead of turning the Git commit count into a misleading patch number.
- Personal cloud model-access synchronization before connecting, with fresh readiness checks and fail-closed recovery. This remains an experimental Unix-only integration.
- A resumable, single-command release workflow with verified native builds, signed macOS packages, website downloads, and installation acceptance.

#### Fixes

- Reliable jump-to-latest behavior.
- Development builds retain the intended release version, such as `0.3.0-dev.12`, instead of turning the Git commit count into a misleading patch number.

Downloads are available at https://jcode.sh/desktop after all platform builds and public download checks pass.

## Previous releases

### Jcode Desktop 0.2.1

- Native Markdown and PDF document panels with explicit open, update, and close behavior.
- An integrated Handterm terminal with text selection, clipboard support, image scrollback, and Desktop theme colors.
- Native streaming voice dictation with live transcription, editable drafts, a global voice shortcut, and recent-session voice navigation. Nari credentials are required.
- Optional first-launch account sign-in and a dedicated accounts panel for `/login`.
- Swarm and Git worktree sidebar workflows, compact session navigation, and tab actions beside titles.
- Independent single-panel windows, session image panes, numbered collapsible workspace groups, and contextual edit previews.
- Startup prompts can queue while connecting. Remote panels show connection progress and recovery actions without stealing local focus.
- Personal cloud alpha panels show startup checks, runtime allowance, and bounded readiness refreshes. The cloud integration remains experimental and its local helper requires Unix.
- Clearer version labels and update history, new Graphite, Slate, Paper, and Silver themes, and improved transcript and footer layout.

Downloads are available at https://jcode.sh/desktop after all platform builds and public download checks pass.

## Previous update

- Compact session tabs keep the FPS indicator and new-session button together.
- Untitled sessions start with the clearer “New session” label.
- Offline update notes open automatically after Desktop updates and hot reloads.
- Use `/changelog` to reopen the notes. Press Esc or choose Close to dismiss them.

Development builds and explicit hot-reload sessions additionally report uncommitted files captured at build time.
