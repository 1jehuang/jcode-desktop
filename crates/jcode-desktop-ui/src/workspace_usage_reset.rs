//! Redeem banked usage resets from the accounts list.
//!
//! OpenAI banked resets and Claude session-limit resets are single-use and
//! cannot be undone, so the flow is always: read-only preparation against the
//! provider, an explicit review, then one confirmed redemption. Preparation
//! pins the login, so switching accounts meanwhile cannot spend another one.
use super::*;
use crate::accounts::{BankedReset, ResetProvider};
use jcode_base::usage::{PendingAnthropicLimitReset, PendingOpenAiUsageReset};

/// A prepared redemption. Holds credentials, so it never leaves this module.
#[derive(Clone)]
pub(super) enum Pending {
    OpenAi(PendingOpenAiUsageReset),
    Claude(PendingAnthropicLimitReset),
}

impl Pending {
    fn account_display(&self) -> &str {
        match self {
            Self::OpenAi(pending) => pending.account_display(),
            Self::Claude(pending) => pending.account_display(),
        }
    }

    fn details(&self) -> Vec<String> {
        match self {
            Self::OpenAi(pending) => pending.confirmation_details(),
            Self::Claude(pending) => pending.confirmation_details(),
        }
    }
}

#[derive(Clone, Default)]
pub(super) enum Stage {
    #[default]
    Checking,
    Review(Pending),
    Redeeming(Pending),
    /// Final message. `ok` colours it, and never implies money was spent.
    Done {
        message: String,
        ok: bool,
    },
    /// The redemption outcome is unknown. The same pinned request may be
    /// retried safely, because the provider will not spend twice.
    Uncertain {
        message: String,
        pending: Pending,
    },
}

pub(super) struct Dialog {
    pub(super) provider: ResetProvider,
    pub(super) account_label: Option<String>,
    pub(super) stage: Stage,
    _task: Option<gpui::Task<()>>,
}

enum PrepareResult {
    Ready(Pending),
    Nothing(String),
}

// Reqwest needs Tokio, GPUI has its own executor. Run each bounded provider
// call on a private runtime off the UI thread.
fn network<T>(operation: impl std::future::Future<Output = T>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start the reset check. Please try again.".to_string())
        .map(|runtime| runtime.block_on(operation))
}

fn prepare(provider: ResetProvider, label: Option<String>) -> Result<PrepareResult, String> {
    network(async move {
        match provider {
            ResetProvider::OpenAi => {
                match jcode_base::usage::prepare_openai_usage_reset_for_account(label).await {
                    Ok(Some(pending)) => Ok(PrepareResult::Ready(Pending::OpenAi(pending))),
                    Ok(None) => Ok(PrepareResult::Nothing(
                        "No banked OpenAI resets are available. Nothing was changed.".into(),
                    )),
                    Err(error) => Err(first_line(&error.to_string())),
                }
            }
            ResetProvider::Claude => {
                match jcode_base::usage::prepare_anthropic_limit_reset(label).await {
                    Ok(Ok(pending)) => Ok(PrepareResult::Ready(Pending::Claude(pending))),
                    Ok(Err(unavailable)) => Ok(PrepareResult::Nothing(unavailable.message())),
                    Err(error) => Err(first_line(&error.to_string())),
                }
            }
        }
    })?
}

/// Provider errors carry a retry hint on later lines written for the TUI.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_string()
}

enum RedeemResult {
    Done { message: String, ok: bool },
    Uncertain(String),
}

fn redeem(pending: &Pending) -> RedeemResult {
    let pending = pending.clone();
    let outcome = network(async move {
        match &pending {
            Pending::OpenAi(pending) => jcode_base::usage::consume_openai_usage_reset(pending)
                .await
                .map(|outcome| {
                    (
                        outcome
                            .message()
                            .replace("Use /usage", "Check the accounts list"),
                        true,
                    )
                }),
            Pending::Claude(pending) => jcode_base::usage::consume_anthropic_limit_reset(pending)
                .await
                .map(|outcome| (outcome.message(), outcome.limits_cleared())),
        }
    });
    match outcome {
        Ok(Ok((message, ok))) => RedeemResult::Done { message, ok },
        Ok(Err(error)) => RedeemResult::Uncertain(first_line(&error.to_string())),
        Err(message) => RedeemResult::Uncertain(message),
    }
}

