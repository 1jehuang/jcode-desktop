//! Workspace integration for applets: opening instance panels, the built-in
//! showcase applet, local provider discovery, and snapshot restore.
use super::*;
use jcode_applet_types::{Placement, ProviderMessage};

impl Workspace {
    /// Insert a native panel after the active one (or in its own window in
    /// single-panel mode), focus it and animate it in.
    pub(super) fn insert_native_panel(
        &mut self,
        panel: Entity<Panel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.single_panel {
            if let Err(error) = panel_window::open_panel_window(panel, None, window, cx) {
                eprintln!("Could not open panel window: {error:#}");
            }
            return;
        }
        let width_fraction = spawned_panel_width(self.slots.len());
        let insert_at = if self.slots.is_empty() {
            0
        } else {
            self.active + 1
        };
        self.slots.insert(
            insert_at,
            Slot {
                panel,
                row: self.active_row,
                width_fraction,
                animated_width: AnimatedValue::new(
                    width_fraction,
                    transition::policy(Transition::PanelOpen).duration,
                ),
                order_offset: AnimatedValue::new(
                    0.0,
                    transition::policy(Transition::PanelOpen).duration,
                ),
                order_distance_fraction: width_fraction,
                close_progress: AnimatedValue::new(
                    1.0,
                    transition::policy(Transition::PanelClose).duration,
                ),
                closing: false,
                restore_fraction: None,
            },
        );
        crate::sounds::play(crate::sounds::Cue::PanelOpen, cx);
        self.set_active(insert_at, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(super) fn new_applet_panel(
        &mut self,
        instance: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Panel> {
        let panel = cx.new(|cx| Panel::new_applet(instance, self.bridge.clone(), cx));
        cx.subscribe_in(
            &panel,
            window,
            |this, panel, _: &crate::panel::applet_panel::AppletPanelClosed, window, cx| {
                if let Some(index) = this.slots.iter().position(|slot| &slot.panel == panel) {
                    this.set_active(index, cx);
                    this.close_panel(&ClosePanel, window, cx);
                }
            },
        )
        .detach();
        panel
    }

    /// Focus the panel for an instance, opening one if needed.
    pub(crate) fn open_applet_instance(
        &mut self,
        instance: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = format!("{}{instance}", crate::panel::applet_panel::APPLET_PREFIX);
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| !slot.closing && slot.panel.read(cx).session_id == session_id)
        {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            return;
        }
        let panel = self.new_applet_panel(instance.to_owned(), window, cx);
        self.insert_native_panel(panel, window, cx);
    }

    /// Open panels for panel-placed instances that are not shown yet, such as
    /// ones a local provider mounted on startup. Instances the user closed are
    /// removed from the host, so they do not reopen.
    pub(crate) fn sync_applet_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let runtime = crate::applet_runtime::get(cx);
        runtime.pump();
        let wanted: Vec<String> = runtime
            .host
            .borrow()
            .at(|placement| matches!(placement, Placement::Panel { .. }))
            .map(|mounted| mounted.instance.id.clone())
            .collect();
        for instance in wanted {
            let session_id = format!("{}{instance}", crate::panel::applet_panel::APPLET_PREFIX);
            let shown = self
                .slots
                .iter()
                .any(|slot| slot.panel.read(cx).session_id == session_id);
            if !shown {
                self.open_applet_instance(&instance, window, cx);
            }
        }
    }

    /// Offline screenshot fixture: mount the showcase beside the fixture chat.
    /// Runs during construction, before a window is available.
    pub(super) fn open_applet_fixture(&mut self, cx: &mut Context<Self>) {
        let runtime = crate::applet_runtime::get(cx);
        for message in showcase_messages() {
            runtime
                .apply(SHOWCASE_ID, message)
                .expect("showcase is valid");
        }
        let panel =
            cx.new(|cx| Panel::new_applet(SHOWCASE_INSTANCE.into(), self.bridge.clone(), cx));
        let width_fraction = spawned_panel_width(self.slots.len());
        self.slots.push(Slot {
            panel,
            row: self.active_row,
            width_fraction,
            animated_width: AnimatedValue::new(width_fraction, Duration::ZERO),
            order_offset: AnimatedValue::new(0.0, Duration::ZERO),
            order_distance_fraction: width_fraction,
            close_progress: AnimatedValue::new(1.0, Duration::ZERO),
            closing: false,
            restore_fraction: None,
        });
        self.active = self.slots.len() - 1;
    }

    pub(super) fn open_applet_showcase(
        &mut self,
        _: &OpenAppletShowcase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let runtime = crate::applet_runtime::get(cx);
        for message in showcase_messages() {
            if let Err(error) = runtime.apply(SHOWCASE_ID, message) {
                eprintln!("jcode desktop: showcase applet rejected: {error}");
            }
        }
        self.open_applet_instance(SHOWCASE_INSTANCE, window, cx);
    }

    /// Start autostart providers from `~/.jcode/applets`. Safe to call on
    /// every activation: running providers are left alone.
    pub(crate) fn start_local_applets(&mut self, cx: &mut Context<Self>) {
        crate::applet_runtime::get(cx).restore_persistent();
        if crate::harness::screenshot_mode() || cfg!(test) {
            return;
        }
        let Some(dir) = crate::applet_runtime::applets_dir() else {
            return;
        };
        let runtime = crate::applet_runtime::get(cx);
        for applet in crate::applet_runtime::discover(&dir) {
            if applet.autostart
                && let Err(error) = runtime.start(&applet)
            {
                eprintln!("jcode desktop: {error}");
            }
        }
    }
}

pub(crate) const SHOWCASE_ID: &str = "jcode.showcase";

impl Workspace {
    /// Tell providers whose manifests claim a tool call that it started or
    /// finished, so they can mount a card anchored to it.
    pub(super) fn forward_tool_call(
        &mut self,
        session_id: &str,
        event: &jcode_sdk::ApiEvent,
        cx: &mut Context<Self>,
    ) {
        use jcode_sdk::ApiEvent;
        let (call_id, tool, output, error, done) = match event {
            ApiEvent::ToolExec { call_id, name, .. } => (call_id, name, None, None, false),
            ApiEvent::ToolDone {
                call_id,
                name,
                output,
                error,
                ..
            } => (call_id, name, Some(output.clone()), error.clone(), true),
            _ => return,
        };
        let claimed = crate::applet_runtime::get(cx)
            .host
            .borrow()
            .manifests()
            .any(|manifest| manifest.tool_cards.iter().any(|claim| &claim.tool == tool));
        if !claimed {
            return;
        }
        let input = self
            .slots
            .iter()
            .map(|slot| slot.panel.read(cx))
            .find(|panel| panel.session_id == session_id)
            .and_then(|panel| panel.tool_input(call_id))
            .and_then(|input| serde_json::from_str(input).ok())
            .unwrap_or(serde_json::Value::Null);
        crate::applet_runtime::get(cx).notify_tool_call(
            jcode_applet_types::HostMessage::ToolCall {
                session_id: session_id.to_owned(),
                call_id: call_id.clone(),
                tool: tool.clone(),
                input,
                output,
                error,
                done,
            },
        );
    }

