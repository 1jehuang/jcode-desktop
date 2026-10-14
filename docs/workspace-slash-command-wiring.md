# Workspace slash-command wiring

Owned implementation files: `panel_workspace_commands.rs`, `workspace_command_parse.rs`, `workspace_commands.rs`, and `workspace_command_tests.rs`. `ui_actions.rs` adds the leaf `WorkspaceSlashCommand { source, content }` action. No panel/input dependency on workspace was introduced.

## Exact integration

1. In `panel.rs`: `#[path = "panel_workspace_commands.rs"] pub(crate) mod workspace_commands;`.
2. At the beginning of the existing `PromptInput::new_with_queue` submit callback, before `is_resume_command` and before pending-session queuing:
   ```rust
   if images.is_empty()
       && let Some(panel) = weak.upgrade()
       && Panel::dispatch_workspace_command(&panel, &content, _window, app)
   {
       return;
   }
   ```
   This intentionally supersedes the old resume-only shortcut, retaining `/active` filters and search arguments. Preview and read-only panels continue to use their existing handlers.
3. In `workspace.rs`: `#[path = "workspace_commands.rs"] mod commands;`.
4. Register `.on_action(cx.listener(Self::workspace_slash_command))` in BOTH the workspace render root and `workspace_single_panel.rs` render root.
5. At the end of `Workspace::apply_snapshot`, after restored slots exist, call `self.restore_workspace_live_views(cx);` once. This reattaches live projection observers to the new source panel entities.

The coordinator has wired items 1 through 4. The worker added the approved, narrowly scoped item 5 in `workspace.rs`. Mixed-owner dirty files must be committed by the coordinator, not staged wholesale by this worker.

## Behaviors

- `/observe [on|off|status]` opens a live read-only latest-tool document, including input/output/error. `/splitview` and `/split-view` open a live Markdown projection of the conversation, with independent document scrolling. Source notifications update the document without sharing its composer or runtime connection. Projections have synthetic ownership so runtime side-panel updates cannot delete them.
- `/btw <question>`, `/fork <prompt>`, and `/split <prompt>` use `Command::ForkWithPrompt`, with the source session's own runtime transport. Bare `/fork` and `/split` use `Command::Fork`. The resulting `SessionForked` lifecycle opens the new conversation; the source is not replaced.
- `/transfer` uses native `Command::Transfer`, not a model prompt. The harness worker implements the true SDK transfer lifecycle and emits `SessionForked` for the new destination.
- `/ssh [host]` opens native machine selection or a validated remote draft. `/ssh local` opens a separate local draft. Existing sessions are not converted to a different host.
- `/remote`, `/remote cloud`, and `/remote setup` open Jcode Cloud account management. Other gateway subcommands use `RuntimeCommand::Remote` on the source session's server, reusing shared status/on/off/pair/revoke controls. `/server-reload` uses `RuntimeCommand::ServerReload`, requesting the existing server reload path with `force: false`. It reports request acknowledgment, not completion, and never restarts the Desktop host.
- `/resume [query]` opens the native session browser. `/active [query]` also selects its active filter.
- `/catchup [list]` opens that browser. `/catchup next` opens the most recently updated idle/completed other session when present. This is explicitly a history-based equivalent, NOT a fabricated unread-attention queue or generated brief.
- `/back` uses focus-previous in a workspace and the existing standalone back behavior in a single-panel window. `/workspace` opens/toggles the spatial overview without changing launch mode.
- `/reload`, `/rebuild`, and `/client-reload` dispatch the running host's state-preserving rebuild/reload action, when hot reload is enabled. `/restart` invokes the existing detached Desktop-only restart worker only when the user submits that command. This implementation task does not invoke it. `/quit` uses the existing app quit action.
- `/selfdev [enter|status]` opens a separate local draft in the detected Desktop checkout or reports checkout identity.
- Release aliases currently use the existing Desktop publish tracker and explicitly announce this substitution. Desktop has no separate fast-macOS/remote-release publish kinds. They do not claim the TUI-specific pipeline was executed.

## Limitations

Gateway configuration changes may require an explicit server reload before the listener changes. Server reload is a non-forced request: the existing runtime can decline to replace a server when there is no newer reload candidate. Neither the command nor its acknowledgment claims that a new binary is running.

Mirrors are Markdown projections, not pixel-perfect transcript widgets: image rows are represented by a placeholder and rich applet/background rows are not mirrored. Native standalone document windows can be opened/closed, but reattaching observers across independently restored native windows may require an additional panel-window restore hook. Workspace-slot projections are reattached by the documented hook.

No command has a generic model-prompt fallback. No running host was forcibly restarted, no source conversation was cleared, and no external release was executed during implementation.
