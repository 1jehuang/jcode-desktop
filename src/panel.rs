//! Panel: one Jcode session as a spatial card with a live transcript.

use std::collections::HashSet;

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
        // History goes first; anything echoed locally before it arrived is
        // appended, minus the duplicate the server already knows about.
        let mut existing = std::mem::take(&mut self.items);
        existing.retain(|item| match item {
            Item::User(text) => !matches!(items.last(), Some(Item::User(last)) if last == text),
            _ => true,
        });
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
