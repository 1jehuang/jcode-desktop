//! Panel: one Jcode session as a spatial card with a live transcript.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, ScrollHandle, SharedString, Window,
    div, prelude::*, px, relative,
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
        /// Raw tool arguments as streamed, used for the one-line summary and
        /// the expanded detail.
        input: String,
        output: String,
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
    /// Model id serving this session, e.g. `gpt-5.6-sol`.
    pub model: Option<String>,
    /// Provider display name, e.g. `openai` or `anthropic`.
    pub provider: Option<String>,
    /// Credential route for the current model, e.g. `oauth` or `api key`.
    pub auth_method: Option<String>,
    /// Reasoning effort, e.g. `high`, when the provider exposes it.
    pub reasoning_effort: Option<String>,
    /// Latest token usage: (input, output, cache_read) from the last update.
    token_usage: Option<(u64, u64, u64)>,
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
    /// Tool rows the user expanded, keyed by call id.
    expanded_tools: HashSet<String>,
    /// Reasoning rows the user expanded, keyed by transcript index.
    expanded_reasoning: HashSet<usize>,
    pending_users: VecDeque<usize>,
    accepted_users: HashMap<usize, Instant>,
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
            PromptInput::new(cx, "message jcode...", move |content, images, _window, _app| {
                send_bridge.send(Command::Send {
                    session_id: send_session.clone(),
                    content,
                    images,
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
            model: None,
            provider: None,
            auth_method: None,
            reasoning_effort: None,
            token_usage: None,
            items: demo_items(),
            streaming_text: String::new(),
            streaming_reasoning: String::new(),
            input,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            stick_to_bottom: true,
            bridge,
            history_loaded: false,
            expanded_tools: HashSet::new(),
            expanded_reasoning: HashSet::new(),
            pending_users: VecDeque::new(),
            accepted_users: HashMap::new(),
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
                PromptInput::new(cx, "message jcode...", move |content, images, _window, app| {
                    bridge.send(Command::Send {
                        session_id: session_id.clone(),
                        content: content.clone(),
                        images,
                    });
                    if let Some(panel) = weak.upgrade() {
                        panel.update(app, |this, cx| {
                            let index = this.items.len();
                            this.items.push(Item::User(content));
                            this.pending_users.push_back(index);
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
        // History goes first; anything echoed locally before it arrived is
        // appended, minus the duplicate the server already knows about.
        let mut existing = std::mem::take(&mut self.items);
        existing.retain(|item| match item {
            Item::User(text) => !matches!(items.last(), Some(Item::User(last)) if last == text),
            _ => true,
        });
        let history_len = items.len();
        self.pending_users = self
            .pending_users
            .drain(..)
            .map(|index| index + history_len)
            .collect();
        self.accepted_users = std::mem::take(&mut self.accepted_users)
            .into_iter()
            .map(|(index, at)| (index + history_len, at))
            .collect();
        items.append(&mut existing);
        self.items = items;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Apply a streaming event addressed to this session.
    pub fn apply(&mut self, event: &ApiEvent, cx: &mut Context<Self>) {
        match event {
            ApiEvent::MessageAccepted { .. } => {
                acknowledge_next(&mut self.pending_users, &mut self.accepted_users, Instant::now());
            }
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
                    input: String::new(),
                    output: String::new(),
                    done: false,
                    error: None,
                });
            }
            ApiEvent::ToolInputDelta { call_id, delta, .. } => {
                if let Some(Item::Tool { input, .. }) = self.find_tool(call_id) {
                    input.push_str(delta);
                }
            }
            ApiEvent::ToolDone {
                call_id,
                name,
                output,
                error,
                ..
            } => {
                if let Some(Item::Tool {
                    done,
                    error: slot,
                    output: output_slot,
                    ..
                }) = self.find_tool(call_id)
                {
                    *done = true;
                    *slot = error.clone();
                    *output_slot = output.clone();
                } else {
                    self.items.push(Item::Tool {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        input: String::new(),
                        output: output.clone(),
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
            ApiEvent::ModelInfo {
                provider,
                model,
                reasoning_effort,
                ..
            } => {
                if provider.is_some() {
                    self.provider = provider.clone();
                }
                if reasoning_effort.is_some() {
                    self.reasoning_effort = reasoning_effort.clone();
                }
                if model.is_some() {
                    self.model = model.clone();
                    // The route catalog keyed the auth method by model, so a
                    // switch invalidates it until the next RuntimeInfo.
                    self.auth_method = None;
                }
            }
            ApiEvent::RuntimeInfo {
                provider,
                model,
                reasoning_effort,
                routes,
                ..
            } => {
                if provider.is_some() {
                    self.provider = provider.clone();
                }
                if model.is_some() {
                    self.model = model.clone();
                }
                if reasoning_effort.is_some() {
                    self.reasoning_effort = reasoning_effort.clone();
                }
                self.auth_method = auth_method_for_model(self.model.as_deref(), routes);
            }
            ApiEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                ..
            } => {
                self.token_usage = Some((*input, *output, cache_read_input.unwrap_or(0)));
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

    fn find_tool(&mut self, call_id: &str) -> Option<&mut Item> {
        self.items.iter_mut().rev().find(
            |item| matches!(item, Item::Tool { call_id: existing, .. } if existing == call_id),
        )
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

    /// A one-line footer describing what the session is doing, when that is
    /// not simply "idle". Provider phase wins over the coarse status because
    /// it is what tells the user progress is still happening.
    fn status_line(&self) -> Option<String> {
        if !self.connection_phase.is_empty() {
            return Some(self.connection_phase.clone());
        }
        if self.status != "idle" {
            return Some(self.status.replace('_', " "));
        }
        None
    }

    fn render_item(
        &self,
        index: usize,
        item: &Item,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match item {
            Item::User(text) => {
                let now = Instant::now();
                let pending = self.pending_users.contains(&index);
                let (offset, opacity, animating) = self.accepted_users.get(&index)
                    .map(|at| crate::ack::motion(*at, now))
                    .unwrap_or((0.0, if pending { crate::ack::PENDING_TONE } else { 1.0 }, false));
                if animating {
                    window.request_animation_frame();
                }
                div()
                    .flex()
                    .flex_col()
                    .ml(px(offset))
                    .opacity(opacity)
                    .bg(Theme::USER_BG)
                    .rounded_md()
                    .px_3()
                    .py_2()
                    .text_color(Theme::TEXT_USER)
                    .child(markdown::render(text, window))
                    .into_any_element()
            }
            Item::Assistant(text) => div()
                .px_1()
                .text_color(Theme::TEXT)
                .child(markdown::render(text, window))
                .into_any_element(),
            Item::Reasoning(text) => {
                let expanded = self.expanded_reasoning.contains(&index);
                let body: String = if expanded {
                    text.clone()
                } else {
                    condense(text, 200)
                };
                let long = text.chars().count() > 200;
                div()
                    .id(("reasoning", index))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(Theme::REASONING_BG)
                    .when(long, |el| {
                        el.cursor_pointer().on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                if !this.expanded_reasoning.remove(&index) {
                                    this.expanded_reasoning.insert(index);
                                }
                                cx.notify();
                            }),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1p5()
                            .items_center()
                            .text_size(px(10.0))
                            .text_color(Theme::TEXT_FAINT)
                            .child("thinking")
                            .when(long, |el| {
                                el.child(if expanded { "collapse" } else { "expand" })
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(Theme::REASONING)
                            .italic()
                            .line_height(relative(1.45))
                            .child(body),
                    )
                    .into_any_element()
            }
            Item::Tool {
                call_id,
                name,
                input,
                output,
                done,
                error,
            } => {
                let (symbol, color) = match (done, error) {
                    (false, _) => ("◇", Theme::WARN),
                    (true, None) => ("◆", Theme::OK),
                    (true, Some(_)) => ("✗", Theme::ERROR),
                };
                let expanded = self.expanded_tools.contains(call_id);
                let summary = tool_summary(input);
                let detail = tool_detail(input, output);
                let has_detail = !detail.is_empty();
                let call_id = call_id.clone();
                div()
                    .id(("tool", index))
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .bg(Theme::TOOL_BG)
                    .border_1()
                    .border_color(Theme::TOOL_BORDER)
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .px_2()
                            .py_1()
                            .text_size(px(11.5))
                            .font_family(Theme::FONT_MONO)
                            .when(has_detail, |el| {
                                el.cursor_pointer().on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        if !this.expanded_tools.remove(&call_id) {
                                            this.expanded_tools.insert(call_id.clone());
                                        }
                                        cx.notify();
                                    }),
                                )
                            })
                            .child(div().flex_none().text_color(color).child(symbol))
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Theme::TOOL_TEXT)
                                    .child(name.clone()),
                            )
                            .when(!summary.is_empty(), |el| {
                                el.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_color(Theme::TEXT_DIM)
                                        .child(summary),
                                )
                            })
                            .when(!*done, |el| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.0))
                                        .text_color(Theme::TEXT_FAINT)
                                        .child("running"),
                                )
                            })
                            .when(has_detail, |el| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.0))
                                        .text_color(Theme::TEXT_FAINT)
                                        .child(if expanded { "▾" } else { "▸" }),
                                )
                            }),
                    )
                    .when(expanded && has_detail, |el| {
                        el.child(
                            div()
                                .border_t_1()
                                .border_color(Theme::TOOL_BORDER)
                                .bg(Theme::CODE_BG)
                                .px_2p5()
                                .py_1p5()
                                .font_family(Theme::FONT_MONO)
                                .text_size(px(11.5))
                                .line_height(relative(1.45))
                                .text_color(Theme::CODE_TEXT)
                                .child(detail),
                        )
                    })
                    .children(error.clone().map(|message| {
                        div()
                            .px_2()
                            .py_1()
                            .border_t_1()
                            .border_color(Theme::TOOL_BORDER)
                            .text_size(px(11.5))
                            .font_family(Theme::FONT_MONO)
                            .text_color(Theme::ERROR)
                            .child(condense(&message, 300))
                    }))
                    .into_any_element()
            }
            Item::Error(message) => div()
                .flex()
                .flex_row()
                .gap_2()
                .items_start()
                .px_2p5()
                .py_1p5()
                .rounded_md()
                .bg(Theme::ERROR_BG)
                .border_1()
                .border_color(Theme::TOOL_BORDER)
                .text_size(px(12.0))
                .text_color(Theme::ERROR)
                .child(div().flex_none().child("!"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_height(relative(1.45))
                        .child(message.clone()),
                )
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
            .gap_2p5()
            .flex_1()
            .min_h_0()
            .px_3()
            .py_2p5()
            .text_size(px(13.5))
            .overflow_y_scroll()
            // Vertical deltas only: horizontal two-finger swipes pan the
            // workspace canvas instead of nudging the transcript.
            .restrict_scroll_to_axis()
            .track_scroll(&self.scroll);

        // Live rows are appended after the settled ones and share the same
        // renderer, so a streaming turn looks identical to a finished one.
        let mut rows: Vec<(usize, Item)> =
            self.items.iter().cloned().enumerate().collect::<Vec<_>>();
        if !self.streaming_reasoning.is_empty() {
            rows.push((
                usize::MAX - 1,
                Item::Reasoning(self.streaming_reasoning.clone()),
            ));
        }
        if !self.streaming_text.is_empty() {
            rows.push((usize::MAX, Item::Assistant(self.streaming_text.clone())));
        }

        let mut previous: Option<&'static str> = None;
        for (index, item) in &rows {
            let role = role_of(item);
            // Group consecutive rows from the same speaker: only the first
            // gets a caption, and tool runs sit tight under their turn.
            let show_label = role.is_some() && role != previous;
            previous = role.or(previous);
            let element = self.render_item(*index, item, window, cx);
            transcript = transcript.child(match (show_label, role) {
                (true, Some(label)) => role_caption(label, element),
                _ => element,
            });
        }

        if rows.is_empty() {
            transcript = transcript.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_color(Theme::TEXT_DIM)
                            .text_size(px(13.0))
                            .child("no messages yet"),
                    )
                    .child(
                        div()
                            .text_color(Theme::TEXT_FAINT)
                            .text_size(px(11.0))
                            .child("type below to start this session"),
                    ),
            );
        }

        let status_line = self.status_line();
        let meta_line = meta_line(
            self.working_dir.as_deref(),
            self.model.as_deref(),
            self.provider.as_deref(),
            self.auth_method.as_deref(),
            self.reasoning_effort.as_deref(),
            self.token_usage,
        );

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(transcript)
            .children(status_line.map(|text| {
                div()
                    .flex()
                    .flex_row()
                    .gap_1p5()
                    .items_center()
                    .px_3()
                    .py_1()
                    .text_size(px(10.5))
                    .font_family(Theme::FONT_MONO)
                    .text_color(Theme::TEXT_FAINT)
                    .child("·")
                    .child(text)
            }))
            // Session identity: where it runs, what serves it, and how full
            // the context is. Always present, so the user never has to ask
            // "which model is this?" mid-conversation.
            .children(meta_line.map(|text| {
                div()
                    // Tagged so a render test can prove the footer painted,
                    // not just that meta_line() produced a string.
                    .debug_selector(|| "panel-meta".into())
                    .px_3()
                    .py_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(10.0))
                    .font_family(Theme::FONT_MONO)
                    .text_color(Theme::TEXT_FAINT)
                    .child(text)
            }))
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

