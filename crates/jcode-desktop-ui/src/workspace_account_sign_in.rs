//! Optional account onboarding. Device secrets stay in memory, never in workspace snapshots.
use super::*;
use crate::theme::ThemePreset;
use jcode_base::account_login::{self as auth, EmailCodeResult, EmailLogin};
use jcode_base::detected_emails::{self, DetectedEmail};
use jcode_base::external_auth::{self, ExternalAuthReviewCandidate};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    pub visible: bool,
    connected: bool,
    /// Offer the email sign-in field. Off while `EMAIL_SIGN_IN` is paused.
    email: bool,
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
    /// Addresses other AI tools are signed in with, offered as the Jcode email.
    emails: Vec<DetectedEmail>,
    /// Which detected address fills the field. None once the user types their own.
    picked_email: Option<usize>,
    emails_task: Option<gpui::Task<()>>,
    /// Theme choice is optional, so the swatches stay folded until asked for.
    theme_expanded: bool,
    /// The clicked theme. Hovering other swatches still previews them, and
    /// leaving the grid returns to this one.
    theme_picked: Option<ThemePreset>,
    /// Live chat replay on the right half. Dropped with the page.
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

/// Keyboard stops, in the left column's reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Email(usize),
    Field,
    Primary,
    OpenGmail,
    StartOver,
    Login(usize),
    ThemeToggle,
    Theme,
    Finish,
}

/// Real side effects (browser, credential import) only outside tests and
/// offline screenshots.
fn live() -> bool {
    !cfg!(test) && !harness::screenshot_mode()
}

/// The onboarding rehearsal (`scripts/onboarding-desktop.py`) mirrors other
/// tools' logins with every secret redacted. Detection is real, but importing
/// those placeholders would only store unusable credentials, so a rehearsal
/// reports what it would import instead.
fn rehearsal() -> bool {
    std::env::var_os("JCODE_ONBOARDING_REHEARSAL").is_some()
}

