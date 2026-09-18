//! Native login UI. Secrets and OAuth state never enter the conversation or snapshots.
use super::*;
use crate::login_input::LoginInput;
use jcode_sdk::{
    AuthClient, AuthFlow, AuthInputKind, AuthOptions, AuthPrompt, LoginMethod, LoginProvider,
};

#[path = "panel_login_status.rs"]
mod connection;
use connection::{ConnectionStatus, ConnectionStatuses};

pub(super) struct LoginState {
    client: AuthClient,
    providers: Vec<LoginProvider>,
    provider: Option<LoginProvider>,
    flow: Option<AuthFlow>,
    prompt: Option<AuthPrompt>,
    input: Entity<LoginInput>,
    busy: bool,
    error: Option<String>,
    complete: bool,
    focus_pending: bool,
    task: Option<Task<()>>,
    statuses: Option<ConnectionStatuses>,
    status_loading: bool,
    status_task: Option<Task<()>>,
}

impl Drop for LoginState {
    fn drop(&mut self) {
        // Cancellation can involve process cleanup. Never block the UI thread.
        if let Some(flow) = self.flow.take() {
            std::thread::spawn(move || {
                let _ = flow.cancel();
            });
        }
    }
}

enum LoginUpdate {
    Prompt(AuthPrompt),
    Complete { validation_warning: bool },
}

