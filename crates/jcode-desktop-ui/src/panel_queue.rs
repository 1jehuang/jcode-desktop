//! Prompts explicitly queued by Ctrl+Enter wait for the current turn to finish.
use super::*;

#[path = "panel_auto_poke.rs"]
mod auto_poke;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptQueue {
    // Never resume automatic work merely by restoring a window.
    #[serde(skip)]
    auto_poke: auto_poke::AutoPoke,
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
    pub(super) fn pause_queue_for_stop(&mut self) {
        self.prompt_queue.paused = true;
        self.prompt_queue.auto_poke = Default::default();
    }

    pub(crate) fn submit_or_queue(
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
        self.prompt_queue.paused = false;
        self.prompt_queue.auto_poke.start(self.items.len());
        self.send_prompt(content, images, cx);
    }

    fn send_prompt(
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
        self.prompt_queue.auto_poke.observe(event);
        match event {
            ApiEvent::Error { .. } | ApiEvent::TurnStopped { .. } => {
                self.prompt_queue.paused = true
            }
            ApiEvent::SessionStatus { status, .. }
                if matches!(status.as_str(), "cancelled" | "canceled") =>
            {
                self.prompt_queue.paused = true;
            }
            ApiEvent::TurnDone { .. } => {
                let panel = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |this, cx| {
                        this.send_queued_prompts(cx);
                        this.send_auto_poke(cx);
                    });
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

    /// Send one queued prompt immediately. During an active turn the harness
    /// delivers it as an urgent steer rather than waiting for the turn to end.
    pub(super) fn send_queued_prompt_now(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.prompt_queue.prompts.len() || self.is_pending_session() {
            return;
        }
        let prompt = self.prompt_queue.prompts.remove(index);
        self.send_prompt(prompt.content, prompt.images, cx);
        cx.notify();
    }

    /// Move one queued prompt back into the composer for editing.
    pub(super) fn recall_queued_prompt(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.prompt_queue.prompts.len() {
            return;
        }
        let prompt = self.prompt_queue.prompts.remove(index);
        self.input.update(cx, |input, cx| {
            input.recall_prompt(&prompt.content, prompt.images, cx)
        });
        let focus = self.input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
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
                .flex()
                .flex_col()
                .gap_2()
                .min_w_0()
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
                        .debug_selector(|| "prompt-queue-items".into())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .max_h(px(144.))
                        .overflow_y_scroll()
                        .children(self.prompt_queue.prompts.iter().enumerate().map(
                            |(index, prompt)| {
                                let label = if prompt.images.is_empty() {
                                    prompt.content.clone()
                                } else {
                                    format!("{} · {} image(s)", prompt.content, prompt.images.len())
                                };
                                div()
                                    .debug_selector(move || format!("queued-prompt-{index}").into())
                                    .flex()
                                    .flex_none()
                                    .min_w_0()
                                    .items_center()
                                    .gap_2()
                                    .py_1()
                                    .child(
                                        div()
                                            .flex_none()
                                            .min_w(px(20.))
                                            .text_color(theme.TEXT_DIM)
                                            .child(format!("{}.", index + 1)),
                                    )
                                    .child(div().flex_1().min_w_0().truncate().child(label))
                                    .child(queue_action(
                                        "send-queued-prompt",
                                        index,
                                        include_bytes!("../../../assets/icons/arrow-up.svg"),
                                        cx.listener(move |this, _, _, cx| {
                                            this.send_queued_prompt_now(index, cx)
                                        }),
                                    ))
                                    .child(queue_action(
                                        "recall-queued-prompt",
                                        index,
                                        include_bytes!("../../../assets/icons/arrow-down.svg"),
                                        cx.listener(move |this, _, window, cx| {
                                            this.recall_queued_prompt(index, window, cx)
                                        }),
                                    ))
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

fn queue_action(
    id: &'static str,
    index: usize,
    icon: &'static [u8],
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let theme = Theme::global();
    div()
        .id((id, index))
        .debug_selector(move || format!("{id}-{index}").into())
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded_sm()
        .cursor_pointer()
        .hover(|el| el.bg(theme.QUOTE_BG))
        .child(
            gpui::svg()
                .data(icon)
                .size(px(13.))
                .text_color(theme.TEXT_DIM),
        )
        .on_click(on_click)
}

#[cfg(test)]
#[path = "panel_queue_tests.rs"]
mod tests;