    /// Deliver queued applet work that needs the workspace: agent intents
    /// to the SDK, new chats and prompts, and toasts.
    pub(crate) fn drain_applet_work(&mut self, cx: &mut Context<Self>) {
        let runtime = crate::applet_runtime::get(cx);
        let outbound = std::mem::take(&mut *runtime.agent_outbox.borrow_mut());
        let effects = std::mem::take(&mut *runtime.effects.borrow_mut());
        let toasts = std::mem::take(&mut *runtime.toasts.borrow_mut());
        for message in outbound {
            let (session_id, operation) = match message {
                crate::applet_runtime::AgentOutbound::Action {
                    session_id,
                    instance,
                    action,
                    state,
                    source_key,
                } => (
                    session_id,
                    harness::SessionOperation::AppletAction {
                        instance,
                        action,
                        state,
                        source_key,
                    },
                ),
                crate::applet_runtime::AgentOutbound::Close {
                    session_id,
                    instance,
                } => (session_id, harness::SessionOperation::CloseApplet(instance)),
            };
            self.bridge.send(Command::SessionOperation {
                session_id,
                operation,
            });
        }
        for effect in effects {
            match effect {
                crate::applet_host::Effect::StartChat { prompt } => {
                    self.open_new_session(cx);
                    if !prompt.trim().is_empty()
                        && let Some(slot) = self.slots.get(self.active)
                    {
                        slot.panel.update(cx, |panel, cx| {
                            panel.submit_or_queue(prompt, Vec::new(), true, cx)
                        });
                    }
                }
                crate::applet_host::Effect::SendPrompt { session_id, prompt }
                    if !prompt.trim().is_empty() =>
                {
                    let target = if session_id.is_empty() {
                        self.slots
                            .get(self.active)
                            .filter(|slot| slot.panel.read(cx).supports_voice())
                    } else {
                        self.slots
                            .iter()
                            .find(|slot| slot.panel.read(cx).session_id == session_id)
                    };
                    match target {
                        Some(slot) => slot.panel.update(cx, |panel, cx| {
                            panel.submit_or_queue(prompt, Vec::new(), true, cx)
                        }),
                        None if !session_id.is_empty() => self.bridge.send(Command::Send {
                            session_id,
                            content: prompt,
                            images: Vec::new(),
                        }),
                        None => {}
                    }
                }
                _ => {}
            }
        }
        for (text, _) in toasts {
            self.applet_toast = Some((text, Instant::now()));
        }
        cx.notify();
    }

