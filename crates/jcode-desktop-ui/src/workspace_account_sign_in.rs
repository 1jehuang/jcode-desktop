//! Optional account onboarding. Device secrets stay in memory, never in workspace snapshots.
use super::*;
use crate::theme::ThemePreset;
use jcode_base::account_login::{self as auth, EmailCodeResult, EmailLogin};
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
    /// Single-line field for the email, then the emailed code.
    input: Option<Entity<PromptInput>>,
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
    #[cfg(test)]
    opened_url: Option<String>,
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
    Field,
    Primary,
    OpenGmail,
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
    Sending,
    /// The code was emailed. `login` is None only for offline fixtures.
    Code {
        email: String,
        login: Option<Arc<EmailLogin>>,
    },
    Verifying {
        email: String,
        login: Option<Arc<EmailLogin>>,
    },
    Complete {
        email: String,
    },
}

impl Stage {
    fn code_step(&self) -> Option<(&str, Option<Arc<EmailLogin>>)> {
        match self {
            Stage::Code { email, login } | Stage::Verifying { email, login } => {
                Some((email.as_str(), login.clone()))
            }
            _ => None,
        }
    }
}

fn looks_like_email(text: &str) -> bool {
    let text = text.trim();
    let Some((local, domain)) = text.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !text.chars().any(char::is_whitespace)
}

