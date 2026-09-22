//! Native recovery actions. Route changes never pass through the composer.
use super::*;
use gpui::AnyElement;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryKind {
    Auth,
    Model,
    Quota,
    Other,
}

fn recovery_kind(message: &str) -> RecoveryKind {
    let message = message.to_ascii_lowercase();
    if [
        "token refresh failed",
        "refresh token was previously rejected",
        "jcode login",
        "refresh_token_invalidated",
        "re-authenticate",
        "invalid_grant",
        "invalid x-api-key",
        "incorrect api key",
        "api key not valid",
        "no refresh token",
        "credentials have been revoked",
        "run /login",
        "unauthorized",
        "authentication",
        "not authenticated",
        "invalid api key",
        "invalid_api_key",
        "api key missing",
        "missing api key",
        "invalid credentials",
        "missing credentials",
        "token expired",
        "expired token",
        "login required",
        "log in",
        "sign in",
        "403 forbidden",
        "http 403",
        "status: 403",
        "status 403",
        "not logged in",
        "http 401",
        "status: 401",
        "status 401",
        "returned 401",
    ]
    .iter()
    .any(|term| message.contains(term))
    {
        RecoveryKind::Auth
    } else if [
        "quota",
        "rate limit",
        "rate_limit",
        "rate-limit",
        "http 429",
        "status: 429",
        "status 429",
        "returned 429",
        "too many requests",
        "insufficient credits",
        "insufficient balance",
        "billing",
        "usage limit",
    ]
    .iter()
    .any(|term| message.contains(term))
    {
        RecoveryKind::Quota
    } else if message.contains("model")
        && [
            "not currently usable",
            "choose another model",
            "not found",
            "not_found",
            "not_supported",
            "deprecated",
            "retired",
            "unavailable",
            "not available",
            "not supported",
            "unsupported",
            "invalid",
            "access",
            "permission",
            "unknown",
            "does not exist",
        ]
        .iter()
        .any(|term| message.contains(term))
    {
        RecoveryKind::Model
    } else {
        RecoveryKind::Other
    }
}

// Provider errors can embed arbitrary CLI commands, nested causes and setup
// instructions. Present a native summary rather than attempting a brittle
// command-by-command rewrite. Copy details retains the complete original.
fn native_error_message(message: &str) -> String {
    match recovery_kind(message) {
        RecoveryKind::Auth => "Account authentication failed. Log in again or choose another model.",
        RecoveryKind::Model => "This model is unavailable for the current account. Choose another model.",
        RecoveryKind::Quota => "The provider's usage or rate limit was reached. Choose another model or log in to a different account.",
        RecoveryKind::Other => message,
    }.to_string()
}

impl Panel {
    pub(crate) fn toggle_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recovery_picker_open {
            self.recovery_picker_open = false;
        } else if self.model_picker_open {
            self.close_model_picker(cx);
        } else {
            self.open_recovery_models(cx);
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    pub(super) fn open_recovery_models(&mut self, cx: &mut Context<Self>) {
        self.login = None;
        if self.available_models.is_empty() {
            self.recovery_picker_open = true;
        } else {
            self.open_model_picker(cx);
        }
        self.focus_pending = true;
        cx.notify();
    }

    pub(super) fn render_recovery_model_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "recovery-model-picker".into())
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .bg(Theme::global().HEADER_BG)
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child("Choose a model")
                    .child(
                        div()
                            .id("recovery-model-close")
                            .debug_selector(|| "recovery-model-close".into())
                            .cursor_pointer()
                            .child("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.recovery_picker_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.available_models.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("recovery-connect-account")
                                .debug_selector(|| "recovery-connect-account".into())
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|el| el.bg(Theme::global().ACCENT_DIM))
                                .child("Connect account")
                                .on_click(cx.listener(|this, _, _, cx| this.open_login_picker(cx))),
                        )
                        .child(
                            div()
                                .id("recovery-refresh-models")
                                .debug_selector(|| "recovery-refresh-models".into())
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|el| el.bg(Theme::global().ACCENT_DIM))
                                .child("Refresh choices")
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.bridge.send(Command::RefreshRuntime {
                                        session_id: this.session_id.clone(),
                                    });
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_recovery_error(
        &self,
        index: usize,
        message: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = recovery_kind(message);
        let details = message.to_string();
        div()
            .debug_selector(|| "recovery-error".into())
            .flex()
            .flex_col()
            .gap_2()
            .px_2p5()
            .py_1p5()
            .rounded_md()
            .bg(Theme::global().ERROR_BG)
            .border_1()
            .border_color(Theme::global().TOOL_BORDER)
            .text_size(px(12.))
            .text_color(Theme::global().ERROR)
            .child(text_selection::plain(
                self.transcript_selection.clone(),
                format!("{index}-error"),
                native_error_message(message),
                window,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .text_color(Theme::global().TEXT)
                    .child(
                        div()
                            .id(("recovery-copy", index))
                            .debug_selector(|| "recovery-copy".into())
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|el| el.bg(Theme::global().ACCENT_DIM))
                            .child("Copy details")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    details.clone(),
                                ));
                                cx.stop_propagation();
                            }),
                    )
                    .when(kind != RecoveryKind::Other, |el| {
                        el.child(
                            div()
                                .id(("recovery-choose-model", index))
                                .debug_selector(|| "recovery-choose-model".into())
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|el| el.bg(Theme::global().ACCENT_DIM))
                                .child("Choose model")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_recovery_models(cx);
                                    cx.stop_propagation();
                                })),
                        )
                    })
                    .when(
                        matches!(kind, RecoveryKind::Auth | RecoveryKind::Quota),
                        |el| {
                            el.child(
                                div()
                                    .id(("recovery-login", index))
                                    .debug_selector(|| "recovery-login".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|el| el.bg(Theme::global().ACCENT_DIM))
                                    .child("Log in")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_login_picker(cx);
                                        cx.stop_propagation();
                                    })),
                            )
                        },
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "panel_recovery_tests.rs"]
mod tests;
