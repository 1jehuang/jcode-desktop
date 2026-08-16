//! Panel: one Jcode session as a spatial card with a live transcript.

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString, Window, div,
    prelude::*, px, relative,
};
use jcode_sdk::ApiEvent;

use crate::harness::{Bridge, Command};
use crate::input::PromptInput;
use crate::markdown;
use crate::theme::Theme;

/// One transcript entry, in display order.
#[derive(Debug, Clone)]
pub enum Item {
    User(String),
    Assistant(String),
    Reasoning(String),
    Tool {
        call_id: String,
        name: String,
        done: bool,
        error: Option<String>,
    },
    Error(String),
}

pub struct Panel {
    pub session_id: String,
    pub title: SharedString,
    pub working_dir: Option<String>,
    pub status: String,
    pub connection_phase: String,
    pub items: Vec<Item>,
    /// Streaming assistant text accumulates here until the turn ends.
    streaming_text: String,
    streaming_reasoning: String,
    pub input: Entity<PromptInput>,
    pub focus_handle: FocusHandle,
    scroll: ScrollHandle,
    stick_to_bottom: bool,
    bridge: Bridge,
    history_loaded: bool,
}

impl Panel {
    pub fn new(
        session_id: String,
        title: Option<String>,
        working_dir: Option<String>,
        bridge: Bridge,
        cx: &mut Context<Self>,
    ) -> Self {
        let send_bridge = bridge.clone();
        let send_session = session_id.clone();
        let input = cx.new(|cx| {
            PromptInput::new(cx, "message jcode...", move |content, _window, _app| {
                send_bridge.send(Command::Send {
                    session_id: send_session.clone(),
                    content,
                });
            })
        });
        // Local echo is appended by the workspace when submit fires; simplest
        // is to observe our own input entity... but the closure above has no
        // panel access. Instead the workspace routes sends through the panel.
        let display_title = title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| short_id(&session_id));
        Self {
            session_id,
            title: display_title.into(),
            working_dir,
            status: "idle".into(),
            connection_phase: String::new(),
            items: Vec::new(),
            streaming_text: String::new(),
            streaming_reasoning: String::new(),
            input,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            stick_to_bottom: true,
            bridge,
            history_loaded: false,
        }
    }

    /// Wire the input's submit to also echo locally. Called once after
    /// creation, when we have the panel entity.
    pub fn connect_input(panel: &Entity<Panel>, cx: &mut App) {
        let weak = panel.downgrade();
        panel.update(cx, |this, cx| {
            let bridge = this.bridge.clone();
            let session_id = this.session_id.clone();
            this.input = cx.new(|cx| {
                PromptInput::new(cx, "message jcode...", move |content, _window, app| {
                    bridge.send(Command::Send {
                        session_id: session_id.clone(),
                        content: content.clone(),
                    });
                    if let Some(panel) = weak.upgrade() {
                        panel.update(app, |this, cx| {
                            this.items.push(Item::User(content));
                            this.stick_to_bottom = true;
                            this.scroll.scroll_to_bottom();
                            cx.notify();
                        });
                    }
                })
            });
        });
    }

    pub fn load_history(
        &mut self,
        messages: Vec<jcode_sdk::HistoryMessage>,
        cx: &mut Context<Self>,
    ) {
        if self.history_loaded {
            return;
        }
        self.history_loaded = true;
        let mut items = Vec::with_capacity(messages.len());
        for message in messages {
            match message.role.as_str() {
                "user" => items.push(Item::User(message.content)),
                "assistant" => {
                    if !message.content.trim().is_empty() {
                        items.push(Item::Assistant(message.content));
                    }
                }
                _ => {}
            }
        }
        // History replaces any placeholder items but keeps live streaming.
        items.extend(
            self.items
                .drain(..)
                .filter(|item| matches!(item, Item::User(_)))
                .take(0),
        );
        let mut existing = std::mem::take(&mut self.items);
        // Keep locally echoed items that arrived before history loaded.
        items.append(&mut existing);
        self.items = items;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Apply a streaming event addressed to this session.
    pub fn apply(&mut self, event: &ApiEvent, cx: &mut Context<Self>) {
        match event {
            ApiEvent::TextDelta { text, .. } => {
                self.flush_reasoning();
                self.streaming_text.push_str(text);
            }
            ApiEvent::ReasoningDelta { text, .. } => {
                self.streaming_reasoning.push_str(text);
            }
            ApiEvent::ReasoningDone { .. } => {
                self.flush_reasoning();
            }
            ApiEvent::ToolStart { call_id, name, .. } => {
                self.flush_streaming();
                self.items.push(Item::Tool {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    done: false,
                    error: None,
                });
            }
            ApiEvent::ToolDone {
                call_id,
                name,
                error,
                ..
            } => {
                let found = self.items.iter_mut().rev().find(|item| {
                    matches!(item, Item::Tool { call_id: existing, .. } if existing == call_id)
                });
                if let Some(Item::Tool {
                    done, error: slot, ..
                }) = found
                {
                    *done = true;
                    *slot = error.clone();
                } else {
                    self.items.push(Item::Tool {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        done: true,
                        error: error.clone(),
                    });
                }
            }
            ApiEvent::TurnDone { .. } => {
                self.flush_reasoning();
                self.flush_streaming();
                self.connection_phase.clear();
            }
            ApiEvent::SessionStatus { status, .. } => {
                self.status = status.clone();
                if status == "idle" {
                    self.flush_reasoning();
                    self.flush_streaming();
                    self.connection_phase.clear();
                }
            }
            ApiEvent::ConnectionPhase { phase, .. } => {
                self.connection_phase = phase.clone();
            }
            ApiEvent::SessionRenamed { display_title, .. } => {
                self.title = display_title.clone().into();
            }
            ApiEvent::Error { message, .. } => {
                self.flush_streaming();
                self.items.push(Item::Error(message.clone()));
            }
            _ => {}
        }
        if self.stick_to_bottom {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn flush_streaming(&mut self) {
        if !self.streaming_text.trim().is_empty() {
            self.items
                .push(Item::Assistant(std::mem::take(&mut self.streaming_text)));
        } else {
            self.streaming_text.clear();
        }
    }

    fn flush_reasoning(&mut self) {
        if !self.streaming_reasoning.trim().is_empty() {
            self.items.push(Item::Reasoning(std::mem::take(
                &mut self.streaming_reasoning,
            )));
        } else {
            self.streaming_reasoning.clear();
        }
    }

    pub fn is_busy(&self) -> bool {
        self.status != "idle" || !self.streaming_text.is_empty()
    }

    pub fn message_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.flush_reasoning();
        self.flush_streaming();
        self.items.push(Item::Error(message));
        self.status = "idle".into();
        self.connection_phase.clear();
        self.stick_to_bottom = true;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn render_item(item: &Item, window: &Window) -> gpui::AnyElement {
        match item {
            Item::User(text) => div()
                .flex()
                .flex_col()
                .bg(Theme::USER_BG)
                .rounded_md()
                .px_3()
                .py_2()
                .text_color(Theme::TEXT_USER)
                .child(markdown::render(text, window))
                .into_any_element(),
            Item::Assistant(text) => div()
                .px_1()
                .text_color(Theme::TEXT)
                .child(markdown::render(text, window))
                .into_any_element(),
            Item::Reasoning(text) => {
                let summary: String = text.chars().take(280).collect();
                div()
                    .px_1()
                    .text_size(px(12.0))
                    .text_color(Theme::REASONING)
                    .italic()
                    .line_height(relative(1.4))
                    .child(summary)
                    .into_any_element()
            }
            Item::Tool {
                name, done, error, ..
            } => {
                let (symbol, color) = match (done, error) {
                    (false, _) => ("●", Theme::WARN),
                    (true, None) => ("✓", Theme::OK),
                    (true, Some(_)) => ("✗", Theme::ERROR),
                };
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .bg(Theme::TOOL_BG)
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .text_size(px(12.0))
                    .font_family(Theme::FONT_MONO)
                    .child(div().text_color(color).child(symbol))
                    .child(div().text_color(Theme::TOOL_TEXT).child(name.clone()))
                    .children(error.clone().map(|e| {
                        let short: String = e.chars().take(120).collect();
                        div().text_color(Theme::ERROR).child(short)
                    }))
                    .into_any_element()
            }
            Item::Error(message) => div()
                .px_2()
                .py_1()
                .text_size(px(12.0))
                .text_color(Theme::ERROR)
                .child(message.clone())
                .into_any_element(),
        }
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut transcript = div()
            .id("transcript")
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .min_h_0()
            .px_3()
            .py_2()
            .overflow_y_scroll()
            .track_scroll(&self.scroll);

        for item in &self.items {
            transcript = transcript.child(Self::render_item(item, window));
        }
        if !self.streaming_reasoning.is_empty() {
            transcript = transcript.child(Self::render_item(
                &Item::Reasoning(self.streaming_reasoning.clone()),
                window,
            ));
        }
        if !self.streaming_text.is_empty() {
            transcript = transcript.child(Self::render_item(
                &Item::Assistant(self.streaming_text.clone()),
                window,
            ));
        }
        if self.items.is_empty() && self.streaming_text.is_empty() {
            transcript = transcript.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(Theme::TEXT_DIM)
                    .text_size(px(13.0))
                    .child("no messages yet"),
            );
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(transcript)
            // Input
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(Theme::PANEL_BORDER)
                    .child(self.input.clone()),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    this.focus_input(window, cx);
                    cx.notify();
                }),
            )
    }
}

impl Focusable for Panel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn short_id(session_id: &str) -> String {
    let tail: String = session_id
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("session {tail}")
}
