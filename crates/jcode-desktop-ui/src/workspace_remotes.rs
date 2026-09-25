//! Machine selection is a workspace preference, not a mutation of existing panels.
use super::*;

/// Reserved managed destination, never an SSH alias or the personal AWS alpha.
pub(super) const MANAGED_CLOUD_HOST: &str = "jcode-cloud";
pub(super) const MANAGED_CLOUD_STARTING: &str = "Checking your Jcode Cloud machine…";

fn managed_destination(host: &str) -> bool {
    matches!(host, MANAGED_CLOUD_HOST | cloud_alpha::HOST)
}

pub(super) fn new_session_host(host: &str) -> &str {
    if managed_destination(host) {
        MANAGED_CLOUD_HOST
    } else {
        host
    }
}

#[path = "workspace_cloud_alpha.rs"]
mod cloud_alpha;

#[path = "workspace_cloud_progress.rs"]
mod cloud_progress;

fn live_cloud_panel(
    closing: bool,
    session_id: &str,
    history_loaded: bool,
    pending: bool,
    connected: bool,
) -> bool {
    !closing
        && history_loaded
        && !pending
        && connected
        && session_id
            .strip_prefix("ssh://jcode-cloud-alpha/")
            .is_some_and(|id| !id.is_empty())
}

pub(super) struct HeaderTooltip(pub gpui::SharedString);

impl Render for HeaderTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(Theme::global().PANEL_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_size(px(12.0))
            .text_color(Theme::global().TEXT)
            .child(self.0.clone())
    }
}

#[derive(Default)]
pub(super) struct Machines {
    pub default_host: Option<String>,
    pub hosts: Vec<String>,
    pub status: Option<String>,
    pub failed: bool,
    pub startup_failed: bool,
    startup_requested: bool,
    pub(super) input: Option<Entity<PromptInput>>,
    notice: Option<String>,
    discovery: Option<gpui::Task<()>>,
    cloud: cloud_alpha::Lifecycle,
    connected_cloud_sessions: std::collections::HashSet<String>,
    cloud_progress: std::collections::HashMap<String, cloud_progress::Progress>,
    cloud_monitor: Option<gpui::Task<()>>,
}

impl Machines {
    pub fn from_config() -> Self {
        let config = &crate::config::get().workspace;
        let mut hosts = config.remote_hosts.clone();
        if let Some(host) = &config.default_remote_host {
            if !hosts.contains(host) {
                hosts.push(host.clone());
            }
        }
        Self {
            default_host: config
                .default_remote_host
                .as_deref()
                .map(new_session_host)
                .map(str::to_owned),
            hosts,
            ..Self::default()
        }
    }
}

impl Workspace {
    pub(super) fn set_cloud_transport_connected(&mut self, session_id: &str, connected: bool) {
        if harness::remote_host(session_id).as_deref() != Some(cloud_alpha::HOST) {
            return;
        }
        if connected {
            self.remotes
                .connected_cloud_sessions
                .insert(session_id.to_owned());
        } else {
            self.remotes.connected_cloud_sessions.remove(session_id);
        }
    }

    pub(super) fn invalidate_cloud_connection(&self, host: &str) {
        if host == cloud_alpha::HOST {
            self.remotes.cloud.invalidate_ready();
        }
    }

    /// Connection progress belongs beside the draft, not only in Machines.
    /// A stale remote result must not change a draft explicitly moved locally.
    pub(super) fn update_startup_status(
        &mut self,
        message: &str,
        failed: bool,
        cx: &mut Context<Self>,
    ) {
        self.remotes.startup_failed = failed;
        for slot in &self.slots {
            if !slot.closing && slot.panel.read(cx).is_startup_draft() {
                slot.panel.update(cx, |panel, cx| {
                    panel.status = message.to_owned();
                    cx.notify();
                });
            }
        }
    }

