//! Optional account onboarding. Device secrets stay in memory, never in workspace snapshots.
use super::*;
use jcode_base::account_login::{self as auth, LoginPoll};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    pub visible: bool,
    connected: bool,
    stage: Stage,
    error: Option<String>,
    keyboard_choice: Option<usize>,
    task: Option<gpui::Task<()>>,
}

#[derive(Default)]
enum Stage {
    #[default]
    Welcome,
    Starting,
    Waiting {
        url: String,
    },
    Complete {
        email: String,
    },
}

fn should_offer(handled: bool, connected: bool, fixture: bool) -> bool {
    !handled && !connected && !fixture
}

impl State {
    pub(super) fn startup() -> Self {
        let fixture = harness::screenshot_mode();
        let connected = !fixture && auth::has_credentials();
        let preview = fixture
            && std::env::var("JCODE_DESKTOP_SCREENSHOT_ACCOUNT_SIGN_IN").as_deref() == Ok("1");
        Self {
            visible: preview
                || should_offer(crate::config::account_sign_in_handled(), connected, fixture),
            connected,
            ..Self::default()
        }
    }

    fn primary_label(&self) -> &'static str {
        match self.stage {
            Stage::Welcome if self.error.is_some() => "Try again",
            Stage::Welcome => "Sign in with email",
            Stage::Starting => "Opening secure sign-in…",
            Stage::Waiting { .. } => "Open browser again",
            Stage::Complete { .. } => "Continue to workspace",
        }
    }
}

// Reqwest needs Tokio, whereas GPUI uses its own executor. Each bounded request
// runs off the UI thread. Dropping the owning GPUI task prevents stale results
// from opening a browser or storing a credential after Skip, Back, or reload.
fn network<T>(operation: impl std::future::Future<Output = T>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start sign-in. Please try again.".to_string())
        .map(|runtime| runtime.block_on(operation))
}

