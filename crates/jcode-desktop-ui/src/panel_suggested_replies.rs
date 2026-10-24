//! Contextual Suggested Replies with Default Action Focus for Jcode Desktop.
//!
//! Shares the GPUI-free evaluation engine in `jcode_base::suggested_replies`
//! with the TUI while rendering native `rounded_full` composer pills above the
//! Desktop prompt input in `panel_composer.rs`.

use std::collections::HashMap;

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    Window, div, px,
};
use jcode_base::config::SuggestedRepliesConfig;
use jcode_base::suggested_replies::{
    SuggestedRepliesContext, SuggestedRepliesResponse, evaluate_cascade_with_context,
    score_locally_with_context_and_options,
};

use super::*;
use crate::panel::composer::{TAB_HEIGHT, composer_pill};

/// Maximum cached turn evaluations per panel.
const MAX_SUGGESTION_CACHE_ENTRIES: usize = 16;
/// Maximum characters displayed on a pill before ellipsis truncation.
const MAX_PILL_LABEL_CHARS: usize = 64;

#[derive(Clone, Debug, Default)]
pub(in crate::panel) struct SuggestedRepliesState {
    /// Optional per-session `/suggestions on|off` override.
    pub(in crate::panel) enabled_override: Option<bool>,
    /// Latest evaluated cascade output for the current idle turn.
    pub(in crate::panel) output: Option<SuggestedRepliesResponse>,
    /// `(session_id, transcript_revision)` that produced `output`.
    pub(in crate::panel) evaluated_key: Option<(String, u64)>,
    /// `(session_id, transcript_revision)` currently being evaluated in the background.
    pub(in crate::panel) in_flight_key: Option<(String, u64)>,
    /// Focused suggestion pill index (`0` = primary/first pill, `1..` = secondary pills).
    pub(in crate::panel) focused_index: Option<usize>,
    /// Text staged into `PromptInput` by a suggestion interaction.
    pub(in crate::panel) staged_text: Option<String>,
    /// Cached cascade outputs keyed by `(session_id, transcript_revision)`.
    pub(in crate::panel) cache: HashMap<(String, u64), SuggestedRepliesResponse>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::panel) struct DisplayedSuggestionPill {
    pub index: usize,
    pub is_default_action: bool,
    pub badge: &'static str,
    pub text: String,
    pub score: f32,
}

impl SuggestedRepliesState {
    pub(in crate::panel) fn is_enabled(&self, cfg: &SuggestedRepliesConfig) -> bool {
        self.enabled_override.unwrap_or(cfg.enabled)
    }

    pub(in crate::panel) fn clear_active(&mut self) {
        self.output = None;
        self.evaluated_key = None;
        self.in_flight_key = None;
        self.focused_index = None;
        self.staged_text = None;
    }

    pub(in crate::panel) fn detach_if_edited(&mut self, current_input: &str) -> bool {
        if let Some(staged) = &self.staged_text
            && staged != current_input
        {
            self.focused_index = None;
            self.staged_text = None;
            return true;
        }
        false
    }
}

fn char_display_cells(ch: char) -> usize {
    if ('\u{1100}'..='\u{115F}').contains(&ch)
        || ('\u{2E80}'..='\u{A4CF}').contains(&ch)
        || ('\u{AC00}'..='\u{D7A3}').contains(&ch)
        || ('\u{F900}'..='\u{FAFF}').contains(&ch)
        || ('\u{FE10}'..='\u{FE6F}').contains(&ch)
        || ('\u{FF00}'..='\u{FF60}').contains(&ch)
        || ('\u{FFE0}'..='\u{FFE6}').contains(&ch)
        || ('\u{1F300}'..='\u{1FAFF}').contains(&ch)
    {
        2
    } else {
        1
    }
}

fn truncate_pill_text(text: &str, max_cells: usize) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let total_cells: usize = single_line.chars().map(char_display_cells).sum();
    if total_cells <= max_cells {
        return single_line;
    }
    let budget = max_cells.saturating_sub(1);
    let mut used = 0usize;
    let mut kept = String::new();
    for ch in single_line.chars() {
        let w = char_display_cells(ch);
        if used + w > budget {
            break;
        }
        used += w;
        kept.push(ch);
    }
    format!("{kept}…")
}

fn evaluate_deterministic(
    ctx: &SuggestedRepliesContext,
    cfg: &SuggestedRepliesConfig,
) -> SuggestedRepliesResponse {
    let threshold = cfg.clamped_confidence_threshold();
    let max_secondary = cfg.clamped_max_secondary();
    let custom_prompts = cfg.sanitized_custom_prompts();
    score_locally_with_context_and_options(
        ctx,
        None,
        &custom_prompts,
        None,
        threshold,
        max_secondary,
        cfg.enable_finish_category,
    )
}