    pub(super) fn update_pending_remote_status(
        &mut self,
        request_id: Option<&str>,
        message: &str,
        failed: bool,
        cx: &mut Context<Self>,
    ) {
        // Keep only live pending requests. Completed, closed, retried and local
        // replacement panels must never inherit another request's checklist.
        self.remotes.cloud_progress.retain(|id, _| {
            self.slots.iter().any(|slot| {
                !slot.closing
                    && slot.panel.read(cx).is_pending_session()
                    && slot.panel.read(cx).session_id == *id
            })
        });
        if let Some(id) = request_id {
            let cloud = pending::remote_draft_host(id).is_some_and(managed_destination)
                || (id == Panel::STARTUP_SESSION_ID
                    && self
                        .remotes
                        .default_host
                        .as_deref()
                        .is_some_and(managed_destination));
            let live = self
                .slots
                .iter()
                .any(|slot| !slot.closing && slot.panel.read(cx).session_id == id);
            if cloud && live {
                self.remotes
                    .cloud_progress
                    .entry(id.to_owned())
                    .or_default()
                    .observe(message, failed);
            }
        }
        if request_id == Some(Panel::STARTUP_SESSION_ID) {
            self.update_startup_status(message, failed, cx);
        } else if let Some(request_id) = request_id {
            for slot in &self.slots {
                if !slot.closing
                    && slot.panel.read(cx).session_id == request_id
                    && slot.panel.read(cx).is_pending_session()
                {
                    slot.panel.update(cx, |panel, cx| {
                        // Helper phases and SSH results use separate channels.
                        // A buffered wake-success phase must not erase an SSH
                        // failure. Retry gets a fresh ID and resets the status.
                        if !failed && panel.status.starts_with("Session creation failed:") {
                            return;
                        }
                        panel.status = if failed {
                            format!("Session creation failed: {message}")
                        } else {
                            message.to_owned()
                        };
                        cx.notify();
                    });
                }
            }
        }
    }

    fn retry_pending_session(&mut self, index: usize, local: bool, cx: &mut Context<Self>) {
        let Some(panel) = self
            .slots
            .get(index)
            .filter(|slot| !slot.closing)
            .map(|slot| slot.panel.clone())
        else {
            return;
        };
        if !panel.read(cx).is_pending_session() {
            return;
        }
        let remote_host = pending::remote_draft_host(&panel.read(cx).session_id).map(str::to_owned);
        if let Some(host) = remote_host.as_ref().filter(|_| !local) {
            let host = new_session_host(host).to_owned();
            let request_id = pending::next_remote_draft_id(&host);
            panel.update(cx, |panel, cx| {
                panel.session_id = request_id.clone();
                panel.status = format!("Retrying connection to {host}…");
                cx.notify();
            });
            self.create_remote_draft_session(host.clone(), request_id, cx);
        } else if panel.read(cx).is_startup_draft() && !local {
            self.remotes
                .cloud_progress
                .remove(Panel::STARTUP_SESSION_ID);
            self.update_startup_status("Retrying connection to the default machine…", false, cx);
            self.create_default_session(
                self.default_working_dir(),
                Some(Panel::STARTUP_SESSION_ID.into()),
                cx,
            );
        } else {
            // Keep the editor, attachments and queued prompts. A fresh request
            // ID rejects a late remote/create reply rather than changing target.
            let directory = if panel.read(cx).is_startup_draft() || remote_host.is_some() {
                self.default_working_dir()
            } else {
                panel.read(cx).working_dir.clone()
            };
            let mut request_id = pending::next_draft_id();
            if panel
                .read(cx)
                .session_id
                .starts_with("startup://draft/help/")
            {
                request_id = request_id.replacen("startup://draft/", "startup://draft/help/", 1);
            } else if crate::publish::is_publish_draft(&panel.read(cx).session_id) {
                request_id =
                    request_id.replacen("startup://draft/", crate::publish::DRAFT_PREFIX, 1);
            }
            panel.update(cx, |panel, cx| {
                panel.session_id = request_id.clone();
                panel.working_dir = directory.clone();
                panel.title = "New local session".into();
                panel.status = "Connecting to this computer…".into();
                cx.notify();
            });
            self.bridge.send(Command::CreateSession {
                working_dir: directory,
                request_id: Some(request_id),
            });
        }
        cx.notify();
    }

