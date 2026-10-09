## What's new

### Jcode Desktop 0.5.1

An account menu, drag and drop, self-healing turns, and a faster workspace. This is the first public build of the 0.5 line, since 0.5.0 never cleared its FreeBSD release gate.

#### Themes

- An in-app account menu from the sidebar account pill, with usage, monthly limit, billing, and plan changes in place.
- Drag files into a chat, and turns that fail transiently continue on their own.
- A faster workspace: retained GPUI rendering, smoother transcript streaming, and much less work per frame.

#### Highlights

- Upgrade plans through Stripe Checkout, which detects the new plan automatically, and sign in again instead of signing out.
- Move a running tool to the background with Alt+B, Ctrl+B, or a Background pill.
- Pin sessions from the sidebar. Running sessions show a live working timer, and hovering swaps the spinner for pin and close.
- Composer example prompts are personalized from the open todos of closed sessions, skipping blocked and wrap-up items.
- A native-feeling Windows caption that shares the tab row.

#### Improvements

- Panel status and build info move into the composer pill row, removing the bottom bar. Fresh sessions start with a single-row composer.
- Subscription limits live in the method pill hover card instead of the composer row.
- Switching providers picks their newest flagship model, not the alphabetically first route.
- The sidebar keeps the account pill pinned to the bottom, shows the Jcode account instead of a provider login, and hides its email by default.
- The change review file tree is restyled like lazygit.
- Clicking the version pill checks for and applies updates. Hot reloads keep your place.

#### Fixes

- CJK IME composition no longer crashes Desktop.
- Older macOS builds auto-update again.
- A new thinking block starts when reasoning resumes after streamed text.
- The transcript tail glides smoothly for every kind of growth, including very fast streams.
- Windows uses an 8 MiB main-thread stack.
- The welcome screen uses the same docked composer size as a chat.
- FreeBSD builds again. The opt-in codemode tool is unavailable there.

Downloads are available at https://jcode.sh/desktop after all platform builds and public download checks pass.

## Previous releases

### Jcode Desktop 0.4.0

Applets, a multi-account workspace, and a smoother composer

#### Themes

- Applets: sandboxed custom UI that agents and bundled integrations can place inline, in panels, as overlays, or as tool call cards.
- Accounts become a first-class workspace, with every login, its limits and usage, auto-switch ordering, and banked resets in one place.
- A redesigned composer, sidebar, and model picker built from pills, with smoother streaming text and less UI-thread work.

#### Highlights

- Bundled Gmail, GitHub issues and pull requests, and Google Sheets applets, plus native cards for Gmail and GitHub MCP tool calls.
- Account rows show limits, usage, and names. Drag logins between Auto-switch and Manual, test them live, add more OAuth accounts, and redeem banked resets.
- Model picker with maker logos, auth pills, and typo-tolerant ranked search. The method pill switches providers inline.
- Jcode Cloud sessions use your local model, logins, and voice, and show where they run.
- Onboarding with inline email sign-in that continues into a real panel, a wider live demo, and theme preview on hover.

#### Improvements

- Composer pills sit below the input with an in-box voice button. Space on an empty composer jumps to the latest message.
- Sidebar with live and past labels, a project dock, full-width headers, and a section menu beside the sidebar.
- Tabs show a color-coded todo progress ring and fade long titles. A next-workspace pill supports mouse navigation.
- Resume picker searches transcripts by keyword and surfaces crashed sessions.
- Streamed text reveals whole words and glides onto new lines instead of jumping.
- `/save [label]` and `/unsave` name sessions. Voice holds reach a focused Jcode CLI session, and every voice status is a pill.
- Stored transcripts paint immediately while the runtime attaches. Daemon-reported KV cache misses appear in the transcript.

#### Fixes

- Much less UI-thread work while typing, from catalog rebroadcasts, and from decorative animations.
- Runtime info backlogs coalesce instead of growing update queues to gigabytes.
- Global voice holds survive kernel key-event drops during UI stalls and in-place host rebuilds.
- Stable list scrollbars across resizes, and markdown hard line breaks and paragraph spacing render correctly.
- macOS auto-update is validated from every published build.

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