impl Panel {
    pub(in crate::panel) fn suggested_replies_config(&self) -> SuggestedRepliesConfig {
        let mut cfg = jcode_base::config::config().suggested_replies.clone();
        if let Some(override_enabled) = self.suggested_replies.enabled_override {
            cfg.enabled = override_enabled;
        }
        cfg
    }

    pub(in crate::panel) fn suggestions_allowed(&self) -> bool {
        let cfg = self.suggested_replies_config();
        cfg.enabled
            && !self.transcript_only
            && !self.is_side_document()
            && self.preview_state.is_none()
            && self.terminal.is_none()
            && self.code_file.is_none()
            && self.applet.is_none()
            && self.todoist.is_none()
            && self.orchestration.is_none()
            && self.login.is_none()
            && !self.recovery_picker_open
            && !self.model_picker_open
            && !self.is_pending_session()
            && !self.prompt_queue.waiting_for_connection
            && !self.prompt_queue.has_queued_prompts()
            && !self.prompt_queue.is_auto_poke_awaiting_state()
            && self.pending_users.is_empty()
            && self.status == "idle"
            && !self.activity_active()
            && self.streaming_text.is_empty()
            && self.streaming_reasoning.is_empty()
            && !self.items.is_empty()
    }

    pub(in crate::panel) fn clear_suggested_replies(&mut self) {
        self.suggested_replies.clear_active();
    }

    pub(in crate::panel) fn on_input_changed_for_suggestions(
        &mut self,
        content: &str,
        cx: &mut Context<Self>,
    ) {
        if self.suggested_replies.detach_if_edited(content) {
            cx.notify();
        }
    }

    pub(in crate::panel) fn clear_suggested_reply_focus_on_escape(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.suggested_replies.focused_index.is_some()
            || self.suggested_replies.staged_text.is_some()
        {
            self.suggested_replies.focused_index = None;
            self.suggested_replies.staged_text = None;
            cx.notify();
        }
    }

    pub(in crate::panel) fn cancel_suggested_reply_focus(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.suggested_replies.focused_index.is_none()
            && self.suggested_replies.staged_text.is_none()
        {
            return false;
        }
        let staged = self.suggested_replies.staged_text.take();
        self.suggested_replies.focused_index = None;
        if let Some(staged_text) = staged {
            let current = self.input.read(cx).content.to_string();
            if current == staged_text {
                self.input.update(cx, |input, cx| {
                    input.set_content(String::new(), cx);
                });
            }
        }
        cx.notify();
        true
    }

    pub(in crate::panel) fn displayed_suggestion_pills(&self) -> Vec<DisplayedSuggestionPill> {
        let Some(output) = &self.suggested_replies.output else {
            return Vec::new();
        };
        let cfg = self.suggested_replies_config();
        let mut pills = Vec::new();

        let map_pill = |index: usize, is_default_action: bool, text: &str| {
            let badge = output.badge_for_text(text);
            let score = output
                .confidence_for_text(text)
                .unwrap_or(if is_default_action { 0.75 } else { 0.50 });
            DisplayedSuggestionPill {
                index,
                is_default_action,
                badge,
                text: text.to_string(),
                score,
            }
        };

        if let Some(default_action) = &output.default_action {
            pills.push(map_pill(0, true, default_action));
        }
        for secondary in output
            .secondary_suggestions
            .iter()
            .take(cfg.clamped_max_secondary())
        {
            let index = pills.len();
            pills.push(map_pill(index, false, secondary));
        }
        pills
    }