    pub(super) fn render_pending_session(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let panel = self.slots[index].panel.clone();
        let startup = panel.read(cx).is_startup_draft();
        let host = pending::remote_draft_host(&panel.read(cx).session_id).or_else(|| {
            startup
                .then_some(self.remotes.default_host.as_deref())
                .flatten()
        });
        let remote = host.is_some();
        let cloud = host.is_some_and(managed_destination);
        let failed = if startup {
            self.remotes.startup_failed
        } else {
            panel.read(cx).status.starts_with("Session creation failed")
        };
        let theme = Theme::global();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("pending-session-status")
                    .debug_selector(|| "pending-session-status".into())
                    .flex_none()
                    .mx_3()
                    .mt_2()
                    .p_2()
                    .rounded_md()
                    .bg(theme.TOOL_BG)
                    .text_size(px(12.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(cloud, |el| {
                        el.child(
                            div()
                                .debug_selector(|| "pending-cloud-label".into())
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.TEXT)
                                .child(
                                    gpui::svg()
                                        .data(include_bytes!("../../../assets/icons/cloud.svg"))
                                        .w(px(22.))
                                        .h(px(16.))
                                        .text_color(theme.TEXT_DIM),
                                )
                                .child("Jcode Cloud"),
                        )
                    })
                    .child(
                        div()
                            .text_color(if failed { theme.ERROR } else { theme.TEXT_DIM })
                            .child(panel.read(cx).status.clone()),
                    )
                    .when_some(
                        cloud
                            .then(|| {
                                self.remotes
                                    .cloud_progress
                                    .get(&panel.read(cx).session_id)
                            })
                            .flatten(),
                        |el, progress| el.child(progress.render()),
                    )
                    .child(div().text_color(theme.TEXT_DIM).child(if failed {
                        "Not sent. Your draft and queued prompts are preserved."
                    } else {
                        if cloud { "This panel shares your cloud VM. Enter queues your prompt until connected." } else { "Enter queues your prompt until the connection is ready." }
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .when(failed, |el| {
                                el.child(
                                    div()
                                        .id("pending-session-retry")
                                        .debug_selector(|| "pending-session-retry".into())
                                        .px_3()
                                        .py_1()
                                        .rounded_full()
                                        .bg(theme.ACCENT_DIM)
                                        .cursor_pointer()
                                        .child("Retry connection")
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.retry_pending_session(index, false, cx);
                                                this.set_active(index, cx);
                                                this.focus_active(window, cx);
                                            }),
                                        ),
                                )
                            })
                            .when(cloud && failed, |el| {
                                el.child(div()
                                    .id("pending-cloud-account")
                                    .debug_selector(|| "pending-cloud-account".into())
                                    .px_3().py_1().rounded_full().bg(theme.ACCENT_DIM)
                                    .cursor_pointer().child("Jcode account")
                                    .on_click(|_, _, cx| {
                                        cx.stop_propagation();
                                        cx.open_url("https://jcode.sh/account");
                                    }))
                            })
                            .when(remote, |el| {
                                el.child(
                                    div()
                                        .id("pending-session-local")
                                        .debug_selector(|| "pending-session-local".into())
                                        .px_3()
                                        .py_1()
                                        .rounded_full()
                                        .bg(theme.ACCENT_DIM)
                                        .cursor_pointer()
                                        .child("Use this computer")
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.retry_pending_session(index, true, cx);
                                                this.set_active(index, cx);
                                                this.focus_active(window, cx);
                                            }),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .when(cloud, |el| {
                        el.child(
                            // This is a background watermark, never a hit-test overlay
                            // or a loading spinner that competes with the editable draft.
                            div()
                                .debug_selector(|| "pending-cloud-background".into())
                                .absolute()
                                .inset_0()
                                .flex()
                                .justify_center()
                                .pt(px(32.))
                                .child(
                                    gpui::svg()
                                        .data(include_bytes!("../../../assets/icons/cloud.svg"))
                                        .w(px(300.))
                                        .h(px(188.))
                                        .text_color(theme.TEXT)
                                        .opacity(0.07),
                                ),
                        )
                    })
                    .child(panel),
            )
            .into_any_element()
    }

    pub(super) fn restore_hidden_machine_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (self.overview
            || self.slots.get(self.active).is_none_or(|slot| {
                slot.closing || slot.row != self.active_row || !slot.panel.read(cx).is_machines()
            }))
            && self
                .remotes
                .input
                .as_ref()
                .is_some_and(|input| input.read(cx).focus_handle.is_focused(window))
        {
            self.focus_active(window, cx);
        }
    }

    pub(super) fn start_default_startup(&mut self, cx: &mut Context<Self>) {
        self.start_cloud_monitor(cx);
        if !self.remotes.startup_requested
            && self
                .slots
                .iter()
                .any(|slot| !slot.closing && slot.panel.read(cx).is_startup_draft())
        {
            self.remotes.startup_requested = true;
            self.create_default_session(
                self.default_working_dir(),
                Some(Panel::STARTUP_SESSION_ID.into()),
                cx,
            );
        }
    }