/// Email sign-in for the Jcode account. Addresses detected in other coding
/// agents' logins are offered first so most people only confirm one.
const EMAIL_SIGN_IN: bool = true;

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
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
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
            email: EMAIL_SIGN_IN,
            ..Self::default()
        }
    }

    fn choices(&self) -> Vec<Choice> {
        let mut choices: Vec<Choice> = self.email_choices().map(Choice::Email).collect();
        match self.stage {
            Stage::Complete { .. } => {}
            Stage::Code { .. } | Stage::Verifying { .. } => choices.extend([
                Choice::Field,
                Choice::Primary,
                Choice::OpenGmail,
                Choice::StartOver,
            ]),
            _ if self.connected || !self.email => {}
            _ => choices.extend([Choice::Field, Choice::Primary]),
        }
        choices.extend(self.importable().into_iter().map(Choice::Login));
        choices.push(Choice::ThemeToggle);
        if self.theme_expanded {
            choices.push(Choice::Theme);
        }
        choices.push(Choice::Finish);
        choices
    }

    fn focused(&self, choice: Choice) -> bool {
        self.keyboard_choice
            .and_then(|index| self.choices().get(index).copied())
            == Some(choice)
    }

    /// Detected addresses are only offered before a code is requested.
    fn email_choices(&self) -> std::ops::Range<usize> {
        let offered = self.email && !self.connected && matches!(self.stage, Stage::Welcome);
        0..if offered { self.emails.len() } else { 0 }
    }

    /// Detected logins that would add a provider Jcode does not have yet.
    fn importable(&self) -> Vec<usize> {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                let ids = candidate.provider_ids();
                ids.is_empty()
                    || ids
                        .iter()
                        .any(|id| !self.in_jcode.iter().any(|known| known == id))
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

    /// The email or code field is on the page and accepts typing.
    fn field_open(&self) -> bool {
        self.email && !self.connected && !matches!(self.stage, Stage::Complete { .. })
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
    pub(super) fn open_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account_sign_in = State {
            visible: true,
            connected: self.account_sign_in.connected,
            email: self.account_sign_in.email,
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
            self.set_account_emails(
                vec![
                    DetectedEmail {
                        email: "ada@example.com".into(),
                        sources: vec!["Codex", "Claude Code"],
                    },
                    DetectedEmail {
                        email: "ada@work.example".into(),
                        sources: vec!["Gemini CLI"],
                    },
                ],
                cx,
            );
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
        if self.account_sign_in.email && !self.account_sign_in.connected {
            // Only the address is read from each tool's file, never a secret.
            let emails = cx
                .background_executor()
                .spawn(async { detected_emails::detected_emails() });
            self.account_sign_in.emails_task = Some(cx.spawn(async move |this, cx| {
                let emails = emails.await;
                let _ = this.update(cx, |this, cx| this.set_account_emails(emails, cx));
            }));
        }
        let detect = cx.background_executor().spawn(async {
            external_auth::pending_external_auth_review_candidates().unwrap_or_default()
        });
        self.account_sign_in.detect_task = Some(cx.spawn(async move |this, cx| {
            let candidates = detect.await;
            let _ = this.update(cx, |this, cx| {
                this.set_account_import_candidates(candidates, cx)
            });
        }));
    }

    /// Prefill the field with the first detected address unless the user
    /// already typed something.
    fn set_account_emails(&mut self, emails: Vec<DetectedEmail>, cx: &mut Context<Self>) {
        let typed = self
            .account_sign_in
            .input
            .as_ref()
            .is_some_and(|input| !input.read(cx).content.trim().is_empty());
        let state = &mut self.account_sign_in;
        state.emails = emails;
        state.emails_task = None;
        state.keyboard_choice = None;
        state.picked_email = None;
        if !typed && let Some(first) = state.emails.first() {
            let address = first.email.clone();
            state.picked_email = Some(0);
            let input = self.ensure_account_input(cx);
            input.update(cx, |input, cx| input.set_content(address, cx));
        }
        cx.notify();
    }

    /// Use a detected address for Jcode: it fills the sign-in field, and the
    /// arrow beside it sends the code.
    fn pick_account_email(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(address) = self
            .account_sign_in
            .emails
            .get(index)
            .map(|e| e.email.clone())
        else {
            return;
        };
        self.account_sign_in.picked_email = Some(index);
        self.account_sign_in.error = None;
        let input = self.ensure_account_input(cx);
        input.update(cx, |input, cx| input.set_content(address, cx));
        cx.notify();
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

    fn toggle_account_theme_picker(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.account_sign_in;
        state.theme_expanded = !state.theme_expanded;
        // Keep keyboard focus on the toggle as the swatch stop comes and goes.
        state.keyboard_choice = state.keyboard_choice.and(
            state
                .choices()
                .iter()
                .position(|c| *c == Choice::ThemeToggle),
        );
        cx.notify();
    }

    fn pick_account_theme(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.account_sign_in.theme_picked = Some(preset);
        self.select_theme(preset, cx);
    }

    fn hover_account_theme(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.select_theme(preset, cx);
    }

    /// Back to the clicked theme once the pointer leaves the swatches.
    fn restore_account_theme(&mut self, cx: &mut Context<Self>) {
        if let Some(preset) = self.account_sign_in.theme_picked {
            self.select_theme(preset, cx);
        }
    }

    /// Start the live chat replay on the right half, once per visible page.
    /// The user's own longest sessions across every harness (Jcode, Claude
    /// Code, Codex, Cursor and Pi) replace the built-in demo when they exist
    /// and replay in turn. They are only read, never imported or sent.
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
            let samples = cx.background_executor().spawn(async {
                jcode_base::transcript_sample::recent_external_transcripts(SHOWCASE_TRANSCRIPTS)
            });
            cx.spawn(async move |this, cx| {
                let samples = samples.await;
                if samples.is_empty() {
                    return;
                }
                let _ = this.update(cx, |this, cx| {
                    let Some(demo) = this.account_sign_in.demo.as_mut() else {
                        return;
                    };
                    let showcase = samples
                        .into_iter()
                        .map(|sample| (Some(format!("From {}", sample.source)), sample.turns))
                        .collect();
                    demo._replay =
                        crate::panel::demo_replay::run_showcase(demo.panel.clone(), showcase, cx);
                    demo._load = None;
                });
            })
        });
        self.account_sign_in.demo = Some(Demo {
            panel,
            _replay: replay,
            _load: load,
        });
    }

    /// The model and login the first session will use: connected logins plus
    /// the imports still switched on, ranked like the runtime's post-import
    /// default. `None` when nothing usable is connected or selected.
    fn account_onboarding_identity(&self) -> Option<(String, String, Option<String>)> {
        let connected: Vec<&accounts::Account> = self
            .accounts
            .iter()
            .filter(|account| account.available() && account.id != "jcode")
            .collect();
        let state = &self.account_sign_in;
        let mut ids: Vec<&str> = connected
            .iter()
            .map(|account| account.id.as_str())
            .collect();
        for index in state.selected_imports() {
            ids.extend(state.candidates[index].provider_ids());
        }
        let (provider, model) = jcode_base::auth::lifecycle::onboarding_default_selection(&ids)?;
        let method = connected
            .iter()
            .find(|account| account.id == provider)
            .map(|account| account.auth_kind.clone())
            .filter(|kind| !kind.trim().is_empty());
        Some((provider, model, method))
    }

    /// Keep the demo composer's model and login pills on the real identity
    /// instead of "Choose model" and "Accounts".
    fn sync_account_demo_identity(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self
            .account_sign_in
            .demo
            .as_ref()
            .map(|demo| demo.panel.clone())
        else {
            return;
        };
        let identity = self.account_onboarding_identity();
        let (provider, model, method) = match identity {
            Some((provider, model, method)) => (Some(provider), Some(model), method),
            None => (None, None, None),
        };
        panel.update(cx, |panel, cx| {
            if panel.provider == provider && panel.model == model && panel.auth_method == method {
                return;
            }
            panel.provider = provider;
            panel.auth_method = method;
            if panel.model != model {
                panel.model = model.clone();
                panel
                    .input
                    .update(cx, |input, cx| input.set_current_model(model, cx));
            }
            cx.notify();
        });
    }

    /// Right-half action: import the checked logins, then enter the workspace.
    fn continue_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.account_sign_in.selected_imports();
        let candidates = std::mem::take(&mut self.account_sign_in.candidates);
        if !selected.is_empty() && live() && rehearsal() {
            let count = selected.len();
            self.status = format!(
                "Rehearsal: would import {count} login{}",
                if count == 1 { "" } else { "s" }
            );
            self.finish_account_sign_in(window, cx);
            return;
        }
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

    /// Typing on the page means "let me start": continue into the workspace
    /// and carry the keystrokes into a real session's composer.
    fn continue_account_sign_in_typing(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.continue_account_sign_in(window, cx);
        let usable = self.slots.get(self.active).is_some_and(|slot| {
            let panel = slot.panel.read(cx);
            panel.preview_state.is_none() && !panel.is_default_directory() && !panel.is_machines()
        });
        if !usable {
            self.open_new_session(cx);
        }
        let Some(panel) = self.slots.get(self.active).map(|slot| slot.panel.clone()) else {
            return;
        };
        let input = panel.read(cx).input.clone();
        input.update(cx, |input, cx| {
            let content = format!("{}{text}", input.content);
            input.set_content(content, cx);
        });
        let focus = panel.read(cx).input_focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn activate_account_choice(
        &mut self,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match choice {
            Choice::Field => self.focus_account_input(window, cx),
            Choice::Primary => self.account_sign_in_primary(window, cx),
            Choice::OpenGmail => self.open_account_gmail(cx),
            Choice::StartOver => self.reset_account_sign_in(window, cx),
            Choice::Email(index) => self.pick_account_email(index, cx),
            Choice::Login(index) => self.toggle_account_import(index, cx),
            Choice::ThemeToggle => self.toggle_account_theme_picker(cx),
            Choice::Theme => self.pick_account_theme(Theme::active_preset().next(), cx),
            Choice::Finish => self.continue_account_sign_in(window, cx),
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
        self.restore_account_theme(cx);
        self.account_sign_in.task = None;
        self.account_sign_in.detect_task = None;
        self.account_sign_in.emails_task = None;
        self.account_sign_in.demo = None;
        self.account_sign_in.input = None;
        self.account_sign_in.stage = Stage::Welcome;
        self.account_sign_in.error = None;
        self.restore_focus(window, cx);
        cx.notify();
    }

    /// Back to the email field, keeping what was typed.
    fn reset_account_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let email = self
            .account_sign_in
            .stage
            .code_step()
            .map(|(email, _)| email.to_owned());
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

    fn new_account_input(
        &mut self,
        content: String,
        cx: &mut Context<Self>,
    ) -> Entity<PromptInput> {
        let code = self.account_sign_in.stage.code_step().is_some();
        let submit = cx.weak_entity();
        let change = cx.weak_entity();
        cx.new(|cx| {
            let mut input = PromptInput::new(
                cx,
                if code {
                    "6-digit code"
                } else {
                    "you@example.com"
                },
                move |text, _, window, app| {
                    // Deferred: the field is still mid-update when Enter fires.
                    let submit = submit.clone();
                    let handle = window.window_handle();
                    app.defer(move |app| {
                        let _ = handle.update(app, |_, window, app| {
                            let _ = submit.update(app, |this, cx| {
                                this.submit_account_field(text, window, cx)
                            });
                        });
                    });
                },
            )
            .without_command_completion()
            .without_chrome()
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
                        // The radio follows whatever address is in the field.
                        if matches!(this.account_sign_in.stage, Stage::Welcome) {
                            let state = &mut this.account_sign_in;
                            let picked = state
                                .emails
                                .iter()
                                .position(|e| e.email.eq_ignore_ascii_case(text.trim()));
                            if state.picked_email != picked {
                                state.picked_email = picked;
                                cx.notify();
                            }
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

    fn replace_account_input(
        &mut self,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                    self.account_sign_in.error =
                        Some("Enter the 6-digit code from the email.".into());
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

    fn start_account_sign_in(
        &mut self,
        email: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            let result =
                network(async { auth::start_email(&reqwest::Client::new(), &address).await });
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
        self.account_sign_in.stage = Stage::Verifying {
            email: email.clone(),
            login: Some(login.clone()),
        };
        let pending = login.clone();
        let request = cx.background_executor().spawn(async move {
            network(async { auth::verify_email(&reqwest::Client::new(), &pending, &code).await })
        });
        self.account_sign_in.task = Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                this.account_sign_in.task = None;
                let retry = |this: &mut Self, message: String, cx: &mut Context<Self>| {
                    this.account_sign_in.stage = Stage::Code {
                        email: email.clone(),
                        login: Some(login.clone()),
                    };
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
                        this.account_sign_in.input =
                            Some(this.new_account_input(email.clone(), cx));
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
        let Some((email, _)) = self.account_sign_in.stage.code_step() else {
            return;
        };
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
        if !self.account_sign_in.email && !connected {
            return div();
        }
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
                        if !this.account_menu.open {
                            this.toggle_account_menu(cx);
                        }
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
        self.sync_account_demo_identity(cx);
        let theme = Theme::global();
        let narrow = window.viewport_size().width < px(760.0);
        let padding = if narrow { 24.0 } else { LEFT_COLUMN_PADDING };
        let finish =
            div()
                .debug_selector(|| "account-sign-in-footer".into())
                .flex_none()
                .w_full()
                .px(px(padding))
                .pt_3()
                .pb(px(if narrow { 16.0 } else { FOOTER_BOTTOM }))
                .flex()
                .justify_center()
                .child(
                    div().w_full().max_w(px(LEFT_CONTENT_WIDTH)).flex().child(
                        account_button(
                            "account-sign-in-finish",
                            "Finish onboarding",
                            true,
                            self.account_sign_in.focused(Choice::Finish),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.continue_account_sign_in(window, cx)
                        })),
                    ),
                );
        let scroll = div()
            .id("account-sign-in-card")
            .debug_selector(|| "account-sign-in-card".into())
            .flex_1()
            .w_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px(px(padding))
            .pt(px(if narrow { 28.0 } else { 56.0 }))
            .pb_4()
            .child(
                div()
                    .w_full()
                    .max_w(px(LEFT_CONTENT_WIDTH))
                    .flex()
                    .flex_col()
                    .gap(px(32.0))
                    .child(self.account_onboarding_header())
                    .child(self.account_onboarding_email(cx))
                    .child(self.account_onboarding_logins(cx))
                    .child(self.account_onboarding_theme(cx)),
            );
        // The left column keeps its natural width (never more than half),
        // so the live transcript gets whatever space remains. Finish stays
        // pinned to its bottom while the choices above scroll.
        let info = div()
            .debug_selector(|| "account-sign-in-left".into())
            .when(narrow, |el| el.flex_1())
            .when(!narrow, |el| {
                el.flex_none()
                    .h_full()
                    .w(px(LEFT_COLUMN_WIDTH))
                    .max_w(gpui::relative(0.5))
            })
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(scroll)
            .child(finish);
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
                            .unwrap_or(Choice::Finish);
                        this.activate_account_choice(choice, window, cx);
                    }
                    _ if !typing
                        && (this.account_sign_in.field_open() || !this.account_sign_in.email)
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.alt
                        && event
                            .keystroke
                            .key_char
                            .as_deref()
                            .is_some_and(|text| text.chars().all(|c| !c.is_control())) =>
                    {
                        // Typing anywhere goes to the email field on the left,
                        // keeping the keystroke.
                        let text = event.keystroke.key_char.clone().unwrap_or_default();
                        if !this.account_sign_in.email {
                            this.continue_account_sign_in_typing(text, window, cx);
                            cx.stop_propagation();
                            return;
                        }
                        let input = this.ensure_account_input(cx);
                        input.update(cx, |input, cx| {
                            let content = format!("{}{text}", input.content);
                            input.set_content(content, cx);
                        });
                        this.focus_account_input(window, cx);
                        this.account_sign_in.keyboard_choice = None;
                        cx.notify();
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

    /// The Jcode account: addresses found in other coding agents' logins as
    /// radio pills, then the email field. The same spot holds the code step
    /// and the signed-in confirmation.
    fn account_onboarding_email(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        if !state.email && !state.connected {
            return div();
        }
        let theme = Theme::global();
        let signed_in = match &state.stage {
            Stage::Complete { email } => Some(format!("Signed in as {email}")),
            _ if state.connected => Some("Signed in to Jcode".to_string()),
            _ => None,
        };
        let mut section = section("Jcode account").debug_selector(|| "account-sign-in-email".into());
        if let Some(status) = signed_in {
            return section.child(
                div()
                    .debug_selector(|| "account-sign-in-status".into())
                    .px_4()
                    .py(px(10.0))
                    .rounded_full()
                    .bg(theme.OK.opacity(0.12))
                    .text_size(px(13.0))
                    .text_color(theme.OK)
                    .truncate()
                    .child(status),
            );
        }
        let code = state.stage.code_step().map(|(email, _)| email.to_owned());
        section = section.child(match &code {
            Some(_) => subheading("Enter the 6-digit code from the email."),
            None if state.emails.is_empty() => {
                subheading("Optional. Sign in to sync sessions and settings.")
            }
            None => subheading("Found in your coding agent logins. Pick one, or type another."),
        });
        if code.is_none() {
            section = section.children(self.account_onboarding_detected_emails(cx));
        }
        section = section.child(self.account_onboarding_field(cx));
        let state = &self.account_sign_in;
        if let Some(email) = code {
            section = section
                .child(
                    div()
                        .debug_selector(|| "account-sign-in-sent".into())
                        .text_size(px(12.0))
                        .text_color(theme.TEXT_DIM)
                        .child(format!("We emailed a code to {email}")),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
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
                );
        }
        if let Some(error) = state.error.clone() {
            section = section.child(
                div()
                    .debug_selector(|| "account-sign-in-error".into())
                    .px_2()
                    .text_size(px(12.0))
                    .text_color(theme.ERROR)
                    .child(error),
            );
        }
        section
    }

    /// Detected addresses as radio pills. Picking one fills the field below.
    fn account_onboarding_detected_emails(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        if state.email_choices().is_empty() {
            return None;
        }
        let mut list = div()
            .debug_selector(|| "account-emails".into())
            .flex()
            .flex_col()
            .gap_1();
        for index in state.email_choices() {
            let detected = &state.emails[index];
            let picked = state.picked_email == Some(index);
            let focused = state.focused(Choice::Email(index));
            let radio = div()
                .size(px(16.0))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(if picked { theme.ACCENT } else { theme.TEXT_DIM })
                .flex()
                .items_center()
                .justify_center()
                .when(picked, |el| {
                    el.child(div().size(px(8.0)).rounded_full().bg(theme.ACCENT))
                });
            list = list.child(
                div()
                    .id(("account-email", index))
                    .debug_selector(move || format!("account-email-{index}"))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py(px(8.0))
                    .rounded_full()
                    .border_1()
                    .border_color(if focused {
                        theme.ACCENT
                    } else {
                        gpui::transparent_black().into()
                    })
                    .bg(if picked {
                        theme.ACCENT.opacity(0.1)
                    } else {
                        theme.TEXT.opacity(0.04)
                    })
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.TEXT.opacity(0.08)))
                    .child(radio)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .truncate()
                                    .child(detected.email.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.TEXT_DIM)
                                    .truncate()
                                    .child(format!("from {}", detected.sources.join(", "))),
                            ),
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.pick_account_email(index, cx)),
                    ),
            );
        }
        Some(list)
    }

    /// The email (then code) field with its send/verify button, as one pill.
    fn account_onboarding_field(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let input = self.ensure_account_input(cx);
        let theme = Theme::global();
        let state = &self.account_sign_in;
        let busy = matches!(state.stage, Stage::Sending | Stage::Verifying { .. });
        div()
            .debug_selector(|| "account-sign-in-panel".into())
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(6.0))
            .pr(px(6.0))
            .h(px(FIELD_HEIGHT))
            .rounded_full()
            .border_1()
            .border_color(if state.focused(Choice::Field) {
                theme.ACCENT
            } else {
                theme.PANEL_BORDER
            })
            .bg(theme.PANEL_BG)
            .child(
                div()
                    .id("account-sign-in-field")
                    .debug_selector(|| "account-sign-in-field".into())
                    .flex_1()
                    .min_w_0()
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
                .text_size(px(13.0))
                .px_3()
                .py(px(6.0))
                .when(busy, |el| el.opacity(0.6))
                .on_click(
                    cx.listener(|this, _, window, cx| this.account_sign_in_primary(window, cx)),
                ),
            )
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

    /// A compact pill that unfolds the swatches. Most people keep the default.
    fn account_onboarding_theme(&self, cx: &mut Context<Self>) -> gpui::Div {
        let state = &self.account_sign_in;
        let theme = Theme::global();
        let active = Theme::active_preset();
        let expanded = state.theme_expanded;
        let label = if expanded {
            "Hide themes".to_string()
        } else {
            format!("Theme: {}", active.label())
        };
        let toggle = div()
            .id("account-theme-toggle")
            .debug_selector(|| "account-theme-toggle".into())
            .flex_none()
            .px_3()
            .py(px(5.0))
            .rounded_full()
            .border_1()
            .border_color(if state.focused(Choice::ThemeToggle) {
                theme.ACCENT
            } else {
                gpui::transparent_black().into()
            })
            .bg(theme.TEXT.opacity(0.06))
            .hover(|el| el.bg(theme.TEXT.opacity(0.1)))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(theme.TEXT_DIM)
            .child(label)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_account_theme_picker(cx)));
        let mut column = div().flex().flex_col().items_start().gap_3().child(toggle);
        if !expanded {
            return column;
        }
        let mut grid = div()
            .id("account-theme-grid")
            .debug_selector(|| "account-theme-grid".into())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.restore_account_theme(cx);
                }
            }))
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
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.pick_account_theme(preset, cx)),
                    )
                    .child(theme_swatch(preset, preset == active))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(if preset == active {
                                theme.TEXT
                            } else {
                                theme.TEXT_DIM
                            })
                            .child(preset.label()),
                    ),
            );
        }
        column = column.child(grid);
        column
    }

    /// The right half: the live chat replay, fully visible. Every control,
    /// including the Jcode email, lives in the left column.
    fn account_onboarding_continue(
        &mut self,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = Theme::global();
        let demo = self
            .account_sign_in
            .demo
            .as_ref()
            .map(|demo| demo.panel.clone());
        let composer = demo
            .as_ref()
            .and_then(|panel| panel.read(cx).input.read(cx).voice_bounds());
        div()
            .id("account-sign-in-right")
            .debug_selector(|| "account-sign-in-right".into())
            .when(narrow, |el| el.h(px(340.0)).flex_none().w_full())
            .when(!narrow, |el| el.flex_1().h_full())
            .min_w(px(0.0))
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.PANEL_BG)
            .child(
                div()
                    .id("account-sign-in-demo")
                    .debug_selector(|| "account-sign-in-demo".into())
                    .flex_1()
                    .min_h(px(0.0))
                    .p(px(if narrow { 4.0 } else { 8.0 }))
                    // The demo is read-only. Reaching for its composer means
                    // "let me type": the email field, or a real session.
                    .capture_any_mouse_down(cx.listener(
                        move |this, event: &gpui::MouseDownEvent, window, cx| {
                            if composer.is_some_and(|bounds| bounds.contains(&event.position)) {
                                if !this.account_sign_in.email {
                                    this.continue_account_sign_in_typing(String::new(), window, cx);
                                } else if this.account_sign_in.field_open() {
                                    this.focus_account_input(window, cx);
                                }
                            }
                            cx.stop_propagation();
                        },
                    ))
                    .children(demo),
            )
    }
}