impl Workspace {
    fn open_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account_sign_in = State {
            visible: true,
            connected: self.account_sign_in.connected,
            ..State::default()
        };
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn finish_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Skip always works, even if the preference cannot be written. Surface
        // the failure in the workspace rather than trapping the user here.
        if crate::config::persist_account_sign_in_handled().is_err() {
            self.status =
                "Could not save the welcome preference. Sign-in may be offered next launch.".into();
        }
        self.account_sign_in.visible = false;
        self.account_sign_in.task = None;
        self.account_sign_in.stage = Stage::Welcome;
        self.account_sign_in.error = None;
        self.restore_focus(window, cx);
        cx.notify();
    }

    fn reset_account_sign_in(&mut self, cx: &mut Context<Self>) {
        self.account_sign_in.task = None;
        self.account_sign_in.stage = Stage::Welcome;
        self.account_sign_in.error = None;
        self.account_sign_in.keyboard_choice = None;
        cx.notify();
    }

    fn account_sign_in_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.account_sign_in.stage {
            Stage::Welcome => self.start_account_sign_in(cx),
            Stage::Starting => {}
            Stage::Waiting { url } => {
                if !harness::screenshot_mode() && !cfg!(test) {
                    cx.open_url(url);
                }
            }
            Stage::Complete { .. } => self.finish_account_sign_in(window, cx),
        }
    }

    fn account_sign_in_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.account_sign_in.stage = Stage::Welcome;
        self.account_sign_in.error = Some(error);
        self.account_sign_in.keyboard_choice = None;
        cx.notify();
    }

    fn start_account_sign_in(&mut self, cx: &mut Context<Self>) {
        self.account_sign_in.error = None;
        self.account_sign_in.keyboard_choice = None;
        if harness::screenshot_mode() || cfg!(test) {
            // Explicit offline fixture, never send email or read/write credentials.
            self.account_sign_in.stage = Stage::Waiting {
                url: "https://jcode.sh/account".into(),
            };
            cx.notify();
            return;
        }
        self.account_sign_in.stage = Stage::Starting;
        let request = cx.background_executor().spawn(async {
            network(async { auth::start(&reqwest::Client::new()).await })?
                .map_err(|error| error.to_string())
        });
        self.account_sign_in.task = Some(cx.spawn(async move |this, cx| {
            let flow = match request.await {
                Ok(flow) => Arc::new(flow),
                Err(error) => {
                    let _ = this.update(cx, |this, cx| this.account_sign_in_failed(error, cx));
                    return;
                }
            };
            let url = flow.auth_url().to_owned();
            if this.update(cx, |this, cx| {
                this.account_sign_in.stage = Stage::Waiting { url: url.clone() };
                cx.open_url(&url);
                cx.notify();
            }).is_err() { return; }
            let mut interval = flow.interval();
            let expires_at = Instant::now() + flow.expires_in();
            loop {
                cx.background_executor().timer(interval.min(expires_at.saturating_duration_since(Instant::now()))).await;
                if flow.is_expired() {
                    let _ = this.update(cx, |this, cx| this.account_sign_in_failed(
                        "This sign-in expired. Try again for a new link, or skip for now.".into(), cx));
                    return;
                }
                let pending = flow.clone();
                let result = cx.background_executor().spawn(async move {
                    network(async { auth::poll(&reqwest::Client::new(), &pending).await })
                }).await;
                match result {
                    Ok(Ok(LoginPoll::Pending)) => {
                        let _ = this.update(cx, |this, cx| {
                            if this.account_sign_in.error.take().is_some() { cx.notify(); }
                        });
                    },
                    Ok(Ok(LoginPoll::SlowDown { retry_after })) => {
                        interval = retry_after.max(interval);
                    },
                    Ok(Err(error)) if error.is_temporary() => {
                        interval = (interval + Duration::from_secs(2)).min(Duration::from_secs(30)).max(interval);
                        let _ = this.update(cx, |this, cx| {
                            this.account_sign_in.error = Some("Connection interrupted. Retrying until this sign-in expires. You can still skip.".into());
                            cx.notify();
                        });
                    },
                    Ok(Ok(LoginPoll::Approved(approved))) => {
                        let _ = this.update(cx, |this, cx| {
                            // Only the still-live UI operation may commit credentials.
                            match auth::save(&approved) {
                                Ok(()) => {
                                    this.account_sign_in.connected = true;
                                    this.account_sign_in.stage = Stage::Complete { email: approved.email };
                                    this.account_sign_in.error = crate::config::persist_account_sign_in_handled().err()
                                        .map(|_| "Signed in, but the welcome preference could not be saved.".into());
                                    this.account_sign_in.keyboard_choice = None;
                                    cx.notify();
                                },
                                Err(error) => this.account_sign_in_failed(error.to_string(), cx),
                            }
                        });
                        return;
                    },
                    other => {
                        let message = match other {
                            Ok(Ok(LoginPoll::Expired)) => "This sign-in expired. Try again for a new link, or skip for now.".into(),
                            Ok(Ok(LoginPoll::Denied)) => "Sign-in was not approved. Try again, or skip for now.".into(),
                            Ok(Err(error)) => error.to_string(),
                            Err(error) => error,
                            _ => unreachable!(),
                        };
                        let _ = this.update(cx, |this, cx| this.account_sign_in_failed(message, cx));
                        return;
                    },
                }
            }
        }));
        cx.notify();
    }

    pub(super) fn render_account_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let connected = self.account_sign_in.connected;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .py_3()
            .child(div().text_size(px(12.0)).child("Jcode account"))
            .child(
                div()
                    .text_color(Theme::global().TEXT_DIM)
                    .child(if connected {
                        "Account saved on this computer"
                    } else {
                        "Optional · Separate from your AI providers"
                    }),
            )
            .child(
                account_button(
                    "settings-account-sign-in",
                    if connected {
                        "Manage account"
                    } else {
                        "Sign in with email"
                    },
                    false,
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if connected {
                        cx.open_url("https://jcode.sh/account");
                    } else {
                        this.open_account_sign_in(window, cx);
                    }
                })),
            )
    }

    pub(super) fn render_account_sign_in(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let waiting = matches!(state.stage, Stage::Waiting { .. });
        let complete = matches!(state.stage, Stage::Complete { .. });
        let (title, description) = match &state.stage {
            Stage::Welcome | Stage::Starting => (
                "Welcome to Jcode Desktop",
                "Sign in to your Jcode account with a magic link. No password to remember.",
            ),
            Stage::Waiting { .. } => (
                "Finish signing in",
                "Enter your email in the browser, then open the magic link in that same browser and approve this device. Desktop will connect automatically.",
            ),
            Stage::Complete { .. } => (
                "You're signed in",
                "Your Jcode account is connected. You're ready to make something.",
            ),
        };
        let mut card = div()
            .id("account-sign-in-card")
            .debug_selector(|| "account-sign-in-card".into())
            .w_full()
            .max_w(px(520.0))
            .max_h_full()
            .overflow_y_scroll()
            .p_8()
            .rounded_xl()
            .bg(Theme::global().PANEL_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .child("JCODE  /  DESKTOP"),
            )
            .child(
                div()
                    .text_size(px(30.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(title),
            )
            .child(div().text_size(px(15.0)).child(description));
        if let Stage::Complete { email } = &state.stage {
            card = card.child(div().text_size(px(14.0)).child(email.clone()));
        } else {
            card = card.child(div().p_4().rounded_lg().bg(Theme::global().HEADER_BG)
                .text_size(px(13.0)).text_color(Theme::global().TEXT_DIM)
                .child(if waiting { "Waiting for approval · You can leave this screen at any time." }
                    else { "Use your own AI providers with or without a Jcode account. Signing in does not start a paid plan." }));
        }
        if let Some(error) = &state.error {
            card = card.child(
                div()
                    .debug_selector(|| "account-sign-in-error".into())
                    .text_size(px(13.0))
                    .text_color(Theme::global().TEXT)
                    .child(error.clone()),
            );
        }
        card = card.child(
            account_button(
                "account-sign-in-primary",
                state.primary_label(),
                true,
                state.keyboard_choice == Some(0),
            )
            .when(matches!(state.stage, Stage::Starting), |el| el.opacity(0.6))
            .on_click(cx.listener(|this, _, window, cx| this.account_sign_in_primary(window, cx))),
        );
        if !complete {
            card = card.child(
                account_button(
                    "account-sign-in-skip",
                    "Skip for now",
                    false,
                    state.keyboard_choice == Some(1),
                )
                .on_click(
                    cx.listener(|this, _, window, cx| this.finish_account_sign_in(window, cx)),
                ),
            );
        }
        if waiting {
            card = card.child(
                account_button(
                    "account-sign-in-back",
                    "Cancel and go back",
                    false,
                    state.keyboard_choice == Some(2),
                )
                .on_click(cx.listener(|this, _, _, cx| this.reset_account_sign_in(cx))),
            );
        }
        card = card.child(
            div()
                .text_size(px(12.0))
                .text_color(Theme::global().TEXT_DIM)
                .child(if complete {
                    "Your AI provider and model settings are unchanged."
                } else {
                    "You can always sign in later from Settings. Esc skips."
                }),
        );
        div()
            .debug_selector(|| "account-sign-in".into())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_5()
            .bg(Theme::global().BG)
            .text_color(Theme::global().TEXT)
            .font_family(Theme::global().FONT_UI)
            .track_focus(&self.focus_handle)
            .capture_action(cx.listener(|this, _: &crate::input::Clear, window, cx| {
                this.finish_account_sign_in(window, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.is_held {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" => this.finish_account_sign_in(window, cx),
                    "enter" | "space" => match this.account_sign_in.keyboard_choice.unwrap_or(0) {
                        1 => this.finish_account_sign_in(window, cx),
                        2 => this.reset_account_sign_in(cx),
                        _ => this.account_sign_in_primary(window, cx),
                    },
                    "tab" => {
                        let count = match this.account_sign_in.stage {
                            Stage::Complete { .. } => 1,
                            Stage::Waiting { .. } => 3,
                            _ => 2,
                        };
                        this.account_sign_in.keyboard_choice =
                            Some(match this.account_sign_in.keyboard_choice {
                                None => {
                                    if event.keystroke.modifiers.shift {
                                        count - 1
                                    } else {
                                        0
                                    }
                                }
                                Some(index) => {
                                    (index
                                        + if event.keystroke.modifiers.shift {
                                            count - 1
                                        } else {
                                            1
                                        })
                                        % count
                                }
                            });
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(card)
    }
}

fn account_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    focused: bool,
) -> gpui::Stateful<gpui::Div> {
    let theme = Theme::global();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px_4()
        .py_3()
        .rounded_md()
        .border_1()
        .border_color(if focused {
            theme.ACCENT
        } else {
            gpui::transparent_black().into()
        })
        .bg(if primary {
            theme.ACCENT.opacity(0.16)
        } else {
            gpui::transparent_black().into()
        })
        .text_size(px(14.0))
        .text_color(theme.TEXT)
        .text_center()
        .cursor_pointer()
        .hover(move |el| el.bg(theme.ACCENT.opacity(if primary { 0.25 } else { 0.08 })))
        .child(label)
}

#[cfg(test)]
#[path = "workspace_account_sign_in_tests.rs"]
mod tests;