    /// Fire a manifest launcher. A running singleton is focused instead.
    pub(crate) fn fire_applet_launcher(
        &mut self,
        applet: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = crate::applet_runtime::get(cx).launch(applet, index);
        if let Some(instance) = focus {
            let placement = crate::applet_runtime::get(cx)
                .host
                .borrow()
                .instance(&instance)
                .map(|mounted| mounted.instance.placement.clone());
            if matches!(placement, Some(Placement::Panel { .. })) {
                self.open_applet_instance(&instance, window, cx);
            }
        }
        cx.notify();
    }
}

/// Called on every UI activation (including Ctrl+R). Starts autostart local
/// providers and keeps panel-placed instances visible as providers mount them.
/// The loop only notifies when the shared runtime changed.
pub(crate) fn install(workspace: &Entity<Workspace>, window: &mut Window, app: &mut App) {
    workspace.update(app, |workspace, cx| workspace.start_local_applets(cx));
    if crate::harness::screenshot_mode() || cfg!(test) {
        return;
    }
    let weak = workspace.downgrade();
    window
        .spawn(app, async move |cx| {
            let mut seen = u64::MAX;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let Some(workspace) = weak.upgrade() else {
                    break;
                };
                let result = cx.update(|window, app| {
                    let runtime = crate::applet_runtime::get(app);
                    runtime.pump();
                    let generation = runtime.generation.get();
                    let pending = !runtime.agent_outbox.borrow().is_empty()
                        || !runtime.effects.borrow().is_empty()
                        || !runtime.toasts.borrow().is_empty();
                    if pending {
                        workspace.update(app, |workspace, cx| workspace.drain_applet_work(cx));
                    }
                    if generation != seen {
                        seen = generation;
                        crate::applet_runtime::get(app).persist();
                        workspace.update(app, |workspace, cx| {
                            workspace.sync_applet_panels(window, cx);
                            // Transcript, sidebar and overlay placements
                            // render from the runtime, so repaint them.
                            for slot in &workspace.slots {
                                slot.panel.update(cx, |_, cx| cx.notify());
                            }
                            cx.notify();
                        });
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
}
pub(crate) const SHOWCASE_INSTANCE: &str = "jcode.showcase#main";

/// A built-in applet that exercises every component. It doubles as living
/// documentation and as the offline screenshot fixture.
pub(crate) fn showcase_messages() -> Vec<ProviderMessage> {
    use base64::Engine;
    let png = include_bytes!("../../../assets/previews/image-preview.png");
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let manifest = serde_json::json!({
        "schema": jcode_applet_types::SCHEMA,
        "id": SHOWCASE_ID,
        "title": "Applet showcase",
        "description": "Every built-in applet component",
        "capabilities": ["open_url", "clipboard"],
    });
    // Kept as JSON: it is also the reference example for provider authors.
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("applet_showcase.json")).expect("showcase json");
    document["assets"][0]["data"] = serde_json::Value::String(data);
    if let Ok(tab) = std::env::var("JCODE_DESKTOP_SCREENSHOT_APPLET_TAB")
        && crate::harness::screenshot_mode()
    {
        document["state"]["tab"] = serde_json::Value::String(tab);
    }
    vec![
        ProviderMessage::Register {
            manifest: serde_json::from_value(manifest).expect("showcase manifest"),
        },
        ProviderMessage::Mount {
            instance: SHOWCASE_INSTANCE.into(),
            placement: Placement::Panel {
                open: Default::default(),
            },
            scope: Default::default(),
            lifetime: jcode_applet_types::Lifetime::Session,
            document: Box::new(serde_json::from_value(document).expect("showcase document")),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn showcase_is_valid_and_exercises_every_known_component() {
        let mut host = crate::applet_host::AppletHost::new();
        for message in showcase_messages() {
            host.apply(SHOWCASE_ID, message).unwrap();
        }
        let mounted = host.instance(SHOWCASE_INSTANCE).unwrap();
        let json = serde_json::to_string(&mounted.instance.document.view).unwrap();
        for kind in jcode_applet_types::view::KNOWN_TYPES {
            if matches!(
                *kind,
                "html" | "scroll" | "card" | "spacer" | "divider" | "empty"
            ) {
                continue;
            }
            assert!(
                json.contains(&format!("\"type\":\"{kind}\"")),
                "showcase lacks {kind}"
            );
        }
    }
}
