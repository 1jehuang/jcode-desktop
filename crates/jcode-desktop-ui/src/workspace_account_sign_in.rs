//! Optional account onboarding. Device secrets stay in memory, never in workspace snapshots.
use super::*;
use crate::theme::ThemePreset;
use jcode_base::account_login::{self as auth, LoginPoll};
use jcode_base::external_auth::{self, ExternalAuthReviewCandidate};
use std::sync::Arc;

const PRICING_URL: &str = "https://jcode.sh/pricing";

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
    /// Parallel to `candidates`. Everything starts checked, "Import less" opts out.
    checked: Vec<bool>,
    choosing: bool,
    detecting: bool,
    detect_task: Option<gpui::Task<()>>,
    /// Outlives the page so Continue can close immediately while importing.
    import_task: Option<gpui::Task<()>>,
    telemetry: Telemetry,
    #[cfg(test)]
    test_api_base: Option<String>,
}

/// Mirrors the CLI onboarding's three telemetry levels, most sharing first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Telemetry {
    Everything,
    #[default]
    UsageOnly,
    Off,
}

impl Telemetry {
    const ALL: [Self; 3] = [Self::Everything, Self::UsageOnly, Self::Off];

    fn label(self) -> &'static str {
        match self {
            Self::Everything => "Everything",
            Self::UsageOnly => "Usage only",
            Self::Off => "Off",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Everything => "Usage stats, crash reports, and prompt content. Helps us improve Jcode the most.",
            Self::UsageOnly => "Anonymous usage stats and crash reports. Never your prompts or code.",
            Self::Off => "Nothing is sent.",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Everything => "account-telemetry-everything",
            Self::UsageOnly => "account-telemetry-usage",
            Self::Off => "account-telemetry-off",
        }
    }

    fn live() -> bool {
        !cfg!(test) && !harness::screenshot_mode()
    }

    fn current() -> Self {
        use jcode_base::telemetry;
        if !Self::live() {
            Self::default()
        } else if !telemetry::is_enabled() {
            Self::Off
        } else if telemetry::content_sharing_enabled() {
            Self::Everything
        } else {
            Self::UsageOnly
        }
    }

    /// Environment opt-outs always win, so the choice is read-only then.
    fn locked() -> bool {
        Self::live() && jcode_base::telemetry::opt_out_forced_by_env()
    }

    fn persist(self) {
        use jcode_base::telemetry;
        if !Self::live() {
            return;
        }
        match self {
            Self::Everything => {
                telemetry::set_usage_telemetry_enabled(true);
                telemetry::set_content_sharing_enabled(true);
            }
            Self::UsageOnly => {
                telemetry::set_usage_telemetry_enabled(true);
                telemetry::set_content_sharing_enabled(false);
            }
            Self::Off => {
                telemetry::set_content_sharing_enabled(false);
                telemetry::set_usage_telemetry_enabled(false);
            }
        }
    }
}

