## What's new

### Jcode Desktop 0.3.0

Clearer navigation and richer session feedback

#### Themes

- Clearer navigation for sessions and swarms, richer document panels, and more visible response progress.
- Desktop-wide update visibility and a release workflow with native-build and installation checks.

#### Highlights

- Nested swarm navigation with clearer indentation, stable session selection, and bounded sidebar rendering.
- Native Thinking Orbs, eagerly streamed tool names and arguments, and clear explanations for interrupted or failed responses.
- A CLI-style resume panel, numbered prompt cards, compact queued prompts, and Shift+Space to return to the latest output.

#### Improvements

- Improved Markdown and PDF panels, text selection, and inline images.
- Workspace-wide version and update status.
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
