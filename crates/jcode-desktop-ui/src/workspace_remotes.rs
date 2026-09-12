//! Machine selection is a workspace preference, not a mutation of existing panels.
use super::*;

#[derive(Default)]
pub(super) struct Machines {
    pub default_host: Option<String>,
    pub hosts: Vec<String>,
    pub status: Option<String>,
    pub failed: bool,
    pub startup_failed: bool,
    startup_requested: bool,
    input: Option<Entity<PromptInput>>,
    notice: Option<String>,
    discovery: Option<gpui::Task<()>>,
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
            default_host: config.default_remote_host.clone(),
            hosts,
            ..Self::default()
        }
    }
}

impl Workspace {
    pub(super) fn restore_hidden_machine_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (!self.show_sidebar || self.sidebar_view != SidebarView::Machines)
            && self
                .remotes
                .input
                .as_ref()
                .is_some_and(|input| input.read(cx).focus_handle.is_focused(window))
        {
            self.focus_active(window, cx);
        }
    }

    pub(super) fn start_default_startup(&mut self, cx: &App) {
        if !self.remotes.startup_requested
            && self
                .slots
                .iter()
                .any(|slot| !slot.closing && slot.panel.read(cx).is_startup_draft())
        {
            self.remotes.startup_requested = true;
            self.create_default_session(
                default_working_dir(),
                Some(Panel::STARTUP_SESSION_ID.into()),
            );
        }
    }

    /// A local favorite must never be passed to a different filesystem. Remote
    /// defaults start in the remote home, while explicit local folder actions
    /// and terminal panels continue to refer to this computer.
    pub(super) fn create_default_session(
        &self,
        local_directory: Option<String>,
        request_id: Option<String>,
    ) {
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

    pub(super) fn open_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar = true;
        self.sidebar_view = SidebarView::Machines;
        self.focus_pending = false;
        if self.remotes.input.is_none() {
            let weak = cx.weak_entity();
            let cancel = weak.clone();
            self.remotes.input = Some(cx.new(|cx| {
                PromptInput::new(cx, "SSH alias or user@hostname", move |host, _, _, app| {
                    let _ = weak.update(app, |this, cx| this.connect_machine(Some(host), cx));
                })
                .with_on_overlay_cancel(move |app| {
                    let _ = cancel.update(app, |this, cx| {
                        this.sidebar_view = SidebarView::Sessions;
                        this.focus_pending = true;
                        cx.notify();
                    });
                    true
                })
            }));
        }
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
        cx.defer_in(window, |this, window, cx| {
            if let Some(input) = &this.remotes.input {
                let focus = input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn connect_machine(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        let host = match host {
            Some(host) => match crate::remote_targets::validate_host(&host) {
                Ok(host) => Some(host),
                Err(error) => {
                    self.remotes.notice = Some(error);
                    cx.notify();
                    return;
                }
            },
            None => None,
        };
        if let Some(host) = host {
            if !self.remotes.hosts.contains(&host) {
                self.remotes.hosts.push(host.clone());
            }
            self.remotes.notice = crate::config::persist_remote_hosts(&self.remotes.hosts)
                .err()
                .map(|error| format!("Could not save machines: {error}"));
            self.remotes.status = Some(format!("Connecting to {host}…"));
            self.bridge.send(Command::CreateRemoteSession {
                host,
                working_dir: None,
                request_id: None,
            });
        } else {
            self.remotes.notice = None;
            self.bridge.send(Command::CreateSession {
                working_dir: default_working_dir(),
                request_id: None,
            });
        }
        cx.notify();
    }

    pub(super) fn set_default_machine(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        let host = match host {
            Some(host) => match crate::remote_targets::validate_host(&host) {
                Ok(host) => Some(host),
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
        div()
            .id("machines-picker-button")
            .debug_selector(|| "machines-picker-button".into())
            .flex_none()
            .px_3()
            .py_1p5()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_size(px(10.5))
            .cursor_pointer()
            .hover(|el| el.bg(Theme::global().TOOL_BG))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.sidebar_view == SidebarView::Machines {
                        this.sidebar_view = SidebarView::Sessions;
                        this.focus_pending = true;
                        cx.notify();
                    } else {
                        this.open_machines(window, cx);
                    }
                }),
            )
            .child(div().text_color(Theme::global().TEXT_DIM).child("Machines"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(Theme::global().ACCENT)
                    .child(format!(
                        "New: {}",
                        self.remotes
                            .default_host
                            .as_deref()
                            .unwrap_or("This computer")
                    )),
            )
            .when(self.remotes.failed, |el| {
                el.child(div().text_color(Theme::global().ERROR).child("!"))
            })
            .child(if self.sidebar_view == SidebarView::Machines {
                "▴"
            } else {
                "▾"
            })
            .into_any_element()
    }

    pub(super) fn render_machines(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let mut picker = div().id("machines-picker").debug_selector(|| "machines-picker".into())
            .p_3().flex().flex_col().gap_2().overflow_y_scroll().text_size(px(11.0))
            .child(div().text_size(px(14.0)).child("Machines"))
            .child(div().text_color(theme.TEXT_DIM)
                .child("Connect opens a new panel. Default applies to new session panels, including after restart."))
            .child(div().debug_selector(|| "machine-connection-status".into())
                .text_color(theme.ACCENT).child(self.remotes.status.clone().unwrap_or_default()))
            .children((self.remotes.startup_failed && self.slots.iter().any(|slot| !slot.closing && slot.panel.read(cx).is_startup_draft())).then(|| {
                div().id("machine-retry-startup").debug_selector(|| "machine-retry-startup".into())
                    .px_2().py_2().cursor_pointer().bg(theme.ACCENT_DIM)
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.remotes.startup_failed = false;
                        this.create_default_session(default_working_dir(), Some(Panel::STARTUP_SESSION_ID.into()));
                        this.remotes.status = Some("Retrying startup on the default machine…".into());
                        cx.notify();
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
        let label = host.clone().unwrap_or_else(|| "This computer".into());
        let default_host = host.clone();
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
                                cx.listener(move |this, _, _, cx| {
                                    this.connect_machine(host.clone(), cx);
                                }),
                            )
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
