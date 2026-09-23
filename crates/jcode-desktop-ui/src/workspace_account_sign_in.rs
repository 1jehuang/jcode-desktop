//! Optional account onboarding. Device secrets stay in memory, never in workspace snapshots.
use super::*;
use crate::theme::ThemePreset;
use jcode_base::account_login::{self as auth, LoginPoll};
use jcode_base::external_auth::{self, ExternalAuthReviewCandidate};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    pub visible: bool,
    connected: bool,
    stage: Stage,
    error: Option<String>,
    keyboard_choice: Option<usize>,
    task: Option<gpui::Task<()>>,
    link_copied: bool,
    remaining: Option<Duration>,
    /// Logins left behind by other tools that Jcode can reuse in place.
    candidates: Vec<ExternalAuthReviewCandidate>,
    /// Parallel to `candidates`. Everything imports unless the row is skipped.
    checked: Vec<bool>,
    /// Provider ids Jcode is already signed in to, synced from the accounts
    /// feed each render. Detected logins that add nothing new are hidden.
    in_jcode: Vec<String>,
    detecting: bool,
    detect_task: Option<gpui::Task<()>>,
    /// Outlives the page so Continue can close immediately while importing.
    import_task: Option<gpui::Task<()>>,
    /// Hovering a swatch previews it until the user clicks one.
    theme_picked: bool,
    /// Live chat replay behind Continue. Dropped with the page.
    demo: Option<Demo>,
    #[cfg(test)]
    test_api_base: Option<String>,
}

struct Demo {
    panel: Entity<Panel>,
    _replay: gpui::Task<()>,
    _load: Option<gpui::Task<()>>,
}

/// Keyboard stops, in reading order: left-half controls, then the right half.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Login(usize),
    Theme,
    Primary,
    CopyLink,
    StartOver,
    Continue,
}

/// Real side effects (browser, credential import) only outside tests and
/// offline screenshots.
fn live() -> bool {
    !cfg!(test) && !harness::screenshot_mode()
}

