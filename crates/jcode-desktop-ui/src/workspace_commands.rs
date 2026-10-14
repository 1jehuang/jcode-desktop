//! Desktop workspace slash actions. No command is sent as a model prompt.
use super::*;
use crate::panel::workspace_commands::parse::{WorkspaceCommand, parse};
use crate::ui_actions::WorkspaceSlashCommand;

#[cfg(test)]
#[path = "workspace_command_tests.rs"]
mod tests;

impl Workspace {
    pub(crate) fn workspace_slash_command(
        &mut self,
        action: &WorkspaceSlashCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| !slot.closing && slot.panel.entity_id() == action.source)
        else {
            return;
        };
        let Some(command) = parse(&action.content) else {
            return;
        };
        let source = self.slots[index].panel.clone();
        if source.read(cx).is_side_document() || source.read(cx).preview_state.is_some() {
            return;
        }
        cx.stop_propagation();
        self.set_active(index, cx);
        match command {
            WorkspaceCommand::Observe(arg) => {
                self.workspace_live_document(source, true, &arg, window, cx)
            }
            WorkspaceCommand::Mirror(arg) => {
                self.workspace_live_document(source, false, &arg, window, cx)
            }
            WorkspaceCommand::Machines(host) => {
                if let Some(host) = host {
                    self.connect_machine((host != "local").then_some(host), cx);
                } else {
                    self.open_machines(window, cx);
                }
            }
            WorkspaceCommand::Remote(argument) => {
                let args: Vec<String> = argument.split_whitespace().map(str::to_owned).collect();
                if args.is_empty()
                    || (args.len() == 1
                        && matches!(args[0].to_ascii_lowercase().as_str(), "cloud" | "setup"))
                {
                    cx.open_url("https://jcode.sh/account");
                    source.update(cx, |panel, cx| panel.workspace_command_notice("Opened Jcode Cloud account management in your browser. Existing sessions were preserved. `/ssh` opens native machine selection; `/remote status` shows this session server's gateway state.", cx));
                } else if source.read(cx).is_pending_session() {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("Gateway controls require a connected session so they run on the correct server. Start or resume a session first.", cx));
                } else {
                    self.bridge.send(Command::RuntimeCommand {
                        session_id: source.read(cx).session_id.clone(),
                        command: jcode_sdk::RuntimeCommand::Remote,
                        args,
                    });
                }
            }
            WorkspaceCommand::ServerReload => {
                if source.read(cx).is_pending_session() {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("Server reload requires a connected session. No server or Desktop host was restarted.", cx));
                } else {
                    self.bridge.send(Command::RuntimeCommand {
                        session_id: source.read(cx).session_id.clone(),
                        command: jcode_sdk::RuntimeCommand::ServerReload,
                        args: Vec::new(),
                    });
                }
            }
            WorkspaceCommand::Resume { active, query } => {
                self.open_resume(&OpenResume, window, cx);
                if let resume::Snapshot::Open {
                    search,
                    selected,
                    start_on_close,
                    ..
                } = self.resume_snapshot(cx)
                {
                    let mut search = search;
                    search.content = query.clone();
                    search.selection_start = query.len();
                    search.selection_end = query.len();
                    self.restore_resume(
                        Some(resume::Snapshot::Open {
                            search,
                            query,
                            selected,
                            filter: if active {
                                resume::Filter::Active
                            } else {
                                resume::Filter::All
                            },
                            start_on_close,
                        }),
                        false,
                        window,
                        cx,
                    );
                }
            }
            WorkspaceCommand::Catchup(next) => {
                source.update(cx, |panel, cx| panel.workspace_command_notice("Catch Up uses Desktop's session browser. The runtime does not expose an unread-attention queue, so this does not mark sessions read or claim an attention brief.", cx));
                if next {
                    let source_id = &source.read(cx).session_id;
                    let target = self
                        .sessions
                        .iter()
                        .filter(|session| {
                            !session.archived
                                && &session.session_id != source_id
                                && matches!(session.status.as_str(), "idle" | "completed" | "done")
                        })
                        .max_by_key(|session| session.updated_at_ms)
                        .cloned();
                    if let Some(session) = target {
                        let index = self
                            .slots
                            .iter()
                            .position(|slot| {
                                !slot.closing
                                    && slot.panel.read(cx).session_id == session.session_id
                            })
                            .unwrap_or_else(|| self.open_session(session, cx));
                        self.set_active(index, cx);
                        self.focus_active(window, cx);
                    } else {
                        self.open_resume(&OpenResume, window, cx);
                    }
                } else {
                    self.open_resume(&OpenResume, window, cx);
                }
            }
            WorkspaceCommand::Back => {
                if self.single_panel {
                    self.single_panel_back(window, cx);
                } else {
                    self.focus_previous(&FocusPrevious, window, cx);
                }
            }
            WorkspaceCommand::Workspace => {
                if self.single_panel {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("This is a standalone chat window. Open Desktop in workspace mode to use the spatial session overview. This session has not been replaced.", cx));
                } else {
                    self.toggle_overview(&ToggleOverview, window, cx);
                }
            }
            WorkspaceCommand::Quit => window.dispatch_action(Box::new(Quit), cx),
            WorkspaceCommand::Reload => {
                match cx.build_action("jcode_desktop_host::RebuildAndReloadUi", None) {
                    Ok(action) if !std::env::args_os().any(|arg| arg == "--no-hot-reload") => window.dispatch_action(action, cx),
                    _ => source.update(cx, |panel, cx| panel.workspace_command_notice("UI rebuild/reload is unavailable or explicitly disabled in this host. No host or server was restarted.", cx)),
                }
            }
            WorkspaceCommand::Restart => {
                let message = match crate::restart_spawn::spawn_worker(false) {
                    Ok(()) => "Restarting Desktop using its state-preserving restart worker. The shared server is not restarted.".into(),
                    Err(error) => format!("Could not restart Desktop: {error}"),
                };
                source.update(cx, |panel, cx| panel.workspace_command_notice(message, cx));
            }
            WorkspaceCommand::Fork(None) => {
                if source.read(cx).can_fork() {
                    self.bridge.send(Command::Fork {
                        session_id: source.read(cx).session_id.clone(),
                    });
                } else {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("Fork requires a connected conversation with history. The original session is unchanged.", cx));
                }
            }
            WorkspaceCommand::Fork(Some(prompt)) => {
                if source.read(cx).can_fork() {
                    self.bridge.send(Command::ForkWithPrompt {
                        session_id: source.read(cx).session_id.clone(),
                        prompt,
                    });
                } else {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("A side question requires a connected conversation with history. The original session is unchanged.", cx));
                }
            }
            WorkspaceCommand::Transfer => {
                if source.read(cx).can_fork() {
                    self.bridge.send(Command::Transfer {
                        session_id: source.read(cx).session_id.clone(),
                    });
                } else {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("Transfer requires a connected conversation. The source session is unchanged.", cx));
                }
            }
            WorkspaceCommand::Selfdev(argument) => {
                let checkout = source.read(cx).publish_checkout();
                if argument == "status" {
                    let message = checkout.map_or_else(|| "This session is not in a Desktop self-development checkout.".into(), |path| format!("Desktop self-development checkout: {}. /rebuild uses the host's state-preserving UI rebuild action.", path.display()));
                    source.update(cx, |panel, cx| panel.workspace_command_notice(message, cx));
                } else if argument.is_empty() || argument == "enter" {
                    if let Some(path) = checkout {
                        self.open_local_draft(Some(path.to_string_lossy().into_owned()), cx);
                    } else {
                        source.update(cx, |panel, cx| panel.workspace_command_notice("/selfdev requires a local Desktop self-development checkout. No session was replaced.", cx));
                    }
                } else {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("Usage: /selfdev [enter|status]. Prompt arguments are not supported in Desktop.", cx));
                }
            }
            WorkspaceCommand::Publish => {
                if source.read(cx).publish_checkout().is_some() {
                    source.update(cx, |panel, cx| panel.workspace_command_notice("Desktop release aliases use the standard publish tracker, not the TUI's platform-specific fast-release pipeline.", cx));
                    self.publish_desktop(
                        &PublishDesktop {
                            source: action.source,
                        },
                        window,
                        cx,
                    );
                } else {
                    source.update(cx, |panel, cx| {
                        panel.workspace_command_notice(
                            "Release aliases require a local Desktop self-development checkout.",
                            cx,
                        )
                    });
                }
            }
            WorkspaceCommand::Invalid(message) => {
                source.update(cx, |panel, cx| panel.workspace_command_notice(message, cx))
            }
        }
    }

    /// Call once after snapshot slots have been restored. Observers belong to the
    /// new document entities and must not retain the previous UI generation.
    pub(crate) fn restore_workspace_live_views(&mut self, cx: &mut Context<Self>) {
        let documents: Vec<_> = self
            .slots
            .iter()
            .filter_map(|slot| {
                let panel = slot.panel.read(cx);
                let owner = panel
                    .side_document_owner()?
                    .strip_prefix("desktop-live://")?
                    .to_owned();
                let observe = match panel.side_document_page_id()? {
                    "desktop-observe" => true,
                    "desktop-splitview" => false,
                    _ => return None,
                };
                Some((slot.panel.clone(), owner, observe))
            })
            .collect();
        for (document, owner, observe) in documents {
            let source = self
                .slots
                .iter()
                .find(|slot| !slot.closing && slot.panel.read(cx).session_id == owner)
                .map(|slot| slot.panel.clone());
            if let Some(source) = source {
                document.update(cx, |panel, cx| {
                    panel.observe_workspace_source(source, observe, cx)
                });
            }
        }
    }

    fn workspace_live_document(
        &mut self,
        source: Entity<Panel>,
        observe: bool,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(argument, "" | "on" | "off" | "status") {
            source.update(cx, |panel, cx| {
                panel.workspace_command_notice(
                    "Usage: /observe [on|off|status] or /splitview [on|off|status]",
                    cx,
                )
            });
            return;
        }
        let page = source.read(cx).workspace_command_page(observe);
        let owner = format!("desktop-live://{}", source.read(cx).session_id);
        let existing = self.slots.iter().position(|slot| {
            !slot.closing
                && slot.panel.read(cx).side_document_owner() == Some(owner.as_str())
                && slot.panel.read(cx).side_document_page_id() == Some(page.id.as_str())
        });
        let native = cx
            .windows()
            .into_iter()
            .filter_map(|handle| handle.downcast::<panel_window::PanelWindow>())
            .find(|handle| {
                handle.read(cx).is_ok_and(|root| {
                    root.panel.read(cx).side_document_owner() == Some(owner.as_str())
                        && root.panel.read(cx).side_document_page_id() == Some(page.id.as_str())
                })
            });
        let enabled = existing.is_some() || native.is_some();
        if argument == "status" {
            source.update(cx, |panel, cx| {
                panel.workspace_command_notice(
                    format!(
                        "{} is {}. Live Markdown projections have independent scroll positions.",
                        if observe { "Observe" } else { "Chat mirror" },
                        if enabled { "on" } else { "off" }
                    ),
                    cx,
                )
            });
            return;
        }
        let turn_off = argument == "off" || (argument.is_empty() && enabled);
        if turn_off {
            if let Some(index) = existing {
                self.set_active(index, cx);
                self.close_panel(&ClosePanel, window, cx);
            }
            if let Some(handle) = native {
                let _ = handle.update(cx, |root, window, _| root.remove_document(window));
            }
            return;
        }
        if enabled {
            return;
        }
        let bridge = self.bridge.clone();
        let document = cx.new(|cx| {
            let panel = Panel::new_side_document(&owner, &page, bridge, cx);
            cx.observe(&source, move |panel: &mut Panel, source, cx| {
                let page = source.read(cx).workspace_command_page(observe);
                panel.update_side_document(&page, cx);
            })
            .detach();
            panel
        });
        if self.single_panel {
            if let Err(error) =
                panel_window::open_panel_window(document, Some(source.clone()), window, cx)
            {
                source.update(cx, |panel, cx| {
                    panel.workspace_command_notice(
                        format!("Could not open live document: {error}"),
                        cx,
                    )
                });
            }
        } else {
            self.insert_spawned_panel(document, window, cx);
        }
    }
}