fn code_digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
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
            Stage::Code { .. } | Stage::Verifying { .. } => {
                choices.extend([Choice::Field, Choice::Primary, Choice::OpenGmail, Choice::StartOver])
            }
            _ if self.connected => {}
            _ => choices.extend([Choice::Field, Choice::Primary]),
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
            Stage::Welcome => "Sign in with email",
            Stage::Sending => "Sending code…",
            Stage::Code { .. } => "Verify",
            Stage::Verifying { .. } => "Verifying…",
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
            Choice::Field => self.focus_account_input(window, cx),
            Choice::Primary => self.account_sign_in_primary(window, cx),
            Choice::OpenGmail => self.open_account_gmail(cx),
            Choice::StartOver => self.reset_account_sign_in(window, cx),
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
        self.account_sign_in.input = None;
        self.account_sign_in.stage = Stage::Welcome;
        self.account_sign_in.error = None;
        self.restore_focus(window, cx);
        cx.notify();
    }

    /// Back to the email field, keeping what was typed.
    fn reset_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let email = self.account_sign_in.stage.code_step().map(|(email, _)| email.to_owned());
        self.account_sign_in.task = None;
        self.account_sign_in.stage = Stage::Welcome;
        self.account_sign_in.error = None;
        self.account_sign_in.keyboard_choice = None;
        self.replace_account_input(email.unwrap_or_default(), window, cx);
        cx.notify();
    }

    /// Single-line field reused for the email and then the code.
    fn ensure_account_input(&mut self, cx: &mut Context<Self>) -> Entity<PromptInput> {
        if let Some(input) = &self.account_sign_in.input {
            return input.clone();
        }
        let input = self.new_account_input(String::new(), cx);
        self.account_sign_in.input = Some(input.clone());
        input
    }

    fn new_account_input(&mut self, content: String, cx: &mut Context<Self>) -> Entity<PromptInput> {
        let code = self.account_sign_in.stage.code_step().is_some();
        let submit = cx.weak_entity();
        let change = cx.weak_entity();
        cx.new(|cx| {
            let mut input = PromptInput::new(
                cx,
                if code { "6-digit code" } else { "you@example.com" },
                move |text, _, window, app| {
                    // Deferred: the field is still mid-update when Enter fires.
                    let submit = submit.clone();
                    let handle = window.window_handle();
                    app.defer(move |app| {
                        let _ = handle.update(app, |_, window, app| {
                            let _ = submit
                                .update(app, |this, cx| this.submit_account_field(text, window, cx));
                        });
                    });
                },
            )
            .without_command_completion()
            .with_on_change(move |text, app| {
                // Six digits verify without an extra click. Deferred so the
                // field is not borrowed while the workspace reacts.
                let text = text.to_owned();
                let change = change.clone();
                app.defer(move |app| {
                    let _ = change.update(app, |this, cx| {
                        if this.account_sign_in.error.take().is_some() {
                            cx.notify();
                        }
                        if matches!(this.account_sign_in.stage, Stage::Code { .. })
                            && code_digits(&text).len() == 6
                        {
                            this.verify_account_code(code_digits(&text), cx);
                        }
                    });
                });
            });
            if !content.is_empty() {
                input.set_content(content, cx);
            }
            input
        })
    }

    fn replace_account_input(&mut self, content: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.new_account_input(content, cx);
        let focus = input.read(cx).focus_handle.clone();
        self.account_sign_in.input = Some(input);
        window.focus(&focus, cx);
    }

    fn focus_account_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.ensure_account_input(cx).read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    }

    /// Enter in the field, or the primary button with the field's text.
    fn submit_account_field(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        match &self.account_sign_in.stage {
            Stage::Welcome => self.start_account_sign_in(text, window, cx),
            Stage::Code { .. } => {
                let digits = code_digits(&text);
                if digits.len() == 6 {
                    self.verify_account_code(digits, cx);
                } else {
                    self.account_sign_in.error = Some("Enter the 6-digit code from the email.".into());
                    if let Some(input) = self.account_sign_in.input.clone() {
                        input.update(cx, |input, cx| input.set_content(text, cx));
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn account_sign_in_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.account_sign_in.stage {
            Stage::Welcome | Stage::Code { .. } => {
                let text = self.ensure_account_input(cx).read(cx).content.to_string();
                self.submit_account_field(text, window, cx);
            }
            Stage::Complete { .. } => self.continue_account_sign_in(window, cx),
            Stage::Sending | Stage::Verifying { .. } => {}
        }
    }

    fn account_sign_in_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.account_sign_in.error = Some(error);
        self.account_sign_in.keyboard_choice = None;
        cx.notify();
    }

    fn start_account_sign_in(&mut self, email: String, window: &mut Window, cx: &mut Context<Self>) {
        let email = email.trim().to_owned();
        self.account_sign_in.keyboard_choice = None;
        if !looks_like_email(&email) {
            self.account_sign_in.error = Some(if email.is_empty() {
                "Enter your email to sign in.".into()
            } else {
                "Enter a valid email address.".into()
            });
            if let Some(input) = self.account_sign_in.input.clone() {
                input.update(cx, |input, cx| input.set_content(email, cx));
            }
            cx.notify();
            return;
        }
        self.account_sign_in.error = None;
        #[cfg(test)]
        let offline = self.account_sign_in.test_api_base.is_none();
        #[cfg(not(test))]
        let offline = false;
        if harness::screenshot_mode() || offline {
            // Explicit offline fixture, never send email or read/write credentials.
            self.account_sign_in.stage = Stage::Code { email, login: None };
            self.replace_account_input(String::new(), window, cx);
            cx.notify();
            return;
        }
        self.account_sign_in.stage = Stage::Sending;
        #[cfg(test)]
        let api_base = self
            .account_sign_in
            .test_api_base
            .clone()
            .expect("explicit test endpoint");
        let address = email.clone();
        let request = cx.background_executor().spawn(async move {
            #[cfg(test)]
            let result = network(async {
                auth::start_email_with_api_base(&reqwest::Client::new(), &api_base, &address).await
            });
            #[cfg(not(test))]
            let result = network(async { auth::start_email(&reqwest::Client::new(), &address).await });
            result?.map_err(|error| auth::email_start_error_message(&error))
        });
        self.account_sign_in.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.account_sign_in.task = None;
                match result {
                    Ok(login) => {
                        this.account_sign_in.stage = Stage::Code {
                            email: login.email().to_owned(),
                            login: Some(Arc::new(login)),
                        };
                        this.replace_account_input(String::new(), window, cx);
                    }
                    Err(error) => {
                        this.account_sign_in.stage = Stage::Welcome;
                        this.replace_account_input(email, window, cx);
                        this.account_sign_in_failed(error, cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn verify_account_code(&mut self, code: String, cx: &mut Context<Self>) {
        let Stage::Code { email, login } = std::mem::take(&mut self.account_sign_in.stage) else {
            return;
        };
        self.account_sign_in.error = None;
        let Some(login) = login else {
            // Offline fixture: any six digits sign in without touching credentials.
            self.account_sign_in_approved(email, cx);
            return;
        };
        self.account_sign_in.stage = Stage::Verifying { email: email.clone(), login: Some(login.clone()) };
        let pending = login.clone();
        let request = cx.background_executor().spawn(async move {
            network(async { auth::verify_email(&reqwest::Client::new(), &pending, &code).await })
        });
        self.account_sign_in.task = Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                this.account_sign_in.task = None;
                let retry = |this: &mut Self, message: String, cx: &mut Context<Self>| {
                    this.account_sign_in.stage = Stage::Code { email: email.clone(), login: Some(login.clone()) };
                    if let Some(input) = this.account_sign_in.input.clone() {
                        input.update(cx, |input, cx| input.set_content(String::new(), cx));
                    }
                    this.account_sign_in_failed(message, cx);
                };
                match result {
                    Ok(Ok(EmailCodeResult::Approved(approved))) => {
                        // Only the still-live UI operation may commit credentials.
                        #[cfg(test)]
                        let saved: Result<(), auth::AccountLoginError> = Ok(());
                        #[cfg(not(test))]
                        let saved = auth::save(&approved);
                        match saved {
                            Ok(()) => this.account_sign_in_approved(approved.email, cx),
                            Err(error) => retry(this, error.to_string(), cx),
                        }
                    }
                    Ok(Ok(EmailCodeResult::Incorrect { attempts_remaining })) => retry(
                        this,
                        match attempts_remaining {
                            Some(1) => "That code is not right. 1 try left.".into(),
                            Some(n) => format!("That code is not right. {n} tries left."),
                            None => "That code is not right.".into(),
                        },
                        cx,
                    ),
                    Ok(Ok(EmailCodeResult::Expired)) => {
                        this.account_sign_in.stage = Stage::Welcome;
                        this.account_sign_in.input = None;
                        this.account_sign_in.input = Some(this.new_account_input(email.clone(), cx));
                        this.account_sign_in_failed(
                            "That code expired. Send a new one, or skip for now.".into(),
                            cx,
                        );
                    }
                    Ok(Err(error)) => retry(this, error.to_string(), cx),
                    Err(error) => retry(this, error, cx),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Gmail, filtered to our sign-in email and including Spam.
    fn open_account_gmail(&mut self, cx: &mut Context<Self>) {
        let Some((email, _)) = self.account_sign_in.stage.code_step() else { return };
        let url = auth::gmail_search_link(email);
        #[cfg(test)]
        {
            self.account_sign_in.opened_url = Some(url);
        }
        #[cfg(not(test))]
        if live() {
            cx.open_url(&url);
        }
        cx.notify();
    }

    fn account_sign_in_approved(&mut self, email: String, cx: &mut Context<Self>) {
        self.account_sign_in.connected = true;
        self.account_sign_in.stage = Stage::Complete { email };
        self.account_sign_in.input = None;
        self.account_sign_in.error = crate::config::persist_account_sign_in_handled()
            .err()
            .map(|_| "Signed in, but the welcome preference could not be saved.".into());
        self.account_sign_in.keyboard_choice = None;
        accounts::request_refresh();
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
                let typing = this
                    .account_sign_in
                    .input
                    .as_ref()
                    .is_some_and(|input| input.read(cx).focus_handle.is_focused(window));
                match event.keystroke.key.as_str() {
                    "escape" => this.finish_account_sign_in(window, cx),
                    // The field owns Enter (submit) and Space (text).
                    "enter" | "space" if typing => return,
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
                        let state = &this.account_sign_in;
                        let field = state
                            .keyboard_choice
                            .and_then(|index| state.choices().get(index).copied())
                            == Some(Choice::Field);
                        if field {
                            this.focus_account_input(window, cx);
                        } else {
                            window.focus(&this.focus_handle, cx);
                        }
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
            Stage::Code { .. } | Stage::Verifying { .. } => "Check your email",
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

    /// Jcode account sign-in: one email field, then the emailed code.
    fn account_onboarding_account(&mut self, narrow: bool, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::global();
        let signed_in = match &self.account_sign_in.stage {
            Stage::Complete { email } => Some(format!("Signed in as {email}")),
            Stage::Welcome if self.account_sign_in.connected => Some("Signed in to Jcode".to_string()),
            _ => None,
        };
        let mut section = div()
            .debug_selector(|| "account-sign-in-account".into())
            .w_full()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(status) = signed_in {
            section = section.child(
                div()
                    .debug_selector(|| "account-sign-in-status".into())
                    .px_2()
                    .text_size(px(13.0))
                    .text_color(theme.OK)
                    .child(status),
            );
        } else {
            let input = self.ensure_account_input(cx);
            let state = &self.account_sign_in;
            let busy = matches!(state.stage, Stage::Sending | Stage::Verifying { .. });
            let code = state.stage.code_step().map(|(email, _)| email.to_owned());
            let field_focused = state.focused(Choice::Field);
            section = section
                .when_some(code.clone(), |el, email| {
                    el.child(
                        div()
                            .debug_selector(|| "account-sign-in-sent".into())
                            .px_2()
                            .text_size(px(12.0))
                            .text_color(theme.TEXT_DIM)
                            .child(format!("We emailed a code to {email}")),
                    )
                })
                .child(
                    div()
                        .flex()
                        .when(narrow, |el| el.flex_col())
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("account-sign-in-field")
                                .debug_selector(|| "account-sign-in-field".into())
                                .flex_1()
                                .w_full()
                                .min_w_0()
                                .rounded(px(19.0))
                                .border_1()
                                .border_color(if field_focused {
                                    theme.ACCENT
                                } else {
                                    gpui::transparent_black().into()
                                })
                                .when(busy, |el| el.opacity(0.6))
                                .child(input),
                        )
                        .child(
                            account_button(
                                "account-sign-in-primary",
                                state.primary_label(),
                                true,
                                state.focused(Choice::Primary),
                            )
                            .flex_none()
                            .when(narrow, |el| el.w_full())
                            .when(busy, |el| el.opacity(0.6))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.account_sign_in_primary(window, cx)
                            })),
                        ),
                )
                .when(code.is_some(), |el| {
                    el.child(
                        div().flex().gap_2().px_1().child(
                            account_button(
                                "account-sign-in-gmail",
                                "Open Gmail",
                                false,
                                state.focused(Choice::OpenGmail),
                            )
                            .text_size(px(12.0))
                            .px_3()
                            .py_1()
                            .on_click(cx.listener(|this, _, _, cx| this.open_account_gmail(cx))),
                        )
                        .child(
                            account_button(
                                "account-sign-in-back",
                                "Use another email",
                                false,
                                state.focused(Choice::StartOver),
                            )
                            .text_size(px(12.0))
                            .px_3()
                            .py_1()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_account_sign_in(window, cx)
                            })),
                        ),
                    )
                });
        }
        if let Some(error) = &self.account_sign_in.error {
            section = section.child(
                div()
                    .debug_selector(|| "account-sign-in-error".into())
                    .px_2()
                    .text_size(px(12.0))
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
            let toggle = div()
                .id(("account-import-toggle", index))
                .debug_selector(move || format!("account-import-toggle-{index}"))
                .cursor_pointer()
                .child(import_slider(checked, state.focused(Choice::Login(index))))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_account_import(index, cx)));
            let row = login_row(
                candidate_logo(candidate.provider_summary()),
                candidate.provider_summary().to_string(),
                format!("from {}", candidate.source_name()),
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

    /// The right half: the live chat replay, fully visible, with a compact
    /// sign-in bar pinned along the bottom and a tiny skip icon in the corner.
    fn account_onboarding_continue(&mut self, narrow: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = Theme::global();
        let focused = self.account_sign_in.focused(Choice::Continue);
        let complete = matches!(self.account_sign_in.stage, Stage::Complete { .. })
            || self.account_sign_in.connected;
        let mut container = div()
            .id("account-sign-in-right")
            .debug_selector(|| "account-sign-in-right".into())
            .when(narrow, |el| el.h(px(340.0)).flex_none().w_full())
            .when(!narrow, |el| el.flex_1().h_full())
            .min_w(px(0.0))
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.PANEL_BG);
        let demo = self.account_sign_in.demo.as_ref().map(|demo| demo.panel.clone());
        container = container.child(
            div()
                .debug_selector(|| "account-sign-in-demo".into())
                .flex_1()
                .min_h(px(0.0))
                .p(px(if narrow { 8.0 } else { 20.0 }))
                .children(demo),
        );
        let skip = div()
            .id("account-sign-in-continue")
            .debug_selector(|| "account-sign-in-continue".into())
            .absolute()
            .top(px(if narrow { 8.0 } else { 14.0 }))
            .right(px(if narrow { 8.0 } else { 14.0 }))
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_1()
            .border_color(if focused { theme.ACCENT } else { gpui::transparent_black().into() })
            .bg(theme.BG.opacity(0.6))
            .text_color(theme.TEXT_DIM)
            .cursor_pointer()
            .hover(|el| el.bg(theme.TEXT.opacity(0.1)).text_color(theme.TEXT))
            .tooltip(move |_, cx| {
                cx.new(|_| {
                    super::remotes::HeaderTooltip(if complete { "Continue" } else { "Skip for now" }.into())
                })
                .into()
            })
            .on_click(cx.listener(|this, _, window, cx| this.continue_account_sign_in(window, cx)))
            .child(
                gpui::svg()
                    .data(include_bytes!("../../../assets/icons/skip.svg") as &'static [u8])
                    .size(px(12.0))
                    .text_color(if focused { theme.TEXT } else { theme.TEXT_DIM }),
            );
        let bar = div()
            .debug_selector(|| "account-sign-in-panel".into())
            .flex_none()
            .w_full()
            .px(px(if narrow { 12.0 } else { 20.0 }))
            .py(px(if narrow { 10.0 } else { 14.0 }))
            .border_t_1()
            .border_color(theme.PANEL_BORDER)
            .bg(theme.BG)
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(520.0))
                    .child(self.account_onboarding_account(narrow, cx)),
            );
        container.child(bar).child(skip)
    }
}

/// Import on the left, Skip on the right, with a knob that slides between.
fn import_slider(importing: bool, focused: bool) -> gpui::Div {
    let theme = Theme::global();
    let label = |text: &'static str, active: bool| {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.0))
            .text_color(if active { theme.BG } else { theme.TEXT_DIM })
            .child(text)
    };
    let (width, knob) = (px(116.0), px(58.0));
    div()
        .relative()
        .w(width)
        .h(px(26.0))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(if focused { theme.ACCENT } else { gpui::transparent_black().into() })
        .bg(theme.TEXT.opacity(0.08))
        .child(
            div()
                .absolute()
                .top(px(2.0))
                .left(if importing { px(2.0) } else { width - knob - px(4.0) })
                .w(knob)
                .h(px(20.0))
                .rounded_full()
                .bg(if importing { theme.ACCENT } else { theme.TEXT_DIM }),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .child(label("Import", importing))
                .child(label("Skip", !importing)),
        )
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
            if primary { theme.TEXT } else { theme.ACCENT }
        } else {
            gpui::transparent_black().into()
        })
        .bg(if primary { theme.ACCENT } else { theme.TEXT.opacity(0.06) })
        .text_size(px(14.0))
        .text_color(if primary { theme.BG } else { theme.TEXT })
        .text_center()
        .cursor_pointer()
        .hover(move |el| {
            if primary { el.opacity(0.9) } else { el.bg(theme.ACCENT.opacity(0.08)) }
        })
        .child(label)
}

#[cfg(test)]
#[path = "workspace_account_sign_in_tests.rs"]
mod tests;