    pub(in crate::panel) fn build_turn_suggestion_context(
        &self,
    ) -> Option<SuggestedRepliesContext> {
        let mut blocks = Vec::new();

        for item in self.items.iter() {
            match item {
                Item::User(text) if prompt::is_pinnable_prompt(item) => {
                    let clean = super::voice::tag::strip(text).0.trim().to_string();
                    if !clean.is_empty() {
                        blocks.push(format!("[user]: {clean}"));
                    }
                }
                Item::Assistant(text) => {
                    let clean = text.trim().to_string();
                    if !clean.is_empty() {
                        blocks.push(format!("[assistant]: {clean}"));
                    }
                }
                Item::Tool {
                    name,
                    input,
                    done: true,
                    error: None,
                    ..
                } if name == "user_response" => {
                    if let Some(parsed) = user_response::parse(name, input) {
                        let clean = parsed.content.trim().to_string();
                        if !clean.is_empty() {
                            blocks.push(format!("[assistant]: {clean}"));
                        }
                    }
                }
                _ => {}
            }
        }

        if blocks.is_empty() {
            return None;
        }
        if blocks.len() > 6 {
            let drain_count = blocks.len() - 6;
            blocks.drain(0..drain_count);
        }
        let transcript_tail = blocks.join("\n");

        let pending_todos = self
            .latest_todo_payload()
            .map(|payload| {
                payload
                    .todos
                    .into_iter()
                    .filter(|todo| {
                        !matches!(
                            todo.status.to_ascii_lowercase().as_str(),
                            "completed" | "cancelled" | "canceled"
                        )
                    })
                    .map(|todo| todo.content)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Some(
            SuggestedRepliesContext::from_transcript(&transcript_tail)
                .with_pending_todos(pending_todos),
        )
    }

    pub(in crate::panel) fn refresh_suggested_replies(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions_allowed() {
            return;
        }
        let Some(ctx_data) = self.build_turn_suggestion_context() else {
            return;
        };
        let key = (self.session_id.clone(), self.items.revision());
        if self.suggested_replies.evaluated_key.as_ref() == Some(&key)
            && self.suggested_replies.output.is_some()
        {
            return;
        }
        if let Some(cached) = self.suggested_replies.cache.get(&key).cloned() {
            self.apply_suggested_replies_output(key, cached, cx);
            return;
        }
        if self.suggested_replies.in_flight_key.as_ref() == Some(&key) {
            return;
        }

        let cfg = self.suggested_replies_config();
        // Fast deterministic evaluation in test mode or when local fallback is requested,
        // avoiding external network calls in headless UI tests while keeping full async
        // SystemOne/Laya/OpenGF cascade in live desktop sessions.
        let force_deterministic = cfg!(test)
            || crate::harness::screenshot_mode()
            || cfg
                .jev_provider
                .trim()
                .eq_ignore_ascii_case("deterministic")
            || cfg.jev_provider.trim().eq_ignore_ascii_case("local");

        if force_deterministic {
            let output = evaluate_deterministic(&ctx_data, &cfg);
            self.apply_suggested_replies_output(key, output, cx);
            return;
        }

        self.suggested_replies.in_flight_key = Some(key.clone());
        cx.spawn(async move |this, cx| {
            let output = cx
                .background_executor()
                .spawn(async move {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt.block_on(evaluate_cascade_with_context(&ctx_data, &cfg, None)),
                        Err(_) => evaluate_deterministic(&ctx_data, &cfg),
                    }
                })
                .await;
            let _ = this.update(cx, |panel, cx| {
                if panel.suggested_replies.in_flight_key.as_ref() != Some(&key) {
                    return;
                }
                panel.suggested_replies.in_flight_key = None;
                if panel.session_id != key.0
                    || panel.items.revision() != key.1
                    || !panel.suggestions_allowed()
                {
                    return;
                }
                panel.apply_suggested_replies_output(key, output, cx);
            });
        })
        .detach();
    }

    pub(in crate::panel) fn apply_suggested_replies_output(
        &mut self,
        key: (String, u64),
        output: SuggestedRepliesResponse,
        cx: &mut Context<Self>,
    ) {
        let prev_focused_text = self.suggested_replies.focused_index.and_then(|idx| {
            self.displayed_suggestion_pills()
                .get(idx)
                .map(|p| p.text.clone())
        });

        if self.suggested_replies.cache.len() >= MAX_SUGGESTION_CACHE_ENTRIES {
            self.suggested_replies.cache.clear();
        }
        self.suggested_replies
            .cache
            .insert(key.clone(), output.clone());
        self.suggested_replies.evaluated_key = Some(key);
        self.suggested_replies.output = Some(output);

        if let Some(prev_text) = prev_focused_text {
            let pills = self.displayed_suggestion_pills();
            self.suggested_replies.focused_index =
                pills.iter().position(|pill| pill.text == prev_text);
        } else {
            self.suggested_replies.focused_index = None;
        }
        cx.notify();
    }

    pub(in crate::panel) fn stage_suggested_reply(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let pills = self.displayed_suggestion_pills();
        let Some(pill) = pills.get(index) else {
            return false;
        };
        let text = pill.text.clone();
        self.suggested_replies.focused_index = Some(index);
        self.suggested_replies.staged_text = Some(text.clone());
        let focus = self.input.read(cx).focus_handle.clone();
        self.input.update(cx, |input, cx| {
            input.set_content(text, cx);
        });
        window.focus(&focus, cx);
        cx.notify();
        true
    }

    pub(in crate::panel) fn handle_suggested_reply_pill_click(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pills = self.displayed_suggestion_pills();
        let Some(pill) = pills.get(index) else {
            return;
        };
        let text = pill.text.clone();
        let current_input = self.input.read(cx).content.to_string();
        let already_staged = self.suggested_replies.focused_index == Some(index)
            && self.suggested_replies.staged_text.as_deref() == Some(text.as_str())
            && current_input.trim() == text.trim();

        if already_staged {
            self.suggested_replies.clear_active();
            let focus = self.input.read(cx).focus_handle.clone();
            self.input.update(cx, |input, cx| {
                input.clear_after_external_submit(&text, cx);
            });
            window.focus(&focus, cx);
            if !self.handle_slash_command(&text, cx) {
                self.submit_or_queue(text, Vec::new(), false, cx);
            }
            cx.notify();
        } else {
            self.stage_suggested_reply(index, window, cx);
        }
    }

    pub(in crate::panel) fn decide_suggested_reply_tab(
        &mut self,
        current_input: &str,
        cx: &mut Context<Self>,
    ) -> crate::input::TabOutcome {
        if !self.suggestions_allowed() {
            return crate::input::TabOutcome::Ignored;
        }
        let pills = self.displayed_suggestion_pills();
        if pills.is_empty() {
            return crate::input::TabOutcome::Ignored;
        }
        let is_staged = self
            .suggested_replies
            .staged_text
            .as_deref()
            .is_some_and(|staged| staged == current_input);

        // Never hijack Tab if the user typed manual text in the composer.
        if !current_input.is_empty() && !is_staged {
            return crate::input::TabOutcome::Ignored;
        }

        if is_staged {
            self.suggested_replies.clear_active();
            cx.notify();
            return crate::input::TabOutcome::Submit;
        }

        let target_index = self.suggested_replies.focused_index.unwrap_or(0);
        let Some(pill) = pills.get(target_index) else {
            return crate::input::TabOutcome::Ignored;
        };
        let text = pill.text.clone();
        self.suggested_replies.focused_index = Some(target_index);
        self.suggested_replies.staged_text = Some(text.clone());
        cx.notify();
        crate::input::TabOutcome::Stage(text)
    }

    pub(in crate::panel) fn render_suggested_replies(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.suggestions_allowed() {
            return None;
        }
        let pills = self.displayed_suggestion_pills();
        if pills.is_empty() {
            return None;
        }
        let cfg = self.suggested_replies_config();
        let theme = Theme::global();
        let focused_index = self.suggested_replies.focused_index;
        let current_input = self.input.read(cx).content.to_string();

        let mut children = Vec::with_capacity(pills.len());
        let mut secondary_number = 0usize;

        for pill in pills {
            let index = pill.index;
            let is_focused = focused_index == Some(index)
                && self
                    .suggested_replies
                    .staged_text
                    .as_deref()
                    .is_none_or(|staged| staged == current_input);
            let short_text = truncate_pill_text(&pill.text, MAX_PILL_LABEL_CHARS);
            let confidence_suffix = cfg
                .show_confidence
                .then(|| format!("({:.0}%)", (pill.score.clamp(0.0, 1.0) * 100.0).round()));

            if pill.is_default_action {
                let key_hint = if is_focused { "Enter ↵" } else { "Tab" };
                let element = div()
                    .id("suggested-reply-default")
                    .debug_selector(|| "suggested-reply-default".into())
                    .h(px(TAB_HEIGHT))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_full()
                    .bg(if is_focused {
                        theme.ACCENT
                    } else {
                        theme.ACCENT_DIM
                    })
                    .text_color(if is_focused {
                        theme.PANEL_BG
                    } else {
                        theme.ACCENT
                    })
                    .text_size(px(10.5))
                    .font_family(theme.FONT_MONO)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("[{key_hint}]")),
                    )
                    .child(div().flex_none().child(pill.badge))
                    .child(
                        div()
                            .min_w_0()
                            .max_w(px(360.))
                            .truncate()
                            .font_family(theme.FONT_UI)
                            .child(short_text),
                    )
                    .children(
                        confidence_suffix
                            .map(|pct| div().flex_none().text_size(px(10.0)).child(pct)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.handle_suggested_reply_pill_click(index, window, cx);
                        cx.stop_propagation();
                    }))
                    .into_any_element();
                children.push(element);
            } else {
                secondary_number += 1;
                let number_label = format!("{secondary_number}.");
                let selector_id: &'static str = match secondary_number {
                    1 => "suggested-reply-1",
                    2 => "suggested-reply-2",
                    3 => "suggested-reply-3",
                    4 => "suggested-reply-4",
                    5 => "suggested-reply-5",
                    _ => "suggested-reply-secondary",
                };
                let element = composer_pill(selector_id)
                    .id(("suggested-reply-pill", index))
                    .when(is_focused, |el| el.bg(theme.USER_BG).text_color(theme.TEXT))
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.TEXT_FAINT)
                            .child(number_label),
                    )
                    .child(div().flex_none().text_color(theme.ACCENT).child(pill.badge))
                    .child(
                        div()
                            .min_w_0()
                            .max_w(px(280.))
                            .truncate()
                            .font_family(theme.FONT_UI)
                            .child(short_text),
                    )
                    .children(confidence_suffix.map(|pct| {
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(theme.TEXT_FAINT)
                            .child(pct)
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.handle_suggested_reply_pill_click(index, window, cx);
                        cx.stop_propagation();
                    }))
                    .into_any_element();
                children.push(element);
            }
        }

        Some(
            div()
                .debug_selector(|| "composer-suggested-replies".into())
                .w_full()
                .min_w_0()
                .px_1()
                .mb(px(4.0))
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .children(children)
                .into_any_element(),
        )
    }

    pub(in crate::panel) fn handle_suggestions_command(
        &mut self,
        args: &str,
        cx: &mut Context<Self>,
    ) {
        let sub = args.trim().to_ascii_lowercase();
        let cfg = self.suggested_replies_config();
        match sub.as_str() {
            "on" | "enable" => {
                self.suggested_replies.enabled_override = Some(true);
                self.refresh_suggested_replies(cx);
                self.items.push(Item::Assistant(
                    "Suggested Replies enabled for this session.".into(),
                ));
            }
            "off" | "disable" => {
                self.suggested_replies.enabled_override = Some(false);
                self.clear_suggested_replies();
                self.items.push(Item::Assistant(
                    "Suggested Replies disabled for this session.".into(),
                ));
            }
            "toggle" => {
                let next = !self.suggested_replies.is_enabled(&cfg);
                self.suggested_replies.enabled_override = Some(next);
                if next {
                    self.refresh_suggested_replies(cx);
                    self.items.push(Item::Assistant(
                        "Suggested Replies enabled for this session.".into(),
                    ));
                } else {
                    self.clear_suggested_replies();
                    self.items.push(Item::Assistant(
                        "Suggested Replies disabled for this session.".into(),
                    ));
                }
            }
            "refresh" | "test" => {
                self.suggested_replies.enabled_override = Some(true);
                self.suggested_replies.evaluated_key = None;
                self.suggested_replies.cache.clear();
                self.refresh_suggested_replies(cx);
                let count = self.displayed_suggestion_pills().len();
                self.items.push(Item::Assistant(format!(
                    "Refreshed Suggested Replies ({count} suggestion pill{} ready).",
                    if count == 1 { "" } else { "s" }
                )));
            }
            "" | "status" | "debug" => {
                let enabled = self.suggested_replies.is_enabled(&cfg);
                let pills = self.displayed_suggestion_pills();
                let (branch, route) = self
                    .suggested_replies
                    .output
                    .as_ref()
                    .map(|out| {
                        let source = out
                            .debug
                            .as_ref()
                            .map(|d| d.source.as_str())
                            .unwrap_or("local");
                        (out.branch.as_str(), source)
                    })
                    .unwrap_or(("none", "none"));
                let mut lines = vec![format!(
                    "**Suggested Replies**: `{}` (provider: `{}`, source: `{}`, branch: `{}`, threshold: `{:.0}%`)",
                    if enabled { "on" } else { "off" },
                    cfg.jev_provider,
                    route,
                    branch,
                    cfg.confidence_threshold * 100.0
                )];
                if pills.is_empty() {
                    lines.push("No active suggestion pills for the current turn.".into());
                } else {
                    for pill in pills {
                        let role = if pill.is_default_action {
                            "Default Action [Tab]"
                        } else {
                            "Secondary"
                        };
                        lines.push(format!(
                            "- **{role}** `{}` {} ({:.0}%)",
                            pill.badge,
                            pill.text,
                            pill.score * 100.0
                        ));
                    }
                }
                self.items.push(Item::Assistant(lines.join("\n")));
            }
            _ => {
                self.items.push(Item::Error(
                    "Usage: `/suggestions [status|debug|refresh|on|off|toggle]`.".into(),
                ));
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
#[path = "panel_suggested_replies_tests.rs"]
mod tests;
