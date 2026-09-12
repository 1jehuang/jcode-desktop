//! Offline fixtures rendered by the ordinary panel and ApiEvent reducers.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

impl Panel {
    pub fn new_preview(state: PreviewState, cx: &mut Context<Self>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = format!(
            "preview://{}/{}",
            state.id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut panel = Self::new(
            id,
            Some(state.title().into()),
            None,
            crate::harness::spawn_inert(),
            cx,
        );
        panel.preview_state = Some(state);
        panel.items.clear();
        panel.streaming_text.clear();
        panel.streaming_reasoning.clear();
        panel.seed_preview(state, cx);
        // All input routes, including directly constructed panels, use the guarded dispatcher.
        let entity = cx.entity();
        cx.defer(move |cx| Self::connect_input(&entity, cx));
        panel
    }

    pub fn reset_preview(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.preview_state else {
            return;
        };
        let id = self.session_id.clone();
        *self = Self::new_preview(state, cx);
        self.session_id = id;
        cx.notify();
    }

    fn seed_preview(&mut self, state: PreviewState, cx: &mut Context<Self>) {
        self.history_loaded = true;
        self.apply(
            &ApiEvent::RuntimeInfo {
                session_id: self.session_id.clone(),
                provider: Some("openai".into()),
                model: Some("preview-model".into()),
                reasoning_effort: None,
                routes: vec![jcode_sdk::ModelRouteInfo {
                    model: "preview-model".into(),
                    provider: "openai".into(),
                    api_method: "preview".into(),
                    available: true,
                    detail: "Offline preview".into(),
                }],
            },
            cx,
        );
        match state {
            PreviewState::Empty => {}
            PreviewState::Streaming => {
                self.apply(
                    &ApiEvent::SessionStatus {
                        session_id: self.session_id.clone(),
                        status: "processing".into(),
                    },
                    cx,
                );
                self.apply(&ApiEvent::TextDelta { session_id: self.session_id.clone(), text: "I’m reviewing the implementation.\n\nThis **streaming preview** uses the real transcript renderer.".into() }, cx);
            }
            PreviewState::LoginDialogError => {
                self.open_login_picker(cx);
                self.set_preview_login_error(cx);
            }
            state => {
                let message = match state {
                    PreviewState::LoginError => {
                        "401 Unauthorized: account authentication failed. Log in again."
                    }
                    PreviewState::ModelAccessError => {
                        "Model access denied: this model is not available for your account."
                    }
                    PreviewState::RateLimit => "429 rate limit exceeded. Please try again later.",
                    PreviewState::Disconnected => "Connection closed. The server is disconnected.",
                    _ => unreachable!(),
                };
                self.apply(
                    &ApiEvent::Error {
                        code: jcode_sdk::api::ErrorCode::Internal,
                        message: message.into(),
                    },
                    cx,
                );
                if state == PreviewState::Disconnected {
                    self.apply(
                        &ApiEvent::SessionStatus {
                            session_id: self.session_id.clone(),
                            status: "disconnected".into(),
                        },
                        cx,
                    );
                }
            }
        }
    }

    pub(super) fn handle_preview_command(&mut self, content: &str, cx: &mut Context<Self>) -> bool {
        // Only simulated login and local model-picker UI are permitted. Never forward
        // arbitrary commands to the normal dispatcher (terminal/files/integrations).
        if !self.login_command(content.trim(), cx) && content.trim() == "/model" {
            self.open_recovery_models(cx);
        }
        cx.notify();
        true
    }

    pub(super) fn render_preview_badge(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.preview_state?;
        let theme = Theme::global();
        Some(
            div()
                .id("preview-badge")
                .debug_selector(|| "preview-badge".into())
                .flex()
                .flex_none()
                .items_center()
                .justify_between()
                .px_2()
                .py_1()
                .bg(theme.ACCENT.opacity(0.12))
                .text_color(theme.ACCENT)
                .text_xs()
                .child(format!("Offline preview · {}", state.id()))
                .child(
                    div()
                        .id("preview-reset")
                        .debug_selector(|| "preview-reset".into())
                        .cursor_pointer()
                        .px_2()
                        .child("Reset")
                        .on_click(cx.listener(|panel, _, _, cx| panel.reset_preview(cx))),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "panel_preview_tests.rs"]
mod tests;