/// The longest real sessions replayed in turn on the right half.
const SHOWCASE_TRANSCRIPTS: usize = 4;
/// Five theme swatches per row, plus the grid's focus outline.
const LEFT_CONTENT_WIDTH: f32 = 512.0;
const LEFT_COLUMN_PADDING: f32 = 48.0;
const LEFT_COLUMN_WIDTH: f32 = LEFT_CONTENT_WIDTH + 2.0 * LEFT_COLUMN_PADDING;
/// The email field pill, matching the chat composer's height.
const FIELD_HEIGHT: f32 = 46.0;
/// Space under the pinned Finish pill.
const FOOTER_BOTTOM: f32 = 28.0;

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
        .border_color(if focused {
            theme.ACCENT
        } else {
            gpui::transparent_black().into()
        })
        .bg(theme.TEXT.opacity(0.08))
        .child(
            div()
                .absolute()
                .top(px(2.0))
                .left(if importing {
                    px(2.0)
                } else {
                    width - knob - px(4.0)
                })
                .w(knob)
                .h(px(20.0))
                .rounded_full()
                .bg(if importing {
                    theme.ACCENT
                } else {
                    theme.TEXT_DIM
                }),
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
    let line =
        |width: f32, color: gpui::Rgba| div().h(px(3.0)).w(px(width)).rounded_full().bg(color);
    div()
        .w(px(92.0))
        .h(px(58.0))
        .p(px(6.0))
        .flex()
        .gap(px(4.0))
        .rounded_xl()
        .bg(t.BG)
        .border_2()
        .border_color(if active {
            current.ACCENT
        } else {
            current.PANEL_BORDER
        })
        .child(div().w(px(14.0)).h_full().rounded_md().bg(t.HEADER_BG))
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
        .bg(if primary {
            theme.ACCENT
        } else {
            theme.TEXT.opacity(0.06)
        })
        .text_size(px(14.0))
        .text_color(if primary { theme.BG } else { theme.TEXT })
        .text_center()
        .cursor_pointer()
        .hover(move |el| {
            if primary {
                el.opacity(0.9)
            } else {
                el.bg(theme.ACCENT.opacity(0.08))
            }
        })
        .child(label)
}

#[cfg(test)]
#[path = "workspace_account_sign_in_tests.rs"]
mod tests;