    /// A local favorite must never be passed to a different filesystem. Remote
    /// defaults start in the remote home, while explicit local folder actions
    /// and terminal panels continue to refer to this computer.
    pub(super) fn create_remote_draft_session(
        &mut self,
        host: String,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        let host = if managed_destination(&host) {
            MANAGED_CLOUD_HOST.to_owned()
        } else {
            host
        };
        if host == MANAGED_CLOUD_HOST {
            // Restored defaults and drafts from the old personal-alpha button
            // move to the managed destination. They never run the AWS helper.
            let request_id = if pending::remote_draft_host(&request_id) == Some(cloud_alpha::HOST) {
                let managed_id = pending::next_remote_draft_id(MANAGED_CLOUD_HOST);
                for slot in &self.slots {
                    if !slot.closing
                        && slot.panel.read(cx).session_id == request_id
                        && slot.panel.read(cx).is_pending_session()
                    {
                        slot.panel.update(cx, |panel, cx| {
                            panel.session_id = managed_id.clone();
                            cx.notify();
                        });
                    }
                }
                managed_id
            } else {
                request_id
            };
            if self.remotes.default_host.as_deref() == Some(cloud_alpha::HOST) {
                self.remotes.default_host = Some(MANAGED_CLOUD_HOST.into());
            }
            self.remotes.cloud_progress.remove(&request_id);
            self.update_pending_remote_status(Some(&request_id), MANAGED_CLOUD_STARTING, false, cx);
            self.bridge.send(Command::CreateRemoteSession {
                host,
                working_dir: None,
                request_id: Some(request_id),
            });
        } else {
            self.bridge.send(Command::CreateRemoteSession {
                host,
                working_dir: None,
                request_id: Some(request_id),
            });
        }
    }

    pub(super) fn create_default_session(
        &mut self,
        local_directory: Option<String>,
        request_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .remotes
            .default_host
            .as_deref()
            .is_some_and(managed_destination)
        {
            self.remotes.default_host = Some(MANAGED_CLOUD_HOST.into());
            for slot in &self.slots {
                if Some(slot.panel.read(cx).session_id.as_str()) == request_id.as_deref()
                    && slot.panel.read(cx).is_pending_session()
                {
                    slot.panel.update(cx, |panel, cx| {
                        panel.title = "Jcode Cloud".into();
                        cx.notify();
                    });
                }
            }
            if let Some(id) = &request_id {
                self.remotes.cloud_progress.remove(id);
            }
            self.update_pending_remote_status(
                request_id.as_deref(),
                MANAGED_CLOUD_STARTING,
                false,
                cx,
            );
            self.bridge.send(Command::CreateRemoteSession {
                host: MANAGED_CLOUD_HOST.into(),
                working_dir: None,
                request_id,
            });
            return;
        }
        self.bridge.send(match &self.remotes.default_host {
            Some(host) => Command::CreateRemoteSession {
                host: host.clone(),
                working_dir: None,
                request_id,
            },
            None => Command::CreateSession {
                working_dir: local_directory,
                request_id,
            },
        });
    }

