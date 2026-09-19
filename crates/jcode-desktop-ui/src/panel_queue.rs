//! Prompts explicitly queued by Ctrl+Enter wait for the current turn to finish.
use super::*;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptQueue {
    prompts: Vec<QueuedPrompt>,
    #[serde(default)]
    pub(super) paused: bool,
    #[serde(default)]
    pub(super) waiting_for_connection: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct QueuedPrompt {
    content: String,
    images: Vec<(String, String)>,
}

impl Panel {
    pub(super) fn submit_or_queue(
        &mut self,
        content: String,
        images: Vec<(String, String)>,
        queued: bool,
        cx: &mut Context<Self>,
    ) {
        if self.is_pending_session()
            || self.prompt_queue.waiting_for_connection
            || (queued && (self.activity_active() || !self.pending_users.is_empty()))
        {
            self.prompt_queue.waiting_for_connection |= self.is_pending_session();
            self.prompt_queue
                .prompts
                .push(QueuedPrompt { content, images });
            self.prompt_queue.paused = false;
            self.send_queued_prompts(cx);
        } else {
            self.send_composer_prompt(content, images, cx);
        }
        cx.notify();
    }

    fn send_composer_prompt(
        &mut self,
        content: String,
        images: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        self.bridge.send(Command::Send {
            session_id: self.session_id.clone(),
            content: content.clone(),
            images: images.clone(),
        });
        if !self.items.iter().any(|item| matches!(item, Item::User(_)))
            && custom_session_title(&self.session_id, self.title.as_ref()).is_none()
            && let Some(title) = first_prompt_title(&content)
        {
            self.title = title.into();
        }
        if let Some(layout) = &mut self.startup_layout {
            layout.committed = true;
        }
        let index = self.items.len();
        self.items.push(Item::User(content));
        self.items.extend(
            images.into_iter().map(|(media_type, data)| {
                Item::Image(TranscriptImage::new(media_type, data, None))
            }),
        );
        self.pending_users.push_back(index);
        crate::sounds::play(crate::sounds::Cue::Sent, cx);
        self.stick_to_bottom = true;
        self.transcript_list.scroll_to_end();
    }

    pub(super) fn observe_prompt_queue(&mut self, event: &ApiEvent, cx: &mut Context<Self>) {
        match event {
            ApiEvent::Error { .. } => self.prompt_queue.paused = true,
            ApiEvent::SessionStatus { status, .. }
                if matches!(status.as_str(), "cancelled" | "canceled") =>
            {
                self.prompt_queue.paused = true;
            }
            ApiEvent::TurnDone { .. } => {
                let panel = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |this, cx| this.send_queued_prompts(cx));
                });
            }
            ApiEvent::SessionStatus { status, .. } if status == "idle" => {
                let panel = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |this, cx| this.send_queued_prompts(cx));
                });
            }
            _ => {}
        }
    }

    pub(super) fn send_queued_prompts(&mut self, cx: &mut Context<Self>) {
        if self.prompt_queue.prompts.is_empty() {
            if !self.is_pending_session() && self.history_loaded {
                self.prompt_queue.waiting_for_connection = false;
            }
            return;
        }
        if self.prompt_queue.paused
            || !(self.status == "idle"
                || (self.prompt_queue.waiting_for_connection && self.status == "connected"))
            || self.activity_active()
            || !self.pending_users.is_empty()
            || self.is_pending_session()
            || !self.history_loaded
        {
            return;
        }
        // Match the TUI: combine waiting prompts into one follow-up turn, in
        // order, retaining every attachment. Draining atomically also makes a
        // TurnDone followed by idle incapable of sending the same queue twice.
        let prompts = std::mem::take(&mut self.prompt_queue.prompts);
        self.prompt_queue.waiting_for_connection = false;
        let content = prompts
            .iter()
            .map(|p| p.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let images = prompts.into_iter().flat_map(|p| p.images).collect();
        self.send_composer_prompt(content, images, cx);
        cx.notify();
    }

    pub(super) fn render_prompt_queue(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.prompt_queue.prompts.is_empty() {
            return None;
        }
        let theme = Theme::global();
        Some(
            div()
                .id("prompt-queue")
                .debug_selector(|| "prompt-queue".into())
                .flex_none()
                .mx_2()
                .p_2()
                .rounded_md()
                .bg(theme.TOOL_BG)
                .text_size(px(12.))
                .child(
                    div()
                        .text_color(theme.TEXT_DIM)
                        .child(if self.prompt_queue.paused {
                            "Queue paused · waiting prompts are preserved"
                        } else if self.is_pending_session()
                            || self.prompt_queue.waiting_for_connection
                        {
                            "Queued · sends when connected"
                        } else {
                            "Queued · sends after this response"
                        }),
                )
                .child(
                    div()
                        .id("prompt-queue-items")
                        .max_h(px(100.))
                        .overflow_y_scroll()
                        .children(self.prompt_queue.prompts.iter().enumerate().map(
                            |(index, prompt)| {
                                let label = if prompt.images.is_empty() {
                                    prompt.content.clone()
                                } else {
                                    format!("{} · {} image(s)", prompt.content, prompt.images.len())
                                };
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().truncate().child(label))
                                    .child(
                                        div()
                                            .id(("remove-queued-prompt", index))
                                            .cursor_pointer()
                                            .px_2()
                                            .text_color(theme.TEXT_DIM)
                                            .child("Remove")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if index < this.prompt_queue.prompts.len() {
                                                    this.prompt_queue.prompts.remove(index);
                                                }
                                                cx.notify();
                                            })),
                                    )
                            },
                        )),
                )
                .when(self.prompt_queue.paused, |el| {
                    el.child(
                        div()
                            .id("resume-prompt-queue")
                            .cursor_pointer()
                            .child("Resume queue")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prompt_queue.paused = false;
                                this.send_queued_prompts(cx);
                                cx.notify();
                            })),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "panel_queue_tests.rs"]
mod tests;