/// Which speaker a row belongs to, or None for rows that carry no caption.
fn role_of(item: &Item) -> Option<&'static str> {
    match item {
        Item::User(_) => Some("you"),
        Item::Assistant(_) => Some("jcode"),
        Item::Reasoning(_) | Item::Tool { .. } | Item::Error(_) => None,
    }
}

/// A labelled role row: a small caption above the message body, so a long
/// transcript stays scannable without heavyweight avatars.
fn role_caption(label: &'static str, body: gpui::AnyElement) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.0))
                .text_color(Theme::TEXT_FAINT)
                .font_family(Theme::FONT_MONO)
                .child(label),
        )
        .child(body)
        .into_any_element()
}

/// The credential route serving `model`, phrased for humans. The route
/// catalog's `api_method` values are stable ids like `openai-oauth` or
/// `anthropic-api-key`; the footer says "oauth" or "api key".
fn auth_method_for_model(
    model: Option<&str>,
    routes: &[jcode_sdk::ModelRouteInfo],
) -> Option<String> {
    let model = model?;
    let route = routes.iter().find(|route| route.model == model)?;
    let method = route.api_method.to_lowercase();
    Some(if method.contains("oauth") {
        "oauth".to_string()
    } else if method.contains("api-key") || method.contains("api_key") {
        "api key".to_string()
    } else {
        method
    })
}