    pub(super) fn start_cloud_monitor(&mut self, cx: &mut Context<Self>) {
        if self.remotes.cloud_monitor.is_some() || harness::screenshot_mode() || cfg!(test) {
            return;
        }
        self.remotes.cloud_monitor = Some(cx.spawn(async move |this, cx| {
            let mut previous_summary = String::new();
            loop {
                if this
                    .update(cx, |this, cx| {
                        let active_cloud_panels: Vec<String> = this
                            .slots
                            .iter()
                            .filter_map(|slot| {
                                let panel = slot.panel.read(cx);
                                live_cloud_panel(
                                    slot.closing,
                                    &panel.session_id,
                                    panel.history_loaded(),
                                    panel.is_pending_session(),
                                    this.remotes
                                        .connected_cloud_sessions
                                        .contains(&panel.session_id),
                                )
                                .then(|| panel.session_id.clone())
                            })
                            .collect();
                        let configured = !active_cloud_panels.is_empty();
                        this.remotes.cloud.refresh_active(active_cloud_panels);
                        if configured {
                            this.remotes.cloud.refresh();
                        }
                        let updates = this.remotes.cloud.take_updates();
                        let changed = !updates.is_empty();
                        for (message, failed, request_id) in updates {
                            this.update_pending_remote_status(
                                request_id.as_deref(),
                                &message,
                                failed,
                                cx,
                            );
                            this.remotes.status = Some(message);
                            this.remotes.failed = failed;
                        }
                        let summary = if configured {
                            this.remotes.cloud.summary()
                        } else {
                            String::new()
                        };
                        let mut elapsed_changed = false;
                        for progress in this.remotes.cloud_progress.values_mut() {
                            elapsed_changed |= progress.tick();
                        }
                        if summary != previous_summary || changed || elapsed_changed {
                            previous_summary = summary;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        }));
    }

    pub(super) fn open_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_machines(cx);
        self.focus_active(window, cx);
    }

    pub(super) fn show_machines(&mut self, cx: &mut Context<Self>) {
        self.start_cloud_monitor(cx);
        if let Some(index) = self.machines_panel_index(cx) {
            self.set_active(index, cx);
        } else {
            let input = self.create_machine_input(cx);
            self.remotes.input = Some(input.clone());
            let panel = cx.new(|cx| {
                let mut panel = Panel::new(
                    Panel::MACHINES_SESSION_ID.into(),
                    Some("Machines".into()),
                    None,
                    self.bridge.clone(),
                    cx,
                );
                panel.input = input;
                panel
            });
            let width_fraction = 0.5;
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
                        transition::policy(Transition::PanelOrder).duration,
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
            self.set_active(insert_at, cx);
            crate::sounds::play(crate::sounds::Cue::PanelOpen, cx);
        }
        self.overview = false;
        self.overview_progress.set(0.0, Instant::now());
        self.focus_pending = true;
        #[cfg(not(test))]
        if !harness::screenshot_mode() {
            self.remotes.discovery = Some(cx.spawn(async move |this, cx| {
                let hosts = cx
                    .background_executor()
                    .spawn(async { crate::remote_targets::discover_hosts() })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    for host in hosts {
                        if !this.remotes.hosts.contains(&host) {
                            this.remotes.hosts.push(host);
                        }
                    }
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    pub(super) fn machines_panel_index(&self, cx: &App) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| !slot.closing && slot.panel.read(cx).is_machines())
    }

    pub(super) fn create_machine_input(&self, cx: &mut Context<Self>) -> Entity<PromptInput> {
        let weak = cx.weak_entity();
        let cancel = weak.clone();
        cx.new(|cx| {
            PromptInput::new(cx, "SSH alias or user@hostname", move |host, _, _, app| {
                let _ = weak.update(app, |this, cx| this.connect_machine(Some(host), cx));
            })
            .with_on_overlay_cancel(move |app| {
                let _ = cancel.update(app, |this, cx| this.close_machines(cx));
                true
            })
        })
    }

    pub(super) fn close_machines(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.machines_panel_index(cx) else {
            return;
        };
        let active_id = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.entity_id());
        let removed_id = self.slots.remove(index).panel.entity_id();
        crate::sounds::play(crate::sounds::Cue::PanelClose, cx);
        if self.previous == Some(removed_id) {
            self.previous = None;
        }
        for remembered in &mut self.row_focus {
            if *remembered == Some(removed_id) {
                *remembered = None;
            }
        }
        self.active = active_id
            .filter(|id| *id != removed_id)
            .and_then(|id| {
                self.slots
                    .iter()
                    .position(|slot| slot.panel.entity_id() == id)
            })
            .unwrap_or_else(|| {
                let remaining: Vec<_> = self
                    .row_indices(self.active_row)
                    .filter(|&index| !self.slots[index].closing)
                    .collect();
                focus_after_close(index, &remaining)
            });
        if let Some(slot) = self
            .slots
            .get(self.active)
            .filter(|slot| slot.row == self.active_row && !slot.closing)
        {
            self.row_focus[self.active_row] = Some(slot.panel.entity_id());
        }
        self.remotes.input = None;
        self.focus_pending = true;
        self.retarget_camera();
        cx.notify();
    }

    pub(super) fn connect_machine(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        let host = match host {
            Some(host) => match crate::remote_targets::validate_host(&host) {
                Ok(host) => Some(new_session_host(&host).to_owned()),
                Err(error) => {
                    self.remotes.notice = Some(error);
                    cx.notify();
                    return;
                }
            },
            None => None,
        };
        if let Some(host) = host {
            self.remotes.failed = false;
            if !self.remotes.hosts.contains(&host) {
                self.remotes.hosts.push(host.clone());
            }
            self.remotes.notice = crate::config::persist_remote_hosts(&self.remotes.hosts)
                .err()
                .map(|error| format!("Could not save machines: {error}"));
            self.remotes.status = Some(format!("Connecting to {host}…"));
            self.open_remote_draft(host, cx);
        } else {
            self.remotes.notice = None;
            self.open_local_draft(self.default_working_dir(), cx);
        }
        cx.notify();
    }

    pub(super) fn set_default_machine(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        let host = match host {
            Some(host) => match crate::remote_targets::validate_host(&host) {
                Ok(host) => Some(new_session_host(&host).to_owned()),
                Err(error) => {
                    self.remotes.notice = Some(error);
                    cx.notify();
                    return;
                }
            },
            None => None,
        };
        match crate::config::persist_default_remote_host(host.as_deref()) {
            Ok(()) => {
                self.remotes.default_host = host;
                self.remotes.notice = None;
            }
            Err(error) => self.remotes.notice = Some(format!("Could not save default: {error}")),
        }
        cx.notify();
    }

    pub(super) fn render_machine_switcher(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let host = self.remotes.default_host.as_deref();
        let button = |id: &'static str, icon: &'static [u8], title: String, selected: bool| {
            let ink = if selected {
                theme.ACCENT
            } else {
                theme.TEXT_DIM
            };
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_none()
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.ACCENT_DIM))
                .hover(|el| el.bg(theme.TOOL_BG))
                .tooltip(move |_, cx| cx.new(|_| HeaderTooltip(title.clone().into())).into())
                .child(gpui::svg().data(icon).size(px(16.0)).text_color(ink))
        };
        div()
            .debug_selector(|| "machine-destination-switcher".into())
            .flex_none()
            .px_3()
            .py_1p5()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(theme.PANEL_BORDER)
            .text_size(px(10.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        button(
                            "machine-destination-local",
                            include_bytes!("../../../assets/icons/computer.svg"),
                            "This computer · Use locally for new sessions".into(),
                            host.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_default_machine(None, cx);
                        })),
                    )
                    .child(
                        button(
                            "machine-destination-cloud",
                            include_bytes!("../../../assets/icons/machine-cloud.svg"),
                            "Cloud · Use Jcode Cloud for new sessions".into(),
                            host.is_some_and(managed_destination),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_default_machine(Some(MANAGED_CLOUD_HOST.into()), cx);
                        })),
                    )
                    .child(
                        button(
                            "machines-picker-button",
                            include_bytes!("../../../assets/icons/ssh.svg"),
                            match host.filter(|host| !managed_destination(host)) {
                                Some(host) => format!("SSH · {host} · Choose a remote host"),
                                None => "SSH · Choose a remote host".into(),
                            },
                            host.is_some_and(|host| !managed_destination(host)),
                        )
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_machines(window, cx);
                            }),
                        ),
                    )
                    .child(self.render_default_directory_button(cx)),
            )
            .when(self.remotes.failed, |el| {
                el.child(
                    div()
                        .text_color(theme.ERROR)
                        .child("Connection needs attention"),
                )
            })
            .children(
                self.remotes
                    .notice
                    .as_ref()
                    .map(|notice| div().text_color(theme.ERROR).child(notice.clone())),
            )
            .children(self.remotes.cloud.lease_warning().map(|warning| {
                div()
                    .debug_selector(|| "cloud-alpha-lease-warning".into())
                    .text_color(Theme::global().ERROR)
                    .child(warning)
            }))
            .into_any_element()
    }

    pub(super) fn render_machines(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let mut picker = div().id("machines-picker").debug_selector(|| "machines-picker".into())
            .size_full().p_3().flex().flex_col().gap_2().overflow_y_scroll().text_size(px(11.0))
            .child(div().flex().justify_between()
                .child(div().text_size(px(14.0)).child("Machines"))
                .child(div().id("machines-panel-close").debug_selector(|| "machines-panel-close".into())
                    .cursor_pointer().child("close")
                    .on_click(cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.close_machines(cx); }))))
            .child(div().text_color(theme.TEXT_DIM)
                .child("Connect opens a new panel. Default applies to new session panels, including after restart."))
            .child(div().debug_selector(|| "machine-connection-status".into())
                .text_color(if self.remotes.failed { theme.ERROR } else { theme.ACCENT })
                .child(self.remotes.status.clone().unwrap_or_default()))
            .children((self.remotes.startup_failed && self.slots.iter().any(|slot| !slot.closing && slot.panel.read(cx).is_startup_draft())).then(|| {
                div().id("machine-retry-startup").debug_selector(|| "machine-retry-startup".into())
                    .px_2().py_2().cursor_pointer().bg(theme.ACCENT_DIM)
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                        if let Some(index) = this.slots.iter().position(|slot| !slot.closing && slot.panel.read(cx).is_startup_draft()) {
                            this.retry_pending_session(index, false, cx);
                        }
                    })).child("Retry startup on default machine")
            }))
            .child(self.render_machine_row(None, 0, cx));
        for (index, host) in self.remotes.hosts.iter().enumerate() {
            picker = picker.child(self.render_machine_row(Some(host.clone()), index + 1, cx));
        }
        picker = picker.child(div().mt_2().child("Connect another machine"));
        if let Some(input) = &self.remotes.input {
            picker = picker
                .child(
                    div()
                        .id("machine-host-input")
                        .debug_selector(|| "machine-host-input".into())
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if let Some(input) = &this.remotes.input {
                                    let focus = input.read(cx).focus_handle.clone();
                                    if let Some(index) = this.machines_panel_index(cx) {
                                        this.set_active(index, cx);
                                    }
                                    window.focus(&focus, cx);
                                    this.focus_pending = false;
                                }
                                cx.stop_propagation();
                            }),
                        )
                        .child(input.clone()),
                )
                .child(
                    div()
                        .id("machine-connect-input")
                        .debug_selector(|| "machine-connect-input".into())
                        .px_2()
                        .py_2()
                        .rounded_sm()
                        .cursor_pointer()
                        .bg(theme.ACCENT_DIM)
                        .hover(|el| el.bg(theme.TOOL_BG))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if let Some(input) = &this.remotes.input {
                                    let host = input.read(cx).snapshot().content;
                                    this.connect_machine(Some(host), cx);
                                }
                            }),
                        )
                        .child("Connect via SSH"),
                );
        } else {
            picker = picker.child(
                div()
                    .id("machine-add-host")
                    .px_2()
                    .py_2()
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_machines(window, cx);
                        }),
                    )
                    .child("Add SSH host…"),
            );
        }
        picker.child(div().text_color(theme.TEXT_DIM)
                .child("Uses your SSH config and keys. First connect in a terminal to trust the host and set up key access. Jcode must be installed on the remote machine."))
            .child(div().text_color(theme.TEXT_DIM)
                .child("Remote panels start in the remote home. Folder browsing and terminal panels stay local."))
            .children(self.remotes.notice.as_ref().map(|notice| div()
                .debug_selector(|| "machine-error".into()).text_color(theme.ERROR).child(notice.clone())))
            .into_any_element()
    }

    pub(super) fn render_machine_row(
        &self,
        host: Option<String>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_default = self.remotes.default_host == host;
        let label = match host.as_deref() {
            Some(MANAGED_CLOUD_HOST) => "Jcode Cloud".into(),
            Some(cloud_alpha::HOST) => "Personal cloud alpha (existing sessions only)".into(),
            Some(host) => host.to_owned(),
            None => "This computer".into(),
        };
        let default_host = host.clone();
        let cloud_status = host
            .as_deref()
            .is_some_and(managed_destination)
            .then(|| "Your own machine, run by Jcode. Sleeps when idle.".to_owned());
        div()
            .px_2()
            .py_2()
            .rounded_sm()
            .bg(Theme::global().PANEL_BG)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_family(Theme::global().FONT_MONO)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .children(cloud_status.map(|status| {
                div()
                    .debug_selector(|| "cloud-alpha-status".into())
                    .text_color(Theme::global().TEXT_DIM)
                    .child(status)
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_between()
                    .child(
                        div()
                            .id(("machine-connect", index))
                            .debug_selector(move || format!("machine-connect-{index}").into())
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(Theme::global().ACCENT_DIM)
                            .hover(|el| el.bg(Theme::global().TOOL_BG))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    window.prevent_default();
                                    this.connect_machine(host.clone(), cx);
                                }),
                            )
                            // A real runtime can attach the new local panel
                            // between press and release. Do not let the old
                            // Machines surface reclaim selection on mouse-up.
                            .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .child("Connect"),
                    )
                    .child(
                        div()
                            .id(("machine-default", index))
                            .debug_selector(move || format!("machine-default-{index}").into())
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(if is_default {
                                Theme::global().ACCENT
                            } else {
                                Theme::global().TEXT_DIM
                            })
                            .hover(|el| el.bg(Theme::global().TOOL_BG))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_default_machine(default_host.clone(), cx);
                                }),
                            )
                            .child(if is_default {
                                "✓ Default"
                            } else {
                                "Set default"
                            }),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod cloud_routing_tests {
    use super::*;

    #[test]
    fn cloud_active_detector_requires_connected_nonclosing_exact_namespace() {
        let id = "ssh://jcode-cloud-alpha/session";
        assert!(live_cloud_panel(false, id, true, false, true));
        assert!(!live_cloud_panel(false, id, true, false, false));
        assert!(live_cloud_panel(false, id, true, false, true));
        // A lost old panel must not keep checks alive once a new live one closes.
        assert!(
            ![(false, false), (true, true)]
                .into_iter()
                .any(|(closing, connected)| live_cloud_panel(closing, id, true, false, connected))
        );
        assert!(!live_cloud_panel(true, id, true, false, true));
        assert!(!live_cloud_panel(false, id, false, false, true));
        assert!(!live_cloud_panel(false, id, true, true, true));
        for id in [
            "local",
            "startup://draft",
            "ssh://other/session",
            "ssh://jcode-cloud-alpha-evil/session",
            "ssh://jcode-cloud-alpha/",
        ] {
            assert!(!live_cloud_panel(false, id, true, false, true));
        }
    }

    #[gpui::test]
    fn managed_and_legacy_cloud_defaults_connect_only_to_managed_cloud(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = harness::spawn_recording();
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.bridge = bridge;
            workspace
        });
        workspace.update(cx, |workspace, cx| {
            for host in [MANAGED_CLOUD_HOST, cloud_alpha::HOST] {
                workspace.remotes.default_host = Some(host.into());
                workspace.create_default_session(
                    Some("/local-only".into()),
                    Some(Panel::STARTUP_SESSION_ID.into()),
                    cx,
                );
                assert!(
                    matches!(
                        commands.try_recv(),
                        Ok(Command::CreateRemoteSession { host, working_dir: None, request_id: Some(id) })
                            if host == MANAGED_CLOUD_HOST && id == Panel::STARTUP_SESSION_ID
                    ),
                    "cloud must never fall back to SSH aliases, the AWS alpha, or local"
                );
                assert!(commands.try_recv().is_err());
                assert!(
                    workspace.remotes.cloud.take_updates().is_empty(),
                    "must not invoke the personal AWS helper"
                );
                assert!(!workspace.remotes.startup_failed);
                assert_eq!(
                    workspace.remotes.default_host.as_deref(),
                    Some(MANAGED_CLOUD_HOST)
                );
            }
        });
    }

    #[gpui::test]
    fn managed_and_legacy_cloud_connect_open_a_managed_draft(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = harness::spawn_recording();
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.bridge = bridge;
            workspace
        });
        workspace.update(cx, |workspace, cx| {
            for host in [MANAGED_CLOUD_HOST, cloud_alpha::HOST] {
                workspace.connect_machine(Some(host.into()), cx);
                let panel = workspace.slots[workspace.active].panel.read(cx);
                let id = panel.session_id.clone();
                assert_eq!(pending::remote_draft_host(&id), Some(MANAGED_CLOUD_HOST));
                assert!(panel.status.contains(MANAGED_CLOUD_STARTING));
                assert!(matches!(
                    commands.try_recv(),
                    Ok(Command::CreateRemoteSession { host, request_id: Some(request), .. })
                        if host == MANAGED_CLOUD_HOST && request == id
                ));
                assert!(workspace.remotes.cloud.take_updates().is_empty());
            }
        });
    }

    #[test]
    fn managed_names_never_rewrite_existing_ssh_session_ids() {
        assert_eq!(new_session_host(cloud_alpha::HOST), MANAGED_CLOUD_HOST);
        assert_eq!(new_session_host("my-server"), "my-server");
        assert!(!managed_destination("jcode-cloud-evil"));
        assert!(!managed_destination("ssh://jcode-cloud-alpha/session"));
    }
}