/// Map a detected source's provider summary to a vendored logo id.
fn candidate_logo(summary: &str) -> &'static str {
    let first = summary.split(',').next().unwrap_or("").trim();
    match first {
        "OpenAI/Codex" => "openai",
        "Claude" => "claude",
        "Gemini" => "gemini",
        "Antigravity" => "antigravity",
        "GitHub Copilot" => "copilot",
        "Cursor" => "cursor",
        "OpenRouter/API-key providers" => "openrouter",
        _ => "jcode",
    }
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

    fn choices(&self) -> Vec<Choice> {
        let mut choices: Vec<Choice> = self.importable().into_iter().map(Choice::Login).collect();
        choices.push(Choice::Theme);
        match self.stage {
            Stage::Complete { .. } => {}
            Stage::Waiting { .. } => {
                choices.extend([Choice::Primary, Choice::CopyLink, Choice::StartOver])
            }
            _ if self.connected => {}
            _ => choices.push(Choice::Primary),
        }
        choices.push(Choice::Continue);
        choices
    }

    fn focused(&self, choice: Choice) -> bool {
        self.keyboard_choice
            .and_then(|index| self.choices().get(index).copied())
            == Some(choice)
    }

    /// Detected logins that would add a provider Jcode does not have yet.
    fn importable(&self) -> Vec<usize> {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                let ids = candidate.provider_ids();
                ids.is_empty() || ids.iter().any(|id| !self.in_jcode.iter().any(|known| known == id))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_imports(&self) -> Vec<usize> {
        self.importable()
            .into_iter()
            .filter(|&index| self.checked.get(index).copied().unwrap_or(false))
            .collect()
    }

    fn primary_label(&self) -> &'static str {
        match self.stage {
            Stage::Welcome if self.error.is_some() => "Try again",
            Stage::Welcome => "Sign in with email",
            Stage::Starting => "Connecting securely…",
            Stage::Waiting { .. } => "Open browser again",
            Stage::Complete { .. } => "Signed in",
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
            import_task: self.account_sign_in.import_task.take(),
            ..State::default()
        };
        self.detect_account_imports(cx);
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Discover reusable logins from other tools without reading their secrets.
    pub(super) fn detect_account_imports(&mut self, cx: &mut Context<Self>) {
        if !self.account_sign_in.visible || cfg!(test) {
            return;
        }
        if harness::screenshot_mode() {
            self.set_account_import_candidates(
                vec![
                    ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
                    ExternalAuthReviewCandidate::fixture("Gemini", "Gemini CLI"),
                    ExternalAuthReviewCandidate::fixture("GitHub Copilot", "VS Code"),
                ],
                cx,
            );
            return;
        }
        self.account_sign_in.detecting = true;
        let detect = cx.background_executor().spawn(async {
            external_auth::pending_external_auth_review_candidates().unwrap_or_default()
        });
        self.account_sign_in.detect_task = Some(cx.spawn(async move |this, cx| {
            let candidates = detect.await;
            let _ = this.update(cx, |this, cx| this.set_account_import_candidates(candidates, cx));
        }));
    }

    fn set_account_import_candidates(
        &mut self,
        candidates: Vec<ExternalAuthReviewCandidate>,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.account_sign_in;
        state.checked = vec![true; candidates.len()];
        state.candidates = candidates;
        state.detecting = false;
        state.detect_task = None;
        state.keyboard_choice = None;
        cx.notify();
    }

    fn toggle_account_import(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(checked) = self.account_sign_in.checked.get_mut(index) {
            *checked = !*checked;
            cx.notify();
        }
    }

    fn pick_account_theme(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.account_sign_in.theme_picked = true;
        self.select_theme(preset, cx);
    }

    fn hover_account_theme(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        if !self.account_sign_in.theme_picked {
            self.select_theme(preset, cx);
        }
    }

    /// Start the live chat replay behind Continue, once per visible page.
    /// A recent transcript from another harness replaces the built-in demo
    /// when one exists. It is only read, never imported or sent.
    fn ensure_account_demo(&mut self, cx: &mut Context<Self>) {
        if self.account_sign_in.demo.is_some() {
            return;
        }
        let panel = cx.new(|cx| Panel::new_demo("Demo", cx));
        let replay = crate::panel::demo_replay::run(
            panel.clone(),
            crate::panel::demo_replay::builtin_script(),
            cx,
        );
        let load = live().then(|| {
            let sample = cx
                .background_executor()
                .spawn(async { jcode_base::transcript_sample::recent_external_transcript() });
            cx.spawn(async move |this, cx| {
                let Some(sample) = sample.await else { return };
                let _ = this.update(cx, |this, cx| {
                    let Some(demo) = this.account_sign_in.demo.as_mut() else { return };
                    demo.panel.update(cx, |panel, cx| {
                        panel.title = format!("From {}", sample.source).into();
                        cx.notify();
                    });
                    demo._replay = crate::panel::demo_replay::run(demo.panel.clone(), sample.turns, cx);
                    demo._load = None;
                });
            })
        });
        self.account_sign_in.demo = Some(Demo { panel, _replay: replay, _load: load });
    }

    /// Right-half action: import the checked logins, then enter the workspace.
    fn continue_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.account_sign_in.selected_imports();
        let candidates = std::mem::take(&mut self.account_sign_in.candidates);
        if !selected.is_empty() && live() {
            let count = selected.len();
            let import = cx.background_executor().spawn(async move {
                network(async {
                    external_auth::run_external_auth_import_candidates_preserving_existing(
                        &candidates,
                        &selected,
                    )
                        .await
                })
            });
            self.account_sign_in.import_task = Some(cx.spawn(async move |this, cx| {
                let result = import.await;
                let _ = this.update(cx, |this, cx| {
                    this.status = match result {
                        Ok(Ok(outcome)) if outcome.imported == count => format!(
                            "Imported {count} login{}",
                            if count == 1 { "" } else { "s" }
                        ),
                        Ok(Ok(outcome)) => format!(
                            "Imported {} of {count} logins. Check Accounts for details.",
                            outcome.imported
                        ),
                        Ok(Err(error)) => format!("Could not import logins: {error}"),
                        Err(error) => format!("Could not import logins: {error}"),
                    };
                    this.account_sign_in.import_task = None;
                    accounts::request_refresh();
                    cx.notify();
                });
            }));
        }
        self.finish_account_sign_in(window, cx);
    }

    fn activate_account_choice(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            Choice::Primary => self.account_sign_in_primary(window, cx),
            Choice::CopyLink => self.copy_account_sign_in_link(cx),
            Choice::StartOver => self.reset_account_sign_in(cx),
            Choice::Login(index) => self.toggle_account_import(index, cx),
            Choice::Theme => self.pick_account_theme(Theme::active_preset().next(), cx),
            Choice::Continue => self.continue_account_sign_in(window, cx),
        }
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
        self.account_sign_in.detect_task = None;
        self.account_sign_in.demo = None;
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
        self.account_sign_in.link_copied = false;
        self.account_sign_in.remaining = None;
        cx.notify();
    }

    fn copy_account_sign_in_link(&mut self, cx: &mut Context<Self>) {
        if let Stage::Waiting { url } = &self.account_sign_in.stage {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
            self.account_sign_in.link_copied = true;
            cx.notify();
        }
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
            Stage::Complete { .. } => self.continue_account_sign_in(window, cx),
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
        self.account_sign_in.link_copied = false;
        self.account_sign_in.remaining = None;
        #[cfg(test)]
        let offline = self.account_sign_in.test_api_base.is_none();
        #[cfg(not(test))]
        let offline = false;
        if harness::screenshot_mode() || offline {
            // Explicit offline fixture, never send email or read/write credentials.
            self.account_sign_in.stage = Stage::Waiting {
                url: "https://jcode.sh/account".into(),
            };
            cx.notify();
            return;
        }
        self.account_sign_in.stage = Stage::Starting;
        #[cfg(test)]
        let api_base = self
            .account_sign_in
            .test_api_base
            .clone()
            .expect("explicit test endpoint");
        let request = cx.background_executor().spawn(async move {
            #[cfg(test)]
            let result = network(async {
                auth::start_with_api_base(&reqwest::Client::new(), &api_base).await
            });
            #[cfg(not(test))]
            let result = network(async { auth::start(&reqwest::Client::new()).await });
            result?.map_err(|error| error.to_string())
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
                this.account_sign_in.remaining = Some(flow.expires_in());
                if !cfg!(test) { cx.open_url(&url); }
                cx.notify();
            }).is_err() { return; }
            let mut interval = flow.interval();
            let expires_at = Instant::now() + flow.expires_in();
            loop {
                cx.background_executor().timer(interval.min(expires_at.saturating_duration_since(Instant::now()))).await;
                let _ = this.update(cx, |this, cx| {
                    this.account_sign_in.remaining = Some(expires_at.saturating_duration_since(Instant::now()));
                    cx.notify();
                });
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
                        let _ = this.update(cx, |this, cx| {
                            this.account_sign_in.error = Some(format!("The service is busy. Checking again in {} seconds.", interval.as_secs()));
                            cx.notify();
                        });
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
                            // Integrated tests exercise real HTTP but never touch a user credential.
                            #[cfg(test)]
                            let saved: Result<(), auth::AccountLoginError> = Ok(());
                            #[cfg(not(test))]
                            let saved = auth::save(&approved);
                            match saved {
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

    pub(super) fn render_account_sign_in(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        self.ensure_account_demo(cx);
        self.account_sign_in.in_jcode = self
            .accounts
            .iter()
            .filter(|account| account.available())
            .map(|account| account.id.clone())
            .collect();
        let theme = Theme::global();
        let narrow = window.viewport_size().width < px(760.0);
        let info = div()
            .id("account-sign-in-card")
            .debug_selector(|| "account-sign-in-card".into())
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px(px(if narrow { 24.0 } else { 56.0 }))
            .py(px(if narrow { 28.0 } else { 56.0 }))
            .child(
                div()
                    .w_full()
                    .max_w(px(520.0))
                    .flex()
                    .flex_col()
                    .gap(px(32.0))
                    .child(self.account_onboarding_header())
                    .child(self.account_onboarding_logins(cx))
                    .child(self.account_onboarding_theme(cx)),
            );
        let proceed = self.account_onboarding_continue(narrow, cx);
        div()
            .debug_selector(|| "account-sign-in".into())
            .size_full()
            .flex()
            .when(narrow, |el| el.flex_col())
            .overflow_hidden()
            .bg(theme.BG)
            .text_color(theme.TEXT)
            .font_family(theme.FONT_UI)
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
                    "enter" | "space" => {
                        let state = &this.account_sign_in;
                        let choice = state
                            .keyboard_choice
                            .and_then(|index| state.choices().get(index).copied())
                            .unwrap_or(Choice::Continue);
                        this.activate_account_choice(choice, window, cx);
                    }
                    "tab" => {
                        let count = this.account_sign_in.choices().len();
                        let back = event.keystroke.modifiers.shift;
                        this.account_sign_in.keyboard_choice =
                            Some(match this.account_sign_in.keyboard_choice {
                                None if back => count - 1,
                                None => 0,
                                Some(index) => (index + if back { count - 1 } else { 1 }) % count,
                            });
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(info)
            .child(proceed)
    }

    fn account_onboarding_header(&self) -> gpui::Div {
        let theme = Theme::global();
        let title = match &self.account_sign_in.stage {
            Stage::Waiting { .. } => "Finish signing in",
            Stage::Complete { .. } => "You're signed in",
            _ => "Welcome to Jcode",
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .debug_selector(|| "account-sign-in-brand".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .mb_2()
                    .child(
                        gpui::svg()
                            .data(accounts::logo("jcode").expect("vendored Jcode logo"))
                            .size(px(28.0))
                            .text_color(theme.TEXT),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Jcode Desktop"),
                    ),
            )
            .child(
                div()
                    .text_size(px(28.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(title),
            )
    }

    /// Jcode account sign-in, shown on the right above Continue.
    fn account_onboarding_account(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let waiting = matches!(state.stage, Stage::Waiting { .. });
        let mut section = div()
            .debug_selector(|| "account-sign-in-account".into())
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.TEXT_DIM)
                    .child("Jcode account · optional"),
            );
        let status = match &state.stage {
            Stage::Complete { email } => Some(format!("Signed in as {email}")),
            Stage::Welcome if state.connected => Some("Signed in to Jcode".to_string()),
            _ => None,
        };
        if let Some(status) = status {
            section = section.child(div().text_size(px(13.0)).text_color(theme.OK).child(status));
        }
        if !matches!(state.stage, Stage::Complete { .. }) && !state.connected {
            section = section.child(
                account_button(
                    "account-sign-in-primary",
                    state.primary_label(),
                    false,
                    state.focused(Choice::Primary),
                )
                .w_full()
                .border_color(if state.focused(Choice::Primary) { theme.ACCENT } else { theme.PANEL_BORDER })
                .when(matches!(state.stage, Stage::Starting), |el| el.opacity(0.6))
                .on_click(cx.listener(|this, _, window, cx| this.account_sign_in_primary(window, cx))),
            );
        }
        if waiting {
            let progress = state
                .remaining
                .map(|remaining| {
                    let seconds = remaining.as_secs();
                    format!("Waiting for approval · {}:{:02}", seconds / 60, seconds % 60)
                })
                .unwrap_or_else(|| "Waiting for approval".into());
            section = section
                .child(
                    div()
                        .debug_selector(|| "account-sign-in-progress".into())
                        .text_size(px(12.0))
                        .text_color(theme.TEXT_DIM)
                        .child(progress),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            account_button(
                                "account-sign-in-copy",
                                if state.link_copied { "Link copied" } else { "Copy link" },
                                false,
                                state.focused(Choice::CopyLink),
                            )
                            .text_size(px(12.0))
                            .py_1()
                            .on_click(cx.listener(|this, _, _, cx| this.copy_account_sign_in_link(cx))),
                        )
                        .child(
                            account_button(
                                "account-sign-in-back",
                                "Start over",
                                false,
                                state.focused(Choice::StartOver),
                            )
                            .text_size(px(12.0))
                            .py_1()
                            .on_click(cx.listener(|this, _, _, cx| this.reset_account_sign_in(cx))),
                        ),
                );
        }
        if let Some(error) = &state.error {
            section = section.child(
                div()
                    .debug_selector(|| "account-sign-in-error".into())
                    .max_w(px(320.0))
                    .text_center()
                    .text_size(px(13.0))
                    .text_color(theme.ERROR)
                    .child(error.clone()),
            );
        }
        section
    }

    /// Two sets: what Jcode can already use, then what other tools have
    /// that Jcode could import. Logins Jcode already has are not offered again.
    fn account_onboarding_logins(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let in_jcode: Vec<_> = self
            .accounts
            .iter()
            .filter(|account| account.available() && account.id != "jcode")
            .collect();
        let importable = state.importable();
        let mut section = section("AI provider logins");

        section = section.child(subheading("In Jcode"));
        if in_jcode.is_empty() {
            section = section.child(empty_note("account-logins-in-jcode-empty", "None yet"));
        } else {
            let mut list = div()
                .debug_selector(|| "account-logins-in-jcode".into())
                .flex()
                .flex_col()
                .gap_1();
            for account in in_jcode {
                list = list.child(login_row(
                    &account.id,
                    account.display_name.clone(),
                    account.method.clone(),
                    div()
                        .mr_1()
                        .px_3()
                        .py(px(3.0))
                        .rounded_full()
                        .bg(theme.OK.opacity(0.12))
                        .text_color(theme.OK)
                        .child(account.status_label()),
                ));
            }
            section = section.child(list);
        }

        section = section.child(subheading("Can import"));
        if importable.is_empty() {
            let note = if state.detecting {
                "Looking in other tools…"
            } else if state.candidates.is_empty() {
                "Nothing found in other tools"
            } else {
                "Nothing new. Jcode already has these."
            };
            return section.child(empty_note("account-logins-import-empty", note));
        }
        let mut list = div().flex().flex_col().gap_1();
        for index in importable {
            let candidate = &state.candidates[index];
            let checked = state.checked.get(index).copied().unwrap_or(false);
            let toggle = account_button(
                if checked { "account-import-skip" } else { "account-import-undo" },
                if checked { "Skip" } else { "Import" },
                false,
                state.focused(Choice::Login(index)),
            )
            .id(("account-import-toggle", index))
            .debug_selector(move || format!("account-import-toggle-{index}"))
            .text_size(px(12.0))
            .px_3()
            .py_1()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_account_import(index, cx)));
            let row = login_row(
                candidate_logo(candidate.provider_summary()),
                candidate.provider_summary().to_string(),
                if checked {
                    format!("from {}", candidate.source_name())
                } else {
                    format!("from {} · skipped", candidate.source_name())
                },
                div().child(toggle),
            )
            .id(("account-import", index))
            .debug_selector(move || format!("account-import-{index}"))
            .when(!checked, |el| el.opacity(0.55));
            list = list.child(row);
        }
        section.child(list)
    }

    fn account_onboarding_theme(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let active = Theme::active_preset();
        let mut grid = div()
            .debug_selector(|| "account-theme-grid".into())
            .flex()
            .flex_wrap()
            .gap_3()
            .rounded_xl()
            .border_1()
            .border_color(if state.focused(Choice::Theme) {
                theme.ACCENT
            } else {
                gpui::transparent_black().into()
            });
        for (index, preset) in ThemePreset::ALL.into_iter().enumerate() {
            grid = grid.child(
                div()
                    .id(("account-theme", index))
                    .debug_selector(move || format!("account-theme-{index}"))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.hover_account_theme(preset, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_account_theme(preset, cx)))
                    .child(theme_swatch(preset, preset == active))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(if preset == active { theme.TEXT } else { theme.TEXT_DIM })
                            .child(preset.label()),
                    ),
            );
        }
        section("Theme").child(grid)
    }

    /// The right half: a live chat replay behind a card holding the Jcode
    /// account sign-in and a Continue button.
    fn account_onboarding_continue(&self, narrow: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let focused = state.focused(Choice::Continue);
        let mut container = div()
            .id("account-sign-in-right")
            .debug_selector(|| "account-sign-in-right".into())
            .when(narrow, |el| el.h(px(260.0)).flex_none().w_full())
            .when(!narrow, |el| el.flex_1().h_full())
            .min_w(px(0.0))
            .relative()
            .overflow_hidden()
            .bg(theme.PANEL_BG);
        if let Some(demo) = &state.demo {
            container = container.child(
                div()
                    .debug_selector(|| "account-sign-in-demo".into())
                    .absolute()
                    .inset_0()
                    .p(px(if narrow { 8.0 } else { 20.0 }))
                    .child(demo.panel.clone()),
            );
        }
        let proceed = div()
            .id("account-sign-in-continue")
            .debug_selector(|| "account-sign-in-continue".into())
            .flex()
            .items_center()
            .justify_center()
            .gap_3()
            .px(px(28.0))
            .py(px(12.0))
            .rounded_full()
            .bg(theme.ACCENT)
            .text_color(theme.BG)
            .text_size(px(16.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .shadow_lg()
            .border_2()
            .border_color(if focused { theme.TEXT } else { theme.ACCENT })
            .cursor_pointer()
            .hover(|el| el.opacity(0.9))
            .on_click(cx.listener(|this, _, window, cx| this.continue_account_sign_in(window, cx)))
            .child("Continue")
            .child("→");
        container.child(
            div()
                .id("account-sign-in-overlay")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(px(if narrow { 12.0 } else { 32.0 }))
                .bg(theme.BG.opacity(0.7))
                .child(
                    div()
                        .debug_selector(|| "account-sign-in-panel".into())
                        .w_full()
                        .max_w(px(360.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(if narrow { 12.0 } else { 20.0 }))
                        .px(px(if narrow { 16.0 } else { 28.0 }))
                        .py(px(if narrow { 14.0 } else { 28.0 }))
                        .rounded(px(28.0))
                        .bg(theme.BG)
                        .border_1()
                        .border_color(theme.PANEL_BORDER)
                        .shadow_lg()
                        .child(self.account_onboarding_account(cx))
                        .child(proceed.w_full()),
                ),
        )
    }
}

fn section(title: &'static str) -> gpui::Div {
    div().flex().flex_col().gap_3().child(
        div()
            .text_size(px(15.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(title),
    )
}

fn subheading(text: &'static str) -> gpui::Div {
    div()
        .text_size(px(11.0))
        .text_color(Theme::global().TEXT_DIM)
        .child(text)
}

fn empty_note(id: &'static str, text: &'static str) -> gpui::Div {
    div()
        .debug_selector(move || id.into())
        .text_size(px(13.0))
        .text_color(Theme::global().TEXT_DIM)
        .child(text)
}

fn login_row(logo: &str, name: String, detail: String, trailing: gpui::Div) -> gpui::Div {
    let theme = Theme::global();
    let mut row = div()
        .flex()
        .items_center()
        .gap_3()
        .pl(px(8.0))
        .pr(px(8.0))
        .py(px(6.0))
        .rounded_full()
        .bg(theme.PANEL_BG);
    if let Some(data) = accounts::logo(logo) {
        row = row.child(
            div()
                .size(px(32.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme.TEXT.opacity(0.06))
                .child(gpui::svg().data(data).size(px(16.0)).text_color(theme.TEXT)),
        );
    }
    row.child(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(div().text_size(px(13.0)).truncate().child(name))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.TEXT_DIM)
                    .truncate()
                    .child(detail),
            ),
    )
    .child(div().flex_none().text_size(px(12.0)).child(trailing))
}

/// A miniature workspace painted in the preset's own palette.
fn theme_swatch(preset: ThemePreset, active: bool) -> gpui::Div {
    let current = Theme::global();
    let t = Theme::preview(preset);
    let line = |width: f32, color: gpui::Rgba| div().h(px(3.0)).w(px(width)).rounded_full().bg(color);
    div()
        .w(px(92.0))
        .h(px(58.0))
        .p(px(6.0))
        .flex()
        .gap(px(4.0))
        .rounded_xl()
        .bg(t.BG)
        .border_2()
        .border_color(if active { current.ACCENT } else { current.PANEL_BORDER })
        .child(
            div()
                .w(px(14.0))
                .h_full()
                .rounded_md()
                .bg(t.HEADER_BG),
        )
        .child(
            div()
                .flex_1()
                .h_full()
                .p(px(5.0))
                .rounded_md()
                .bg(t.PANEL_BG)
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(line(34.0, t.TEXT))
                .child(line(46.0, t.TEXT_DIM))
                .child(line(24.0, t.ACCENT))
                .child(line(38.0, t.TEXT_DIM)),
        )
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
        .min_w_0()
        .px_4()
        .py_2()
        .rounded_full()
        .border_1()
        .border_color(if focused {
            theme.ACCENT
        } else {
            gpui::transparent_black().into()
        })
        .bg(if primary {
            theme.ACCENT.opacity(0.16)
        } else {
            theme.TEXT.opacity(0.06)
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