impl Panel {
    pub(super) fn login_input_focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        let state = self.login.as_ref()?;
        (state
            .provider
            .as_ref()
            .is_some_and(|provider| provider.method == LoginMethod::ApiKey)
            || state
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.input_kind != AuthInputKind::DeviceCode))
        .then(|| state.input.read(cx).focus_handle.clone())
    }

    fn login_is_remote(&self) -> bool {
        let source = self
            .session_id
            .strip_prefix("accounts://")
            .unwrap_or(&self.session_id);
        crate::harness::remote_host(source).is_some()
    }

    pub(super) fn open_login_picker(&mut self, cx: &mut Context<Self>) {
        self.recovery_picker_open = false;
        if self.login.is_some() {
            return;
        }
        let client = AuthClient::new(AuthOptions {
            binary: crate::platform::companion_executable("jcode"),
            ..Default::default()
        });
        let providers = if self.preview_state.is_some() {
            vec![
                LoginProvider {
                    id: "openai",
                    display_name: "OpenAI",
                    detail: "Offline OAuth preview",
                    method: LoginMethod::OAuth,
                },
                LoginProvider {
                    id: "openai-api",
                    display_name: "OpenAI API key",
                    detail: "Offline credential preview",
                    method: LoginMethod::ApiKey,
                },
            ]
        } else {
            client.providers()
        };
        self.login = Some(LoginState {
            client,
            providers,
            provider: None,
            flow: None,
            prompt: None,
            input: cx.new(|cx| LoginInput::new(cx, "Paste the code or callback URL")),
            busy: false,
            error: None,
            complete: false,
            focus_pending: true,
            task: None,
            statuses: None,
            status_loading: false,
            status_task: None,
        });
        // Local credentials cannot authenticate an SSH-hosted session.
        if self.login_is_remote() {
            self.login.as_mut().unwrap().error = Some(
                "This session runs on another machine. Desktop login currently connects accounts on this computer only. Your remote credentials have not been changed.".into());
        } else {
            self.refresh_login_status(cx);
        }
        cx.notify();
    }

    fn refresh_login_status(&mut self, cx: &mut Context<Self>) {
        let offline =
            self.preview_state.is_some() || cfg!(test) || crate::harness::screenshot_mode();
        let Some(state) = self.login.as_mut() else {
            return;
        };
        if state.status_loading {
            return;
        }
        state.status_loading = true;
        state.statuses = None;
        let provider_ids: Vec<_> = state
            .providers
            .iter()
            .map(|provider| provider.id.to_owned())
            .collect();
        let task = cx.background_executor().spawn(async move {
            if offline {
                let mut statuses = ConnectionStatuses::from([
                    ("openai".into(), ConnectionStatus::Connected),
                    ("openai-api".into(), ConnectionStatus::NotConnected),
                    ("claude".into(), ConnectionStatus::Expired),
                    ("gemini".into(), ConnectionStatus::Unverified),
                    ("copilot".into(), ConnectionStatus::Failed),
                ]);
                for id in provider_ids {
                    statuses.entry(id).or_insert(ConnectionStatus::NotConnected);
                }
                Some(statuses)
            } else {
                connection::fetch_connection_statuses()
            }
        });
        state.status_task = Some(cx.spawn(async move |this, cx| {
            let statuses = task.await;
            let _ = this.update(cx, |panel, cx| {
                if let Some(state) = panel.login.as_mut() {
                    state.statuses = statuses;
                    state.status_loading = false;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    pub(super) fn set_preview_login_error(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.login.as_mut() {
            state.error = Some("Sign-in failed: the authorization code has expired. Offline preview: no browser was opened and no credentials were changed.".into());
            state.busy = false;
        }
        cx.notify();
    }

    pub(super) fn close_login_picker(&mut self, cx: &mut Context<Self>) {
        self.login = None;
        cx.notify();
    }

    pub(crate) fn login_command(&mut self, content: &str, cx: &mut Context<Self>) -> bool {
        let mut words = content.split_whitespace();
        if words.next() != Some("/login") {
            return false;
        }
        self.open_login_picker(cx);
        if let Some(provider) = words.next() {
            if words.next().is_some() {
                self.login.as_mut().unwrap().error =
                    Some("Choose a provider below to sign in.".into());
            } else if let Some(provider) = {
                let state = self.login.as_ref().unwrap();
                if self.preview_state.is_some() {
                    state
                        .providers
                        .iter()
                        .find(|entry| entry.id == provider)
                        .copied()
                } else {
                    state.client.resolve_provider(provider)
                }
            } {
                self.select_login_provider(provider, cx);
            } else {
                self.login.as_mut().unwrap().error =
                    Some("That provider is not available here. Choose one below.".into());
            }
        }
        true
    }

    fn select_login_provider(&mut self, provider: LoginProvider, cx: &mut Context<Self>) {
        if self.login_is_remote() {
            return;
        }
        // Replacing state cancels precisely the old flow, not saved credentials.
        self.close_login_picker(cx);
        self.open_login_picker(cx);
        let state = self.login.as_mut().unwrap();
        state.provider = Some(provider.clone());
        if self.preview_state.is_some() {
            if provider.method == LoginMethod::ApiKey {
                state.input = cx.new(|cx| LoginInput::new(cx, "Paste your API key"));
            }
            self.set_preview_login_error(cx);
            return;
        }
        if provider.method == LoginMethod::ApiKey {
            state.input = cx.new(|cx| LoginInput::new(cx, "Paste your API key"));
        } else {
            match state.client.begin(provider.id, None) {
                Ok(flow) => {
                    state.flow = Some(flow.clone());
                    self.run_login_task(
                        move || {
                            flow.start()
                                .map(LoginUpdate::Prompt)
                                .map_err(|e| e.to_string())
                        },
                        cx,
                    );
                }
                Err(error) => state.error = Some(error.to_string()),
            }
        }
        cx.notify();
    }

    fn run_login_task(
        &mut self,
        work: impl FnOnce() -> Result<LoginUpdate, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.preview_state.is_some() {
            self.set_preview_login_error(cx);
            return;
        }
        let state = self.login.as_mut().unwrap();
        state.busy = true;
        state.error = None;
        let task = cx.background_executor().spawn(async move {
            if crate::harness::screenshot_mode() {
                return Err(
                    "Sign-in is disabled in the offline preview. No credentials were changed."
                        .into(),
                );
            }
            work()
        });
        state.task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |panel, cx| {
                let Some(state) = panel.login.as_mut() else { return; };
                state.busy = false;
                match result {
                    Ok(LoginUpdate::Prompt(prompt)) => state.prompt = Some(prompt),
                    Ok(LoginUpdate::Complete { validation_warning }) => {
                        state.complete = true;
                        state.flow = None;
                        if validation_warning {
                            state.error = Some("Credentials were saved, but the provider could not be verified. Choose an available model below. You do not need to reuse the sign-in code.".into());
                        }
                        if !panel.is_accounts_panel() {
                            panel.bridge.send(Command::RefreshRuntime { session_id: panel.session_id.clone() });
                        }
                        crate::accounts::request_refresh();
                    }
                    Err(error) => state.error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    fn submit_login(&mut self, cx: &mut Context<Self>) {
        if self.preview_state.is_some() {
            if let Some(state) = self.login.as_mut() {
                state.input.update(cx, |input, cx| input.clear(cx));
            }
            self.set_preview_login_error(cx);
            return;
        }
        let Some(state) = self.login.as_mut() else {
            return;
        };
        if state.busy {
            return;
        }
        let Some(provider) = state.provider.clone() else {
            return;
        };
        let secret = state.input.update(cx, |input, cx| input.take(cx));
        if provider.method == LoginMethod::ApiKey {
            if secret.trim().is_empty() {
                state.error = Some("Paste an API key first.".into());
                cx.notify();
                return;
            }
            self.run_login_task(
                move || {
                    let client = jcode_sdk::JcodeClient::connect(jcode_sdk::ConnectOptions {
                        client_name: "jcode-desktop-login".into(),
                        ensure_runtime: false,
                        ..Default::default()
                    })
                    .map_err(|_| {
                        "Could not connect to Jcode. Try again when the connection is restored."
                            .to_string()
                    })?;
                    client
                        .set_api_key(provider.id, secret.trim())
                        .map_err(|_| {
                            "Could not save this API key. Check the key and try again.".to_string()
                        })?;
                    Ok(LoginUpdate::Complete {
                        validation_warning: false,
                    })
                },
                cx,
            );
            return;
        }
        let (Some(flow), Some(prompt)) = (state.flow.clone(), state.prompt.as_ref()) else {
            return;
        };
        let kind = prompt.input_kind.clone();
        if kind != AuthInputKind::DeviceCode && secret.trim().is_empty() {
            state.error = Some("Paste the code or callback URL from your browser first.".into());
            cx.notify();
            return;
        }
        self.run_login_task(
            move || {
                let result = match kind {
                    AuthInputKind::DeviceCode => flow.complete_device(),
                    AuthInputKind::CallbackUrl => flow.submit_callback(secret.trim()),
                    AuthInputKind::AuthCode => flow.submit_code(secret.trim()),
                    AuthInputKind::AuthCodeOrCallbackUrl => {
                        if secret.trim().starts_with("http") {
                            flow.submit_callback(secret.trim())
                        } else {
                            flow.submit_code(secret.trim())
                        }
                    }
                };
                let result = result.map_err(|e| e.to_string())?;
                Ok(LoginUpdate::Complete {
                    validation_warning: result.validation_warning,
                })
            },
            cx,
        );
    }

    pub(super) fn render_login_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if let Some(state) = self.login.as_mut()
            && state.focus_pending
        {
            state.focus_pending = false;
            if state
                .provider
                .as_ref()
                .is_some_and(|provider| provider.method == LoginMethod::ApiKey)
            {
                let focus = state.input.read(cx).focus_handle.clone();
                focus.focus(window, cx);
            } else {
                self.focus_handle.focus(window, cx);
            }
        }
        let state = self.login.as_ref()?;
        let theme = Theme::global();
        let remote = self.login_is_remote();
        let mut body = div().flex().flex_col().gap_3();
        if let Some(error) = &state.error {
            body = body.child(
                div()
                    .debug_selector(|| "login-error".into())
                    .text_color(theme.ERROR)
                    .child(error.clone()),
            );
        }
        if state.complete {
            body = body
                .child(
                    div()
                        .debug_selector(|| "login-complete".into())
                        .text_color(if state.error.is_some() {
                            theme.WARN
                        } else {
                            theme.OK
                        })
                        .child("Account connected. Choose a model to continue."),
                )
                .child(
                    login_button("login-choose-model", "Choose a model").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.close_login_picker(cx);
                            if this.is_accounts_panel() {
                                cx.emit(AccountsPanelChooseModel);
                            } else {
                                this.open_recovery_models(cx);
                            }
                        },
                    )),
                );
        } else if let Some(provider) = &state.provider {
            body = body.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(provider.display_name),
            );
            if state.busy {
                body = body.child(
                    div()
                        .debug_selector(|| "login-busy".into())
                        .child("Connecting securely…"),
                );
            } else if provider.method == LoginMethod::ApiKey {
                body = body
                    .child(div().text_color(theme.TEXT_DIM).child(
                        "Your key is saved in Jcode's private credential store, never in chat.",
                    ))
                    .child(state.input.clone())
                    .child(
                        login_button("login-paste", "Paste from clipboard").on_click(
                            cx.listener(|this, _, window, cx| this.paste_login(window, cx)),
                        ),
                    )
                    .child(
                        login_button("login-submit", "Connect account")
                            .on_click(cx.listener(|this, _, _, cx| this.submit_login(cx))),
                    );
            } else if let Some(prompt) = &state.prompt {
                let url = prompt.auth_url.clone();
                let preview = self.preview_state.is_some();
                body = body
                    .child(
                        div().child(
                            "Continue in your browser, then return here to finish connecting.",
                        ),
                    )
                    .child(
                        login_button("login-open-browser", "Open sign-in page").on_click(
                            move |_, _, cx| {
                                if !preview {
                                    cx.open_url(&url);
                                }
                            },
                        ),
                    );
                if let Some(code) = &prompt.user_code {
                    body = body.child(
                        div()
                            .font_family(theme.FONT_MONO)
                            .child(format!("Device code: {code}")),
                    );
                }
                if prompt.input_kind != AuthInputKind::DeviceCode {
                    body = body
                        .child(div().text_color(theme.TEXT_DIM).child(
                            "Paste the returned code or full callback URL below. It stays private.",
                        ))
                        .child(state.input.clone())
                        .child(
                            login_button("login-paste", "Paste from clipboard").on_click(
                                cx.listener(|this, _, window, cx| this.paste_login(window, cx)),
                            ),
                        );
                }
                body = body.child(
                    login_button("login-submit", "Finish sign-in")
                        .on_click(cx.listener(|this, _, _, cx| this.submit_login(cx))),
                );
            } else {
                let provider = provider.clone();
                body = body.child(
                    login_button("login-retry", "Try again").on_click(cx.listener(
                        move |this, _, _, cx| this.select_login_provider(provider.clone(), cx),
                    )),
                );
            }
            body = body.child(
                login_button("login-back", "Choose another provider").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close_login_picker(cx);
                        this.open_login_picker(cx);
                    },
                )),
            );
        } else if !remote {
            body =
                body.child(div().text_color(theme.TEXT_DIM).child(
                    "Choose an account to connect or reconnect. Status is for this computer.",
                ));
            body = body.child(
                div().flex().items_center().gap_2().flex_wrap()
                    .child(div().flex_1().min_w_0().whitespace_normal().text_size(px(12.)).text_color(theme.TEXT_DIM)
                        .child("Green: working · Amber: unverified · Red: needs attention · Gray: not connected. Uses saved checks, not a live test."))
                    .child(login_button("login-refresh-status", if state.status_loading { "Checking…" } else { "Refresh status" })
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_login_status(cx)))),
            );
            for provider in &state.providers {
                let provider = provider.clone();
                let id = format!("login-provider-{}", provider.id);
                let status = connection::status_for(
                    state.statuses.as_ref(),
                    provider.id,
                    state.status_loading,
                );
                let color = status.color();
                let status_id = format!("login-status-{}", provider.id);
                let label = format!(
                    "{} · {}",
                    provider.display_name,
                    match provider.method {
                        LoginMethod::OAuth => "Browser sign-in",
                        LoginMethod::DeviceCode => "Device sign-in",
                        LoginMethod::ApiKey => "API key",
                    }
                );
                body = body.child(
                    div()
                        .id(SharedString::from(id.clone()))
                        .debug_selector(move || id.clone())
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(color.opacity(0.35))
                        .bg(color.opacity(0.045))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .cursor_pointer()
                        .hover(|el| el.bg(theme.ACCENT_DIM))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_login_provider(provider.clone(), cx)
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .flex_wrap()
                                .child(div().flex_1().min_w_0().child(label))
                                .child(
                                    div()
                                        .debug_selector(move || status_id.clone())
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(color.opacity(0.12))
                                        .text_size(px(11.))
                                        .text_color(color)
                                        .child(div().size(px(6.)).rounded_full().bg(color))
                                        .child(status.label()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.TEXT_DIM)
                                .child(status.detail()),
                        ),
                );
            }
        }
        Some(
            div()
                .id("login-dialog")
                .debug_selector(|| "login-dialog".into())
                .absolute()
                .inset_0()
                .size_full()
                .bg(theme.PANEL_BG)
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .flex()
                .flex_col()
                .p_4()
                .gap_3()
                .text_size(px(14.))
                .text_color(theme.TEXT)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Accounts"),
                        )
                        .child(
                            login_button(
                                "login-close",
                                if state.busy { "Cancel" } else { "Close" },
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.close_login_picker(cx);
                                    if this.is_accounts_panel() {
                                        cx.emit(AccountsPanelClosed);
                                    } else {
                                        this.focus_input(window, cx);
                                    }
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .id("login-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn paste_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = &self.login {
            state.input.update(cx, |input, cx| input.paste(window, cx));
        }
    }
}

fn login_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px_3()
        .py_2()
        .rounded_md()
        .bg(Theme::global().ACCENT_DIM)
        .text_color(Theme::global().TEXT)
        .cursor_pointer()
        .hover(|el| el.bg(Theme::global().PANEL_BORDER))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn login_status_badges_and_refresh_are_visible(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new_accounts("login-test", None, crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("accounts-panel").is_some());
        assert!(vcx.debug_bounds("login-status-openai").is_some());
        assert!(vcx.debug_bounds("login-refresh-status").is_some());
        panel.read_with(vcx, |panel, _| {
            let state = panel.login.as_ref().unwrap();
            assert!(!state.status_loading);
            assert_eq!(
                state.statuses.as_ref().unwrap()["openai"],
                ConnectionStatus::Connected
            );
        });
        let refresh = vcx.debug_bounds("login-refresh-status").unwrap();
        vcx.simulate_click(refresh.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("login-status-openai").is_some());
    }

    #[gpui::test]
    fn login_accounts_back_keeps_dedicated_panel_and_remote_does_not_probe(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new_accounts("login-test", None, crate::harness::spawn_inert(), cx)
        });
        panel.update(vcx, |panel, cx| {
            panel.login_command("/login openai-api", cx);
        });
        let back = vcx.debug_bounds("login-back").unwrap();
        vcx.simulate_click(back.center(), gpui::Modifiers::default());
        assert!(vcx.debug_bounds("accounts-panel").is_some());
        assert!(vcx.debug_bounds("login-provider-openai").is_some());
        panel.update(vcx, |panel, cx| {
            panel.close_login_picker(cx);
            panel.session_id = "accounts://ssh://test-host/session".into();
            panel.open_login_picker(cx);
            let state = panel.login.as_ref().unwrap();
            assert!(state.error.as_ref().unwrap().contains("another machine"));
            assert!(state.status_task.is_none());
        });
        assert!(vcx.debug_bounds("login-provider-openai").is_none());
    }

    #[gpui::test]
    fn native_login_is_clickable_private_and_preserves_draft(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = crate::harness::spawn_recording();
        let (panel, vcx) =
            cx.add_window_view(|_, cx| Panel::new("login-test".into(), None, None, bridge, cx));
        panel.update(vcx, |panel, cx| {
            panel.input.update(cx, |input, cx| {
                input.set_content("keep my draft".into(), cx)
            });
        });
        assert!(vcx.debug_bounds("panel-login").is_some());
        // Workspace tests exercise the footer opening a separate adjacent panel.
        panel.update(vcx, |panel, cx| panel.open_login_picker(cx));
        assert!(vcx.debug_bounds("login-dialog").is_some());
        let button = vcx
            .debug_bounds("login-provider-openai-api")
            .expect("API key choice");
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        assert!(vcx.debug_bounds("login-submit").is_some());
        vcx.simulate_input("typed-private-value");
        panel.read_with(vcx, |panel, cx| {
            assert!(!panel.login.as_ref().unwrap().input.read(cx).content_empty());
            assert_eq!(panel.input.read(cx).content.as_ref(), "keep my draft");
        });
        vcx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                "dummy-secret-not-for-chat".into(),
            ))
        });
        let paste = vcx.debug_bounds("login-paste").expect("mouse paste action");
        vcx.simulate_click(paste.center(), gpui::Modifiers::default());
        panel.read_with(vcx, |panel, cx| {
            let state = panel.login.as_ref().unwrap();
            assert!(!state.input.read(cx).content_empty());
            let snapshot = serde_json::to_string(&panel.snapshot(cx)).unwrap();
            assert!(!snapshot.contains("dummy-secret"));
            assert!(snapshot.contains("keep my draft"));
            assert!(panel.items.is_empty());
        });
        let close = vcx.debug_bounds("login-close").unwrap();
        vcx.simulate_click(close.center(), gpui::Modifiers::default());
        panel.read_with(vcx, |panel, cx| {
            assert!(panel.login.is_none());
            assert_eq!(panel.input.read(cx).content.as_ref(), "keep my draft");
        });
        assert!(
            commands.try_recv().is_err(),
            "login secrets must never be sent as messages"
        );
    }

    #[gpui::test]
    fn login_completion_refreshes_routes_and_offers_model_selection(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = crate::harness::spawn_recording();
        let (panel, vcx) =
            cx.add_window_view(|_, cx| Panel::new("login-test".into(), None, None, bridge, cx));
        panel.update(vcx, |panel, cx| {
            panel.open_login_picker(cx);
            panel.run_login_task(
                || {
                    Ok(LoginUpdate::Complete {
                        validation_warning: true,
                    })
                },
                cx,
            );
        });
        vcx.run_until_parked();
        assert!(
            matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "login-test")
        );
        panel.read_with(vcx, |panel, _| {
            let state = panel.login.as_ref().unwrap();
            assert!(state.complete);
            assert!(
                state
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("Credentials were saved")
            );
            assert!(panel.items.is_empty());
        });
        let choose = vcx
            .debug_bounds("login-choose-model")
            .expect("native post-login action");
        vcx.simulate_click(choose.center(), gpui::Modifiers::default());
        assert!(vcx.debug_bounds("recovery-model-picker").is_some());
        assert!(vcx.debug_bounds("login-dialog").is_none());
        assert!(
            commands.try_recv().is_err(),
            "login must not replay user prompts"
        );
    }

    #[gpui::test]
    fn login_slash_command_opens_ui_and_remote_login_never_touches_local_credentials(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "ssh://test-host/session".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(vcx, |panel, cx| {
            assert!(panel.handle_slash_command("/login openai", cx));
            let state = panel.login.as_ref().unwrap();
            assert!(state.error.as_ref().unwrap().contains("another machine"));
            assert!(state.provider.is_none());
            assert!(state.flow.is_none());
            assert!(state.task.is_none());
            assert!(!panel.login_command("/login-custom", cx));
        });
        assert!(vcx.debug_bounds("login-dialog").is_some());
        assert!(vcx.debug_bounds("login-provider-openai").is_none());
    }

    #[gpui::test]
    fn login_unknown_provider_and_empty_key_have_native_recovery(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "login-test".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(vcx, |panel, cx| {
            assert!(panel.handle_slash_command("/login does-not-exist", cx));
            assert!(panel.login.as_ref().unwrap().error.is_some());
            assert!(panel.handle_slash_command("/login openai-api", cx));
            panel.submit_login(cx);
            assert_eq!(
                panel.login.as_ref().unwrap().error.as_deref(),
                Some("Paste an API key first.")
            );
            assert!(panel.login.as_ref().unwrap().task.is_none());
            assert!(panel.items.is_empty());
        });
        assert!(vcx.debug_bounds("login-paste").is_some());
    }
}

#[cfg(test)]
#[path = "panel_preview_login_tests.rs"]
mod preview_tests;