/// The identity footer: directory, model (provider, auth, effort), and
/// context usage. Absent parts are simply omitted, so the line never shows
/// placeholders.
fn meta_line(
    working_dir: Option<&str>,
    model: Option<&str>,
    provider: Option<&str>,
    auth_method: Option<&str>,
    reasoning_effort: Option<&str>,
    token_usage: Option<(u64, u64, u64)>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(dir) = working_dir.filter(|dir| !dir.is_empty()) {
        parts.push(compact_dir(dir));
    }
    if let Some(model) = model {
        let mut qualifiers: Vec<&str> = Vec::new();
        if let Some(provider) = provider {
            qualifiers.push(provider);
        }
        if let Some(auth) = auth_method {
            qualifiers.push(auth);
        }
        if let Some(effort) = reasoning_effort {
            qualifiers.push(effort);
        }
        if qualifiers.is_empty() {
            parts.push(model.to_string());
        } else {
            parts.push(format!("{model} ({})", qualifiers.join(", ")));
        }
    }
    if let Some((input, output, cache_read)) = token_usage {
        let used = input + output + cache_read;
        match model.and_then(context_window_for_model) {
            Some(window) => {
                let percent = (used as f64 / window as f64 * 100.0).min(100.0);
                parts.push(format!(
                    "{} / {} ({percent:.0}%)",
                    format_tokens(used),
                    format_tokens(window)
                ));
            }
            None => parts.push(format!("{} tokens", format_tokens(used))),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("  ·  "))
}

/// Shorten a home-relative path for the footer.
fn compact_dir(path: &str) -> String {
    match std::env::var("HOME").ok() {
        Some(home) if path == home => "~".to_string(),
        Some(home) => match path.strip_prefix(&format!("{home}/")) {
            Some(relative) => format!("~/{relative}"),
            None => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// Best-effort context window by model family. The harness API does not carry
/// the provider's exact window, so this mirrors jcode's own fallbacks for the
/// families the user actually runs; unknown models show raw token counts.
fn context_window_for_model(model: &str) -> Option<u64> {
    let m = model.to_lowercase();
    if m.starts_with("gpt-5.3-codex-spark") {
        return Some(128_000);
    }
    if m.contains("chat") && m.starts_with("gpt-5") {
        return Some(128_000);
    }
    if m.starts_with("gpt-5.4") {
        return Some(1_000_000);
    }
    if m.starts_with("gpt-5") {
        return Some(272_000);
    }
    if m.starts_with("claude-")
        || m.starts_with("fable")
        || m.contains("opus")
        || m.contains("sonnet")
        || m.contains("haiku")
    {
        return Some(200_000);
    }
    if m.starts_with("gemini-") {
        return Some(1_000_000);
    }
    None
}

/// `12500` -> `12.5k`, `1048576` -> `1.0m`; small counts stay exact.
fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}m", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Collapse whitespace and clip to a readable length.
fn condense(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let clipped: String = flat.chars().take(limit).collect();
    format!("{}…", clipped.trim_end())
}

/// The most useful field of a tool call, rendered as a one-line summary.
fn tool_summary(input: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(input) else {
        return condense(input, 90);
    };
    const PREFERRED: &[&str] = &[
        "command",
        "query",
        "file_path",
        "path",
        "pattern",
        "url",
        "prompt",
        "content",
        "task",
        "action",
        "intent",
    ];
    for key in PREFERRED {
        if let Some(found) = value.get(*key).and_then(json_scalar) {
            if !found.trim().is_empty() {
                return condense(&found, 90);
            }
        }
    }
    condense(input, 90)
}

fn json_scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Expanded tool detail: pretty arguments plus a clipped output tail.
fn tool_detail(input: &str, output: &str) -> String {
    let mut parts = Vec::new();
    if !input.trim().is_empty() && input.trim() != "{}" {
        let pretty = serde_json::from_str::<serde_json::Value>(input)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .unwrap_or_else(|| input.trim().to_string());
        parts.push(clip_lines(&pretty, 40));
    }
    if !output.trim().is_empty() {
        parts.push(clip_lines(output.trim(), 40));
    }
    parts.join("\n\n")
}

/// Keep a block short from the top, noting how much was hidden.
fn acknowledge_next(
    pending: &mut VecDeque<usize>,
    accepted: &mut HashMap<usize, Instant>,
    now: Instant,
) {
    if let Some(index) = pending.pop_front() {
        accepted.insert(index, now);
    }
}

fn clip_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    let hidden = lines.len() - max_lines;
    let mut kept = lines[..max_lines].join("\n");
    kept.push_str(&format!("\n… {hidden} more lines"));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_acceptance_promotes_the_oldest_pending_prompt() {
        let now = Instant::now();
        let mut pending = VecDeque::from([4, 7]);
        let mut accepted = HashMap::new();
        acknowledge_next(&mut pending, &mut accepted, now);
        assert_eq!(pending, VecDeque::from([7]));
        assert_eq!(accepted.get(&4), Some(&now));
    }

    #[test]
    fn tool_summary_prefers_the_meaningful_field() {
        assert_eq!(
            tool_summary(r#"{"intent":"look","command":"cargo test"}"#),
            "cargo test"
        );
        assert_eq!(tool_summary(r#"{"other":1}"#), r#"{"other":1}"#);
    }

    #[test]
    fn condense_flattens_and_clips() {
        assert_eq!(condense("a\n  b\tc", 90), "a b c");
        assert_eq!(condense("abcdef", 3), "abc…");
    }

    #[test]
    fn tool_detail_pretty_prints_and_clips() {
        let detail = tool_detail(r#"{"a":1}"#, "line1\nline2");
        assert!(detail.contains("\"a\": 1"));
        assert!(detail.contains("line2"));
        let long: String = (0..50).map(|n| format!("l{n}\n")).collect();
        assert!(tool_detail("{}", &long).contains("more lines"));
    }

    fn route(model: &str, api_method: &str) -> jcode_sdk::ModelRouteInfo {
        jcode_sdk::ModelRouteInfo {
            model: model.into(),
            provider: "openai".into(),
            api_method: api_method.into(),
            available: true,
            detail: String::new(),
        }
    }

    #[test]
    fn auth_method_is_read_from_the_current_models_route() {
        let routes = vec![
            route("gpt-5.6-sol", "openai-oauth"),
            route("claude-fable-5", "anthropic-api-key"),
        ];
        assert_eq!(
            auth_method_for_model(Some("gpt-5.6-sol"), &routes).as_deref(),
            Some("oauth")
        );
        assert_eq!(
            auth_method_for_model(Some("claude-fable-5"), &routes).as_deref(),
            Some("api key")
        );
        // Unknown model or no model: nothing to claim.
        assert_eq!(auth_method_for_model(Some("other"), &routes), None);
        assert_eq!(auth_method_for_model(None, &routes), None);
    }

    #[test]
    fn meta_line_shows_directory_model_auth_and_context() {
        let line = meta_line(
            Some("/srv/project"),
            Some("gpt-5.6-sol"),
            Some("openai"),
            Some("oauth"),
            Some("high"),
            Some((100_000, 8_000, 28_000)),
        )
        .unwrap();
        assert_eq!(
            line,
            "/srv/project  ·  gpt-5.6-sol (openai, oauth, high)  ·  136.0k / 272.0k (50%)"
        );
    }

    #[test]
    fn meta_line_omits_missing_parts_instead_of_showing_placeholders() {
        // No identity at all: no footer.
        assert_eq!(meta_line(None, None, None, None, None, None), None);
        // Model only: just the model, no empty parens.
        assert_eq!(
            meta_line(None, Some("gpt-5.6"), None, None, None, None).as_deref(),
            Some("gpt-5.6")
        );
        // Unknown model family: raw token count, no bogus percentage.
        assert_eq!(
            meta_line(
                None,
                Some("mystery-model"),
                None,
                None,
                None,
                Some((1_500, 500, 0))
            )
            .as_deref(),
            Some("mystery-model  ·  2.0k tokens")
        );
    }

    #[test]
    fn context_windows_match_the_families_jcode_uses() {
        assert_eq!(context_window_for_model("gpt-5.6-sol"), Some(272_000));
        assert_eq!(context_window_for_model("gpt-5.4-alto"), Some(1_000_000));
        assert_eq!(
            context_window_for_model("gpt-5.2-chat-latest"),
            Some(128_000)
        );
        assert_eq!(
            context_window_for_model("claude-sonnet-4-20250514"),
            Some(200_000)
        );
        assert_eq!(context_window_for_model("gemini-3-pro"), Some(1_000_000));
        assert_eq!(context_window_for_model("mystery"), None);
    }

    #[test]
    fn token_counts_format_compactly() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(12_500), "12.5k");
        assert_eq!(format_tokens(1_048_576), "1.0m");
    }

    /// The acceptance path: real events land in a real panel inside a painted
    /// window, and the footer element occupies space on screen. Without this,
    /// the tests above only prove the string is right, not that anyone sees it.
    #[gpui::test]
    fn the_identity_footer_paints_after_runtime_info_and_token_events(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            let _ = window;
            workspace
        });
        vcx.run_until_parked();

        // A fresh panel knows nothing: no footer may be painted.
        assert!(
            vcx.debug_bounds("panel-meta").is_none(),
            "no identity footer before any identity is known"
        );

        // The events the harness worker forwards after attach and during a turn.
        workspace.update(vcx, |workspace, cx| {
            let panel = workspace.test_panel(0).expect("panel exists");
            panel.update(cx, |panel, cx| {
                panel.apply(
                    &ApiEvent::RuntimeInfo {
                        session_id: "session-a".into(),
                        provider: Some("openai".into()),
                        model: Some("gpt-5.6-sol".into()),
                        reasoning_effort: Some("high".into()),
                        routes: vec![jcode_sdk::ModelRouteInfo {
                            model: "gpt-5.6-sol".into(),
                            provider: "openai".into(),
                            api_method: "openai-oauth".into(),
                            available: true,
                            detail: String::new(),
                        }],
                    },
                    cx,
                );
                panel.apply(
                    &ApiEvent::TokenUsage {
                        session_id: "session-a".into(),
                        input: 100_000,
                        output: 8_000,
                        cache_read_input: Some(28_000),
                    },
                    cx,
                );
                assert_eq!(panel.model.as_deref(), Some("gpt-5.6-sol"));
                assert_eq!(panel.provider.as_deref(), Some("openai"));
                assert_eq!(panel.auth_method.as_deref(), Some("oauth"));
                assert_eq!(panel.reasoning_effort.as_deref(), Some("high"));
            });
        });
        vcx.run_until_parked();

        let bounds = vcx
            .debug_bounds("panel-meta")
            .expect("the identity footer should have painted");
        assert!(
            bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.),
            "the footer must occupy real space, got {bounds:?}"
        );
    }
}

/// `JCODE_DESKTOP_DEMO_TRANSCRIPT=1` seeds one panel with a sample of every
/// transcript shape, so rendering changes can be reviewed without driving a
/// real session through each case.
fn demo_items() -> Vec<Item> {
    if std::env::var("JCODE_DESKTOP_DEMO_TRANSCRIPT").as_deref() != Ok("1") {
        return Vec::new();
    }
    vec![
        Item::User("Explain **markdown** rendering and show `code`, a [link](https://example.com), ~~old~~ new.".into()),
        Item::Reasoning("The user wants a survey of the renderer. I should cover blocks, inline spans, and how streaming text is handled while a turn is still in flight, then mention the tables and code paths in order.".into()),
        Item::Tool {
            call_id: "1".into(),
            name: "bash".into(),
            input: r#"{"command":"cargo test --offline","intent":"run the suite"}"#.into(),
            output: "test result: ok. 61 passed".into(),
            done: true,
            error: None,
        },
        Item::Tool {
            call_id: "2".into(),
            name: "read".into(),
            input: r#"{"file_path":"src/markdown.rs"}"#.into(),
            output: String::new(),
            done: false,
            error: None,
        },
        Item::Assistant(
            "# Heading one\n## Heading two\n\nA paragraph with *italic*, **bold**, `inline code`, and math $e^{i\\pi}+1=0$.\n\n- top level\n  - nested item\n- [x] finished task\n- [ ] pending task\n\n1. first\n2. second\n\n> A quote line\n> continued here\n\n| block | supported |\n| --- | --- |\n| tables | yes |\n| code | yes |\n\n```rust\nfn main() {\n    // a comment\n    let name = \"world\";\n    println!(\"hello {name}\");\n}\n```\n\n$$\n\\sum_{i=0}^{n} i^2\n$$\n\n---\n\nDone."
                .into(),
        ),
        Item::Error("provider returned 429: rate limited, retrying".into()),
    ]
}