/// The daemon keeps its own quota caches and cooldowns. Tell it the login was
/// reset so the next prompt is not refused on stale state. Best effort.
fn notify_daemon(provider: ResetProvider, label: Option<&str>) {
    let Ok(client) = jcode_sdk::JcodeClient::connect(jcode_sdk::ConnectOptions {
        client_name: "jcode-desktop-usage-reset".into(),
        ensure_runtime: false,
        ..Default::default()
    }) else {
        return;
    };
    let _ = client.invalidate_usage(provider.api_id(), label);
}

fn live() -> bool {
    !cfg!(test) && !harness::screenshot_mode()
}

impl Workspace {
    /// Open the review for one login's reset and start the read-only check.
    pub(super) fn open_usage_reset(&mut self, reset: &BankedReset, cx: &mut Context<Self>) {
        let provider = reset.provider;
        let label = reset.account_label.clone();
        let task = live().then(|| {
            let check = {
                let label = label.clone();
                cx.background_executor()
                    .spawn(async move { prepare(provider, label) })
            };
            cx.spawn(async move |this, cx| {
                let result = check.await;
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.usage_reset.as_mut() else {
                        return;
                    };
                    if !matches!(dialog.stage, Stage::Checking) {
                        return;
                    }
                    dialog.stage = match result {
                        Ok(PrepareResult::Ready(pending)) => Stage::Review(pending),
                        Ok(PrepareResult::Nothing(message)) => Stage::Done { message, ok: false },
                        Err(message) => Stage::Done { message, ok: false },
                    };
                    cx.notify();
                });
            })
        });
        self.usage_reset = Some(Dialog {
            provider,
            account_label: label,
            stage: Stage::Checking,
            _task: task,
        });
        cx.notify();
    }

    pub(super) fn close_usage_reset(&mut self, cx: &mut Context<Self>) {
        // A redemption already sent cannot be recalled. Keep the dialog so the
        // user sees its result instead of guessing.
        if matches!(
            self.usage_reset.as_ref().map(|dialog| &dialog.stage),
            Some(Stage::Redeeming(_))
        ) {
            return;
        }
        self.usage_reset = None;
        cx.notify();
    }

    fn confirm_usage_reset(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.usage_reset.as_mut() else {
            return;
        };
        let pending = match &dialog.stage {
            Stage::Review(pending) | Stage::Uncertain { pending, .. } => pending.clone(),
            _ => return,
        };
        let provider = dialog.provider;
        let label = dialog.account_label.clone();
        dialog.stage = Stage::Redeeming(pending.clone());
        if live() {
            let work = cx.background_executor().spawn(async move {
                let result = redeem(&pending);
                // Invalidate even when uncertain: the reset may have applied.
                notify_daemon(provider, label.as_deref());
                (result, pending)
            });
            dialog._task = Some(cx.spawn(async move |this, cx| {
                let (result, pending) = work.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(dialog) = this.usage_reset.as_mut() {
                        dialog.stage = match result {
                            RedeemResult::Done { message, ok } => Stage::Done { message, ok },
                            RedeemResult::Uncertain(message) => {
                                Stage::Uncertain { message, pending }
                            }
                        };
                    }
                    accounts::request_refresh();
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    pub(super) fn render_usage_reset(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let Some(dialog) = self.usage_reset.as_ref() else {
            return div().into_any_element();
        };
        let title = match dialog.provider {
            ResetProvider::OpenAi => "Reset OpenAI usage limits",
            ResetProvider::Claude => "Reset Claude session limit",
        };
        let login = match &dialog.stage {
            Stage::Review(pending)
            | Stage::Redeeming(pending)
            | Stage::Uncertain { pending, .. } => pending.account_display().to_string(),
            _ => dialog
                .account_label
                .clone()
                .unwrap_or_else(|| format!("default {} login", dialog.provider.label())),
        };

        let line = |text: String| {
            div()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(theme.TEXT_DIM)
                .child(text)
        };
        let mut body = div().flex().flex_col().gap_1();
        let mut primary: Option<(&'static str, bool)> = None;
        let mut cancel_label = "Cancel";
        match &dialog.stage {
            Stage::Checking => {
                body = body.child(line(
                    "Checking which resets this login can use. Nothing is spent yet.".into(),
                ));
            }
            Stage::Review(pending) => {
                for detail in pending.details() {
                    body = body.child(line(detail));
                }
                body = body.child(
                    div()
                        .debug_selector(|| "usage-reset-warning".into())
                        .pt_1()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(theme.WARN)
                        .child("This spends one reset and cannot be undone. It does not buy credits or raise your plan limits."),
                );
                primary = Some(("Reset now", true));
            }
            Stage::Redeeming(_) => {
                body = body.child(line("Requesting the reset…".into()));
                primary = Some(("Resetting…", false));
            }
            Stage::Done { message, ok } => {
                body = body.child(
                    div()
                        .debug_selector(|| "usage-reset-result".into())
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(if *ok { theme.OK } else { theme.TEXT_DIM })
                        .child(message.clone()),
                );
                cancel_label = "Close";
            }
            Stage::Uncertain { message, .. } => {
                body = body
                    .child(
                        div()
                            .debug_selector(|| "usage-reset-result".into())
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(theme.WARN)
                            .child(message.clone()),
                    )
                    .child(line(
                        "The reset may already have applied. Retrying sends the same request, so it cannot spend a second reset.".into(),
                    ));
                primary = Some(("Retry same reset", true));
                cancel_label = "Close";
            }
        }
        let redeeming = matches!(dialog.stage, Stage::Redeeming(_));

        let mut actions = div().pt_3().flex().justify_end().gap_2();
        if !redeeming {
            actions = actions.child(
                div()
                    .id("usage-reset-cancel")
                    .debug_selector(|| "usage-reset-cancel".into())
                    .px_4()
                    .py_1()
                    .rounded_full()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .bg(theme.HEADER_BG)
                    .text_color(theme.TEXT)
                    .hover(|el| el.bg(theme.INLINE_CODE_BG))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_usage_reset(cx);
                    }))
                    .child(cancel_label),
            );
        }
        if let Some((label, enabled)) = primary {
            actions = actions.child(
                div()
                    .id("usage-reset-confirm")
                    .debug_selector(|| "usage-reset-confirm".into())
                    .px_4()
                    .py_1()
                    .rounded_full()
                    .text_size(px(12.0))
                    .bg(if enabled {
                        theme.ACCENT
                    } else {
                        theme.ACCENT.opacity(0.4)
                    })
                    .text_color(theme.BG)
                    .when(enabled, |el| {
                        el.cursor_pointer()
                            .hover(|el| el.opacity(0.9))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.confirm_usage_reset(cx);
                            }))
                    })
                    .child(label),
            );
        }

        div()
            .id("usage-reset-overlay")
            .debug_selector(|| "usage-reset-overlay".into())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000099))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("usage-reset-dialog")
                    .debug_selector(|| "usage-reset-dialog".into())
                    .w(px(400.0))
                    .max_w(relative(0.9))
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme.PANEL_BORDER_FOCUS)
                    .bg(theme.PANEL_BG)
                    .shadow_md()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| "usage-reset-title".into())
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.TEXT)
                            .child(title),
                    )
                    .child(
                        div()
                            .debug_selector(|| "usage-reset-login".into())
                            .text_size(px(11.0))
                            .text_color(theme.TEXT_DIM)
                            .child(login),
                    )
                    .child(body)
                    .child(actions),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{Account, UsageReport};

    fn claude_account(reset: Option<BankedReset>) -> Account {
        Account {
            id: "claude".into(),
            display_name: "Claude".into(),
            status: "available".into(),
            auth_kind: "OAuth".into(),
            method: "OAuth".into(),
            limits: vec![accounts::UsageLimit {
                name: "5-hour window".into(),
                usage_percent: 100.0,
                reset_in: Some("2h".into()),
            }],
            usage_reports: vec![UsageReport {
                provider_name: "Anthropic (Claude)".into(),
                account_label: None,
                limits: Vec::new(),
                extra_info: Vec::new(),
                banked_reset: reset,
            }],
        }
    }

    fn session_reset(available: bool) -> BankedReset {
        BankedReset {
            provider: ResetProvider::Claude,
            account_label: None,
            available_count: u64::from(available),
            limit_reached: true,
            next_available_at: None,
        }
    }

    #[gpui::test]
    fn reset_pill_opens_a_review_that_cannot_spend_without_confirmation(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.sidebar_view = SidebarView::Accounts;
            w.set_test_accounts(vec![claude_account(Some(session_reset(true)))]);
            w
        });
        vcx.run_until_parked();
        let pill = vcx
            .debug_bounds("account-claude-reset-0")
            .expect("an offerable reset shows a pill");
        vcx.simulate_click(pill.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("usage-reset-dialog").is_some());
        workspace.read_with(vcx, |w, _| {
            let dialog = w.usage_reset.as_ref().unwrap();
            assert_eq!(dialog.provider, ResetProvider::Claude);
            // Tests never reach the network: the dialog waits in its read-only check.
            assert!(matches!(dialog.stage, Stage::Checking));
        });
        // No confirm control exists until a prepared review is shown.
        assert!(vcx.debug_bounds("usage-reset-confirm").is_none());
        let cancel = vcx.debug_bounds("usage-reset-cancel").unwrap();
        vcx.simulate_click(cancel.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("usage-reset-dialog").is_none());
        workspace.read_with(vcx, |w, _| assert!(w.usage_reset.is_none()));
    }

    #[gpui::test]
    fn spent_or_absent_resets_show_no_pill(cx: &mut gpui::TestAppContext) {
        for reset in [None, Some(session_reset(false))] {
            let (_, vcx) = cx.add_window_view(|_, cx| {
                let mut w = Workspace::for_test(learning::Coach::new(), cx);
                w.sidebar_view = SidebarView::Accounts;
                w.sidebar_view = SidebarView::Accounts;
                w.set_test_accounts(vec![claude_account(reset.clone())]);
                w
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("account-claude").is_some());
            assert!(vcx.debug_bounds("account-claude-reset-0").is_none());
        }
    }

    #[gpui::test]
    fn results_replace_the_confirm_action_and_can_be_closed(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.sidebar_view = SidebarView::Accounts;
            w.set_test_accounts(vec![claude_account(Some(session_reset(true)))]);
            w
        });
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            w.open_usage_reset(&session_reset(true), cx);
            w.usage_reset.as_mut().unwrap().stage = Stage::Done {
                message: "Claude session limit reset.".into(),
                ok: true,
            };
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("usage-reset-result").is_some());
        assert!(vcx.debug_bounds("usage-reset-confirm").is_none());
        workspace.update(vcx, |w, cx| w.close_usage_reset(cx));
        workspace.read_with(vcx, |w, _| assert!(w.usage_reset.is_none()));
    }

    #[test]
    fn openai_resets_are_offered_only_once_the_limit_binds() {
        let reset = |limit_reached| BankedReset {
            provider: ResetProvider::OpenAi,
            account_label: Some("work".into()),
            available_count: 2,
            limit_reached,
            next_available_at: None,
        };
        assert!(reset(true).offerable());
        assert!(!reset(false).offerable());
        assert_eq!(reset(true).pill_label(), "Reset limits · 2");
        assert!(session_reset(true).offerable());
        assert!(!session_reset(false).offerable());
        assert_eq!(first_line("first\nsecond"), "first");
    }
}