/// Keyboard stops, in reading order: left-half controls, then Continue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Primary,
    CopyLink,
    StartOver,
    Subscribe,
    ImportLess,
    Login(usize),
    Theme,
    Telemetry(Telemetry),
    Continue,
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
            telemetry: Telemetry::current(),
            ..Self::default()
        }
    }

    fn choices(&self) -> Vec<Choice> {
        let mut choices = Vec::new();
        match self.stage {
            Stage::Complete { .. } => {}
            Stage::Waiting { .. } => {
                choices.extend([Choice::Primary, Choice::CopyLink, Choice::StartOver])
            }
            _ => choices.push(Choice::Primary),
        }
        if !self.connected {
            choices.push(Choice::Subscribe);
        }
        if !self.candidates.is_empty() {
            choices.push(Choice::ImportLess);
            if self.choosing {
                choices.extend((0..self.candidates.len()).map(Choice::Login));
            }
        }
        choices.push(Choice::Theme);
        if !Telemetry::locked() {
            choices.extend(Telemetry::ALL.map(Choice::Telemetry));
        }
        choices.push(Choice::Continue);
        choices
    }

    fn focused(&self, choice: Choice) -> bool {
        self.keyboard_choice
            .and_then(|index| self.choices().get(index).copied())
            == Some(choice)
    }

    fn selected_imports(&self) -> Vec<usize> {
        (0..self.candidates.len())
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
            telemetry: Telemetry::current(),
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

    fn toggle_account_import_less(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.account_sign_in;
        state.choosing = !state.choosing;
        if !state.choosing {
            state.checked.iter_mut().for_each(|checked| *checked = true);
        }
        state.keyboard_choice = state
            .choices()
            .iter()
            .position(|choice| *choice == Choice::ImportLess);
        cx.notify();
    }

    fn select_account_telemetry(&mut self, level: Telemetry, cx: &mut Context<Self>) {
        if Telemetry::locked() {
            return;
        }
        self.account_sign_in.telemetry = level;
        level.persist();
        cx.notify();
    }

    fn open_account_pricing(&mut self, cx: &mut Context<Self>) {
        if Telemetry::live() {
            cx.open_url(PRICING_URL);
        }
    }

    /// Right-half action: import the checked logins, then enter the workspace.
    fn continue_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.account_sign_in.selected_imports();
        let candidates = std::mem::take(&mut self.account_sign_in.candidates);
        if !selected.is_empty() && Telemetry::live() {
            let count = selected.len();
            let import = cx.background_executor().spawn(async move {
                network(async {
                    external_auth::run_external_auth_auto_import_candidates(&candidates, &selected)
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
            Choice::Subscribe => self.open_account_pricing(cx),
            Choice::ImportLess => self.toggle_account_import_less(cx),
            Choice::Login(index) => self.toggle_account_import(index, cx),
            Choice::Theme => self.select_theme(Theme::active_preset().next(), cx),
            Choice::Telemetry(level) => self.select_account_telemetry(level, cx),
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
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
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
                    .gap(px(36.0))
                    .child(self.account_onboarding_header())
                    .child(self.account_onboarding_account(cx))
                    .child(self.account_onboarding_logins(cx))
                    .child(self.account_onboarding_theme(cx))
                    .child(self.account_onboarding_telemetry(cx))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme.TEXT_DIM)
                            .child("Everything here can be changed later in Settings. Esc skips without importing."),
                    ),
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
        let (title, description) = match &self.account_sign_in.stage {
            Stage::Waiting { .. } => (
                "Finish signing in",
                "Approve this device in your browser. You can keep setting up while you wait.",
            ),
            Stage::Complete { .. } => (
                "You're signed in",
                "Your Jcode account is connected. Review the rest, then continue.",
            ),
            _ => (
                "Welcome to Jcode Desktop",
                "A few choices before you start. Defaults are fine, so you can just continue.",
            ),
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
            .child(div().text_size(px(14.0)).text_color(theme.TEXT_DIM).child(description))
    }

    fn account_onboarding_account(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let waiting = matches!(state.stage, Stage::Waiting { .. });
        let mut section = section("Jcode account").child(
            div().text_size(px(13.0)).child(match &state.stage {
                Stage::Waiting { .. } => "Enter your email in the browser, open the magic link in that same browser, then approve this device.".to_string(),
                Stage::Complete { email } => format!("Signed in as {email}"),
                _ if state.connected => "Signed in on this computer.".to_string(),
                _ => "Optional. Sign in with an email magic link, no password. Subscribe for Jcode's hosted models and cloud features.".to_string(),
            }),
        );
        if waiting {
            let progress = state
                .remaining
                .map(|remaining| {
                    let seconds = remaining.as_secs();
                    format!(
                        "Waiting for approval · Link expires in {}:{:02}",
                        seconds / 60,
                        seconds % 60
                    )
                })
                .unwrap_or_else(|| "Waiting for approval".into());
            section = section.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(12.0))
                    .child(
                        div()
                            .debug_selector(|| "account-sign-in-progress".into())
                            .child(progress),
                    )
                    .child(div().text_color(theme.TEXT_DIM).child(
                        "Desktop connects automatically. Copy the link if your browser did not open.",
                    )),
            );
        }
        if let Some(error) = &state.error {
            section = section.child(
                div()
                    .debug_selector(|| "account-sign-in-error".into())
                    .text_size(px(13.0))
                    .text_color(theme.ERROR)
                    .child(error.clone()),
            );
        }
        let mut actions = div().flex().flex_wrap().gap_2();
        if !matches!(state.stage, Stage::Complete { .. }) && !state.connected {
            actions = actions.child(
                account_button(
                    "account-sign-in-primary",
                    state.primary_label(),
                    true,
                    state.focused(Choice::Primary),
                )
                .when(matches!(state.stage, Stage::Starting), |el| el.opacity(0.6))
                .on_click(cx.listener(|this, _, window, cx| this.account_sign_in_primary(window, cx))),
            );
        }
        if waiting {
            actions = actions
                .child(
                    account_button(
                        "account-sign-in-copy",
                        if state.link_copied { "Link copied" } else { "Copy link" },
                        false,
                        state.focused(Choice::CopyLink),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.copy_account_sign_in_link(cx))),
                )
                .child(
                    account_button(
                        "account-sign-in-back",
                        "Start over",
                        false,
                        state.focused(Choice::StartOver),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.reset_account_sign_in(cx))),
                );
        }
        if !state.connected {
            actions = actions.child(
                account_button(
                    "account-sign-in-subscribe",
                    "Subscribe to Jcode",
                    false,
                    state.focused(Choice::Subscribe),
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_account_pricing(cx))),
            );
        }
        section.child(actions)
    }

    fn account_onboarding_logins(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let connected: Vec<_> = self
            .accounts
            .iter()
            .filter(|account| account.available() && account.id != "jcode")
            .collect();
        let mut section = section("AI provider logins");
        if connected.is_empty() && state.candidates.is_empty() {
            return section.child(
                div()
                    .debug_selector(|| "account-logins-empty".into())
                    .text_size(px(13.0))
                    .text_color(theme.TEXT_DIM)
                    .child(if state.detecting {
                        "Looking for existing logins…"
                    } else {
                        "No existing logins found. Connect Claude, OpenAI, and others later from Accounts in the sidebar."
                    }),
            );
        }
        if !connected.is_empty() {
            section = section.child(subheading("Already connected to Jcode"));
            let mut list = div().flex().flex_col().gap_1();
            for account in connected {
                list = list.child(login_row(
                    &account.id,
                    account.display_name.clone(),
                    account.method.clone(),
                    div().text_color(theme.OK).child(account.status_label()),
                ));
            }
            section = section.child(list);
        }
        if !state.candidates.is_empty() {
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(subheading("Found in other tools"))
                    .child(
                        account_button(
                            "account-import-less",
                            if state.choosing { "Import all" } else { "Import less" },
                            false,
                            state.focused(Choice::ImportLess),
                        )
                        .text_size(px(12.0))
                        .py_1()
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_account_import_less(cx))),
                    ),
            );
            let mut list = div().flex().flex_col().gap_1();
            for (index, candidate) in state.candidates.iter().enumerate() {
                let checked = state.checked.get(index).copied().unwrap_or(false);
                let trailing = if state.choosing {
                    checkbox(checked, state.focused(Choice::Login(index)))
                } else {
                    div().text_color(theme.TEXT_DIM).child("Will import")
                };
                let mut row = login_row(
                    candidate_logo(candidate.provider_summary()),
                    candidate.provider_summary().to_string(),
                    format!("from {}", candidate.source_name()),
                    trailing,
                )
                .id(("account-import", index))
                .debug_selector(move || format!("account-import-{index}"))
                .when(state.choosing && !checked, |el| el.opacity(0.55));
                if state.choosing {
                    row = row
                        .cursor_pointer()
                        .hover(move |el| el.bg(theme.TOOL_BG))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_account_import(index, cx)
                        }));
                }
                list = list.child(row);
            }
            section = section.child(list).child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.TEXT_DIM)
                    .child("Jcode reads these in place when you continue. The originals are never moved or changed."),
            );
        }
        section
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
            .rounded_md()
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
                    .on_click(cx.listener(move |this, _, _, cx| this.select_theme(preset, cx)))
                    .child(theme_swatch(preset, preset == active))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(if preset == active { theme.TEXT } else { theme.TEXT_DIM })
                            .child(preset.label()),
                    ),
            );
        }
        section("Theme")
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme.TEXT_DIM)
                    .child(format!("{} · Applies instantly, so this page is the preview.", active.label())),
            )
            .child(grid)
    }

    fn account_onboarding_telemetry(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        if Telemetry::locked() {
            return section("Telemetry").child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme.TEXT_DIM)
                    .child("Off. Disabled by JCODE_NO_TELEMETRY or DO_NOT_TRACK in your environment."),
            );
        }
        let mut options = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .p_1()
            .rounded_md()
            .bg(theme.PANEL_BG);
        for level in Telemetry::ALL {
            let selected = state.telemetry == level;
            let focused = state.focused(Choice::Telemetry(level));
            options = options.child(
                div()
                    .id(level.id())
                    .debug_selector(move || level.id().into())
                    .flex_1()
                    .min_w(px(96.0))
                    .px_3()
                    .py_1p5()
                    .rounded_sm()
                    .text_center()
                    .text_size(px(13.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if focused { theme.ACCENT } else { gpui::transparent_black().into() })
                    .when(selected, |el| el.bg(theme.ACCENT.opacity(0.18)))
                    .when(!selected, |el| el.hover(move |el| el.bg(theme.TOOL_BG)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_account_telemetry(level, cx)))
                    .child(level.label()),
            );
        }
        section("Telemetry").child(options).child(
            div()
                .text_size(px(12.0))
                .text_color(theme.TEXT_DIM)
                .child(state.telemetry.detail()),
        )
    }

    fn account_onboarding_continue(&self, narrow: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let imports = state.selected_imports().len();
        let mut recap = Vec::new();
        if imports > 0 {
            recap.push(format!("Import {imports} login{}", if imports == 1 { "" } else { "s" }));
        }
        recap.push(format!("{} theme", Theme::active_preset().label()));
        recap.push(format!("Telemetry: {}", state.telemetry.label().to_lowercase()));
        let focused = state.focused(Choice::Continue);
        div()
            .id("account-sign-in-continue")
            .debug_selector(|| "account-sign-in-continue".into())
            .when(narrow, |el| el.h(px(150.0)).flex_none().w_full())
            .when(!narrow, |el| el.flex_1().h_full())
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p(px(32.0))
            .bg(theme.PANEL_BG)
            .border_1()
            .border_color(if focused { theme.ACCENT } else { gpui::transparent_black().into() })
            .cursor_pointer()
            .hover(move |el| el.bg(theme.ACCENT.opacity(0.10)))
            .on_click(cx.listener(|this, _, window, cx| this.continue_account_sign_in(window, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_size(px(if narrow { 24.0 } else { 32.0 }))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Continue")
                    .child(div().text_color(theme.ACCENT).child("→")),
            )
            .child(
                div()
                    .debug_selector(|| "account-sign-in-recap".into())
                    .text_center()
                    .text_size(px(12.0))
                    .text_color(theme.TEXT_DIM)
                    .child(recap.join(" · ")),
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

fn login_row(logo: &str, name: String, detail: String, trailing: gpui::Div) -> gpui::Div {
    let theme = Theme::global();
    let mut row = div()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(theme.PANEL_BG);
    if let Some(data) = accounts::logo(logo) {
        row = row.child(gpui::svg().data(data).size(px(18.0)).flex_none().text_color(theme.TEXT));
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

fn checkbox(checked: bool, focused: bool) -> gpui::Div {
    let theme = Theme::global();
    div()
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(if focused || checked { theme.ACCENT } else { theme.PANEL_BORDER })
        .when(checked, |el| el.bg(theme.ACCENT.opacity(0.25)))
        .text_size(px(12.0))
        .child(if checked { "✓" } else { "" })
}

/// A miniature workspace painted in the preset's own palette.
fn theme_swatch(preset: ThemePreset, active: bool) -> gpui::Div {
    let current = Theme::global();
    let t = Theme::preview(preset);
    let line = |width: f32, color: gpui::Rgba| div().h(px(3.0)).w(px(width)).rounded_sm().bg(color);
    div()
        .w(px(92.0))
        .h(px(58.0))
        .p(px(6.0))
        .flex()
        .gap(px(4.0))
        .rounded_md()
        .bg(t.BG)
        .border_2()
        .border_color(if active { current.ACCENT } else { current.PANEL_BORDER })
        .child(
            div()
                .w(px(14.0))
                .h_full()
                .rounded_sm()
                .bg(t.HEADER_BG),
        )
        .child(
            div()
                .flex_1()
                .h_full()
                .p(px(5.0))
                .rounded_sm()
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
