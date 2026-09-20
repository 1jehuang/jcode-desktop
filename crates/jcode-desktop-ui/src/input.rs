//! Prompt input with full IME support, adapted from gpui's input example.
//! Long prompts soft-wrap in a bounded, scrollable editor. Enter submits via a callback.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Style,
    StyledImage, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill,
    img, point, prelude::*, px, relative, size,
};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

#[path = "input_model_menu.rs"]
mod model_menu;
#[path = "input_paste_preview.rs"]
mod paste_preview;

use crate::commands::registered_command_entries;
use crate::theme::{Theme, to_hsla};

const PENDING_COMMAND_NOTICE: &str =
    "Commands are available once connected. Your command is still in the editor.";

actions!(
    prompt_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordBack,
        DeleteWordForward,
        KillToStart,
        KillToEnd,
        Undo,
        Redo,
        HistoryPrev,
        HistoryNext,
        Clear,
        Submit,
        Queue,
    ]
);

pub fn bind_keys(cx: &mut App) {
    crate::text_selection::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("PromptInput")),
        KeyBinding::new("delete", Delete, Some("PromptInput")),
        KeyBinding::new("left", Left, Some("PromptInput")),
        KeyBinding::new("right", Right, Some("PromptInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("PromptInput")),
        KeyBinding::new("shift-right", SelectRight, Some("PromptInput")),
        KeyBinding::new("ctrl-left", MoveWordLeft, Some("PromptInput")),
        KeyBinding::new("ctrl-right", MoveWordRight, Some("PromptInput")),
        KeyBinding::new("alt-left", MoveWordLeft, Some("PromptInput")),
        KeyBinding::new("alt-right", MoveWordRight, Some("PromptInput")),
        KeyBinding::new("alt-b", MoveWordLeft, Some("PromptInput")),
        KeyBinding::new("alt-f", MoveWordRight, Some("PromptInput")),
        KeyBinding::new("ctrl-b", MoveWordLeft, Some("PromptInput")),
        KeyBinding::new("ctrl-f", MoveWordRight, Some("PromptInput")),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, Some("PromptInput")),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, Some("PromptInput")),
        KeyBinding::new("alt-shift-left", SelectWordLeft, Some("PromptInput")),
        KeyBinding::new("alt-shift-right", SelectWordRight, Some("PromptInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("PromptInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("PromptInput")),
        KeyBinding::new("super-a", SelectAll, Some("PromptInput")),
        KeyBinding::new("ctrl-shift-a", SelectAll, Some("PromptInput")),
        KeyBinding::new("ctrl-v", Paste, Some("PromptInput")),
        KeyBinding::new("cmd-v", Paste, Some("PromptInput")),
        KeyBinding::new("super-v", Paste, Some("PromptInput")),
        KeyBinding::new("alt-v", Paste, Some("PromptInput")),
        KeyBinding::new("ctrl-c", Copy, Some("PromptInput")),
        KeyBinding::new("cmd-c", Copy, Some("PromptInput")),
        KeyBinding::new("super-c", Copy, Some("PromptInput")),
        KeyBinding::new("ctrl-shift-c", Copy, Some("PromptInput")),
        KeyBinding::new("ctrl-x", Cut, Some("PromptInput")),
        KeyBinding::new("cmd-x", Cut, Some("PromptInput")),
        KeyBinding::new("ctrl-z", Undo, Some("PromptInput")),
        KeyBinding::new("cmd-z", Undo, Some("PromptInput")),
        KeyBinding::new("super-z", Undo, Some("PromptInput")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("PromptInput")),
        KeyBinding::new("cmd-shift-z", Redo, Some("PromptInput")),
        KeyBinding::new("ctrl-y", Redo, Some("PromptInput")),
        KeyBinding::new("ctrl-w", DeleteWordBack, Some("PromptInput")),
        KeyBinding::new("ctrl-backspace", DeleteWordBack, Some("PromptInput")),
        KeyBinding::new("alt-backspace", DeleteWordBack, Some("PromptInput")),
        KeyBinding::new("alt-delete", DeleteWordBack, Some("PromptInput")),
        KeyBinding::new("cmd-backspace", DeleteWordBack, Some("PromptInput")),
        KeyBinding::new("ctrl-delete", DeleteWordForward, Some("PromptInput")),
        KeyBinding::new("alt-d", DeleteWordForward, Some("PromptInput")),
        KeyBinding::new("ctrl-u", KillToStart, Some("PromptInput")),
        KeyBinding::new("ctrl-e", End, Some("PromptInput")),
        KeyBinding::new("ctrl-k", HistoryPrev, Some("PromptInput")),
        KeyBinding::new("ctrl-j", HistoryNext, Some("PromptInput")),
        KeyBinding::new("ctrl-[", HistoryPrev, Some("PromptInput")),
        KeyBinding::new("ctrl-]", HistoryNext, Some("PromptInput")),
        KeyBinding::new("up", HistoryPrev, Some("PromptInput")),
        KeyBinding::new("down", HistoryNext, Some("PromptInput")),
        KeyBinding::new("escape", Clear, Some("PromptInput")),
        KeyBinding::new("home", Home, Some("PromptInput")),
        KeyBinding::new("end", End, Some("PromptInput")),
        KeyBinding::new("ctrl-home", Home, Some("PromptInput")),
        KeyBinding::new("ctrl-end", End, Some("PromptInput")),
        KeyBinding::new("enter", Submit, Some("PromptInput")),
        KeyBinding::new("ctrl-enter", Queue, Some("PromptInput")),
    ]);
}

pub struct PromptInput {
    pub focus_handle: FocusHandle,
    pub content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<WrappedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    visual_line_count: usize,
    is_selecting: bool,
    undo: Vec<String>,
    redo: Vec<String>,
    history: Vec<String>,
    history_index: Option<usize>,
    live_draft: String,
    attachments: Vec<Attachment>,
    attachment_notice: Option<SharedString>,
    /// The newest paste briefly appears at reading size, then flies into its
    /// thumbnail. The index keeps simultaneous attachments independent.
    attachment_preview: Option<paste_preview::Preview>,
    preview_panel_bounds: paste_preview::MeasuredBounds,
    on_submit: Box<dyn Fn(String, Vec<(String, String)>, bool, &mut Window, &mut App)>,
    on_change: Option<Box<dyn Fn(&str, &mut App)>>,
    on_overlay_cancel: Option<Box<dyn Fn(&mut App) -> bool>>,
    command_models: Vec<String>,
    command_completion: bool,
    command_selection: usize,
    model_logo_providers: HashMap<String, String>,
    model_details: HashMap<String, model_menu::ModelDetails>,
    expanded_model_groups: HashSet<String>,
    current_model: Option<String>,
    command_scroll: gpui::ScrollHandle,
    command_layout_key: Option<(SharedString, usize, usize, gpui::Size<Pixels>)>,
    command_entrance: jcode_desktop_motion::MenuEntrance,
    editor_scroll: gpui::ScrollHandle,
    revealed_caret: Option<(usize, SharedString, gpui::Size<Pixels>)>,
    submission_enabled: bool,
    pending_session: bool,
    spacious: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommandSuggestion {
    value: String,
    help: String,
    detail: Option<String>,
    header: Option<String>,
    toggle: Option<String>,
}

fn command_suggestions(input: &str, models: &[String]) -> Vec<CommandSuggestion> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') || trimmed.contains('\n') {
        return Vec::new();
    }
    // Keep the command in the composer, so typing `/models` remains possible.
    // Model rows replace the command rows in the same suggestion area.
    if trimmed == "/model" || trimmed.starts_with("/model ") {
        let query = trimmed
            .split_once(' ')
            .map(|(_, query)| query.trim().to_ascii_lowercase())
            .unwrap_or_default();
        return models
            .iter()
            .filter(|model| query.is_empty() || model.to_ascii_lowercase().contains(&query))
            .map(|model| CommandSuggestion {
                value: format!("/model {model}"),
                help: "Switch this session".into(),
                detail: None,
                header: None,
                toggle: None,
            })
            .collect();
    }
    if trimmed == "/effort" || trimmed.starts_with("/effort ") {
        let query = trimmed
            .split_once(' ')
            .map(|(_, query)| query.trim().to_ascii_lowercase())
            .unwrap_or_default();
        return ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .filter(|effort| query.is_empty() || effort.contains(&query))
            .map(|effort| CommandSuggestion {
                value: format!("/effort {effort}"),
                help: "Set reasoning effort".into(),
                detail: None,
                header: None,
                toggle: None,
            })
            .collect();
    }
    let query = trimmed.to_ascii_lowercase();
    let mut commands = registered_command_entries()
        .filter_map(|(command, help)| {
            let command_lower = command.to_ascii_lowercase();
            let help_lower = help.to_ascii_lowercase();
            (command_lower.starts_with(&query)
                || command_lower.contains(query.trim_start_matches('/'))
                || help_lower.contains(query.trim_start_matches('/')))
            .then_some((command, help, command_lower.starts_with(&query)))
        })
        .collect::<Vec<_>>();
    commands.sort_by_key(|(_, _, prefix)| !prefix);
    commands
        .into_iter()
        .map(|(value, help, _)| CommandSuggestion {
            value: value.into(),
            help: help.into(),
            detail: None,
            header: None,
            toggle: None,
        })
        .collect()
}

fn accepted_command_submission(
    suggestions: &[CommandSuggestion],
    selection: usize,
) -> Option<String> {
    let suggestion = suggestions.get(selection.min(suggestions.len().saturating_sub(1)))?;
    // Matching includes descriptions, not only command-name prefixes. Enter
    // must select the highlighted result for either kind of search.
    suggestion
        .toggle
        .is_none()
        .then(|| suggestion.value.clone())
}

#[derive(Clone)]
struct Attachment {
    media_type: String,
    encoded: String,
    label: SharedString,
    preview: Arc<gpui::Image>,
    bounds: paste_preview::MeasuredBounds,
}

fn preview_image(media_type: &str, bytes: Vec<u8>) -> Option<Arc<gpui::Image>> {
    Some(crate::image_cache::encoded(
        gpui::ImageFormat::from_mime_type(media_type)?,
        bytes,
    ))
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PromptInputSnapshot {
    pub content: String,
    pub selection_start: usize,
    pub selection_end: usize,
    pub selection_reversed: bool,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
    pub live_draft: String,
    pub attachments: Vec<AttachmentSnapshot>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AttachmentSnapshot {
    pub media_type: String,
    pub encoded: String,
    pub label: String,
}

impl PromptInput {
    pub fn snapshot(&self) -> PromptInputSnapshot {
        PromptInputSnapshot {
            content: self.content.to_string(),
            selection_start: self.selected_range.start,
            selection_end: self.selected_range.end,
            selection_reversed: self.selection_reversed,
            history: self.history.clone(),
            history_index: self.history_index,
            live_draft: self.live_draft.clone(),
            attachments: self
                .attachments
                .iter()
                .map(|attachment| AttachmentSnapshot {
                    media_type: attachment.media_type.clone(),
                    encoded: attachment.encoded.clone(),
                    label: attachment.label.to_string(),
                })
                .collect(),
        }
    }

    pub fn restore(&mut self, snapshot: PromptInputSnapshot, cx: &mut Context<Self>) {
        let len = snapshot.content.len();
        self.content = snapshot.content.into();
        self.selected_range = snapshot.selection_start.min(len)..snapshot.selection_end.min(len);
        if self.selected_range.end < self.selected_range.start {
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.selection_reversed = snapshot.selection_reversed;
        self.history = snapshot.history;
        self.history_index = snapshot
            .history_index
            .filter(|index| *index < self.history.len());
        self.live_draft = snapshot.live_draft;
        self.attachments = snapshot
            .attachments
            .into_iter()
            .filter_map(|attachment| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&attachment.encoded)
                    .ok()?;
                let preview = preview_image(&attachment.media_type, bytes)?;
                Some(Attachment {
                    media_type: attachment.media_type,
                    encoded: attachment.encoded,
                    label: attachment.label.into(),
                    preview,
                    bounds: Default::default(),
                })
            })
            .collect();
        self.attachment_preview = None;
        self.attachment_notice = match self.attachments.len() {
            0 => None,
            1 => Some("1 image attached".into()),
            count => Some(format!("{count} images attached").into()),
        };
        self.marked_range = None;
        self.undo.clear();
        self.redo.clear();
        cx.notify();
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.attachments.len() {
            return;
        }
        self.attachments.remove(index);
        self.attachment_preview = None;
        self.attachment_notice = match self.attachments.len() {
            0 => None,
            1 => Some("1 image attached".into()),
            count => Some(format!("{count} images attached").into()),
        };
        cx.notify();
    }

    pub fn new(
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        on_submit: impl Fn(String, Vec<(String, String)>, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self::new_with_queue(cx, placeholder, move |text, images, _, window, cx| {
            on_submit(text, images, window, cx)
        })
    }

    pub(crate) fn new_with_queue(
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        on_submit: impl Fn(String, Vec<(String, String)>, bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            visual_line_count: 1,
            is_selecting: false,
            undo: Vec::new(),
            redo: Vec::new(),
            history: Vec::new(),
            history_index: None,
            live_draft: String::new(),
            attachments: Vec::new(),
            attachment_notice: None,
            attachment_preview: None,
            preview_panel_bounds: Default::default(),
            on_submit: Box::new(on_submit),
            on_change: None,
            on_overlay_cancel: None,
            command_models: Vec::new(),
            command_completion: true,
            command_selection: 0,
            model_logo_providers: HashMap::new(),
            model_details: HashMap::new(),
            expanded_model_groups: HashSet::new(),
            current_model: None,
            command_scroll: gpui::ScrollHandle::new(),
            command_layout_key: None,
            command_entrance: jcode_desktop_motion::MenuEntrance::default(),
            editor_scroll: gpui::ScrollHandle::new(),
            revealed_caret: None,
            submission_enabled: true,
            pending_session: false,
            spacious: false,
        }
    }

    pub(crate) fn set_spacious(&mut self, spacious: bool, cx: &mut Context<Self>) {
        if self.spacious != spacious {
            self.spacious = spacious;
            cx.notify();
        }
    }

    /// Plain metadata editors should not interpret titles as slash commands.
    pub(crate) fn without_command_completion(mut self) -> Self {
        self.command_completion = false;
        self
    }

    pub fn with_on_change(mut self, on_change: impl Fn(&str, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(on_change));
        self
    }

    /// Explicitly disable submission without disabling editing or focus.
    /// Startup panels instead queue prompts and separately gate slash commands.
    pub fn set_submission_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.submission_enabled = enabled;
        cx.notify();
    }

    pub(crate) fn set_pending_session(&mut self, pending: bool, cx: &mut Context<Self>) {
        self.pending_session = pending;
        if !pending && self.attachment_notice.as_deref() == Some(PENDING_COMMAND_NOTICE) {
            self.attachment_notice = None;
        }
        cx.notify();
    }

    pub fn with_on_overlay_cancel(mut self, cancel: impl Fn(&mut App) -> bool + 'static) -> Self {
        self.on_overlay_cancel = Some(Box::new(cancel));
        self
    }

    pub fn set_command_models(&mut self, models: Vec<String>, cx: &mut Context<Self>) {
        if self.command_models != models {
            self.command_models = models;
            self.command_selection = 0;
            cx.notify();
        }
    }

    pub fn set_model_routes(
        &mut self,
        mut models: Vec<String>,
        routes: &[jcode_sdk::ModelRouteInfo],
        current_model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .command_suggestions()
            .get(self.command_selection)
            .cloned();
        self.model_details = model_menu::from_routes(routes);
        self.current_model = current_model;
        if !routes.is_empty() {
            models = self.model_details.keys().cloned().collect();
        }
        model_menu::rank(&mut models, &self.model_details);
        self.set_command_models(models, cx);
        if let Some(model) = selected
            .as_ref()
            .and_then(|row| row.value.strip_prefix("/model "))
        {
            if !self
                .command_suggestions()
                .iter()
                .any(|row| row.value == format!("/model {model}"))
                && self
                    .command_models
                    .iter()
                    .any(|candidate| candidate == model)
            {
                self.expanded_model_groups
                    .insert(model_menu::group_key(model, &self.model_details));
            }
        }
        // A background usage update must not move Enter onto a different model.
        if let Some(index) = selected.and_then(|selected| {
            self.command_suggestions().iter().position(|row| {
                if selected.toggle.is_some() {
                    row.toggle == selected.toggle
                } else {
                    row.toggle.is_none() && row.value == selected.value
                }
            })
        }) {
            self.command_selection = index;
            self.command_scroll.scroll_to_item(index);
        }
        cx.notify();
    }

    pub fn set_current_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if self.current_model != model {
            self.current_model = model;
            cx.notify();
        }
    }

    pub fn set_model_logo_providers(
        &mut self,
        providers: HashMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if self.model_logo_providers != providers {
            self.model_logo_providers = providers;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn model_picker_rows(&self) -> Vec<(String, bool)> {
        self.command_suggestions()
            .into_iter()
            .enumerate()
            .filter_map(|(index, suggestion)| {
                suggestion.value.strip_prefix("/model ").map(|model| {
                    (
                        self.model_details
                            .get(model)
                            .map_or_else(|| model.to_string(), |detail| detail.model.clone()),
                        index == self.command_selection,
                    )
                })
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn command_models(&self) -> &[String] {
        &self.command_models
    }

    fn command_suggestions(&self) -> Vec<CommandSuggestion> {
        if !self.command_completion {
            return Vec::new();
        }
        let now = model_menu::now_unix_secs();
        let trimmed = self.content.trim_start();
        let suggestions =
            if !trimmed.contains('\n') && (trimmed == "/model" || trimmed.starts_with("/model ")) {
                let query = trimmed.strip_prefix("/model").unwrap_or_default();
                model_menu::grouped_rows(
                    &self.command_models,
                    &self.model_details,
                    self.current_model.as_deref(),
                    &self.expanded_model_groups,
                    query,
                )
                .into_iter()
                .map(|row| CommandSuggestion {
                    value: row.value,
                    help: String::new(),
                    detail: None,
                    header: row.header,
                    toggle: row.toggle,
                })
                .collect()
            } else {
                command_suggestions(&self.content, &self.command_models)
            };
        suggestions
            .into_iter()
            .map(|mut suggestion| {
                if let Some(model) = suggestion.value.strip_prefix("/model ") {
                    suggestion.detail = Some(
                        self.model_details
                            .get(model)
                            .map(|details| details.label(now))
                            .unwrap_or_else(|| model_menu::usage_label(None, now)),
                    );
                    suggestion.help = if model_menu::current_spec(
                        self.current_model.as_deref(),
                        &self.command_models,
                        &self.model_details,
                    ) == Some(model)
                    {
                        "Current".into()
                    } else {
                        String::new()
                    };
                }
                suggestion
            })
            .collect()
    }

    fn toggle_model_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded_model_groups.remove(&key) {
            self.expanded_model_groups.insert(key.clone());
        }
        // Keep focus on the disclosure when rows appear or disappear above it.
        if let Some(index) = self
            .command_suggestions()
            .iter()
            .position(|row| row.toggle.as_ref() == Some(&key))
        {
            self.command_selection = index;
            self.command_scroll.scroll_to_item(index);
        }
        cx.notify();
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_prompt(false, window, cx);
    }

    fn queue(&mut self, _: &Queue, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_prompt(true, window, cx);
    }

    fn submit_prompt(&mut self, queued: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.submission_enabled {
            return;
        }
        if self.pending_session
            && self.command_completion
            && self.content.trim_start().starts_with('/')
        {
            self.attachment_notice = Some(PENDING_COMMAND_NOTICE.into());
            cx.notify();
            return;
        }
        let raw_content = self.content.to_string();
        let mut content = raw_content.trim().to_string();
        if content.is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.command_completion && self.attachments.is_empty() {
            let suggestions = self.command_suggestions();
            if let Some(key) = suggestions
                .get(self.command_selection)
                .and_then(|row| row.toggle.clone())
            {
                self.toggle_model_group(key, cx);
                return;
            }
            if (raw_content.trim_start() == "/model"
                || raw_content.trim_start().starts_with("/model "))
                && suggestions.is_empty()
            {
                return;
            }
            if let Some(accepted) =
                accepted_command_submission(&suggestions, self.command_selection)
            {
                content = accepted;
            }
        }
        let content = if content.is_empty() {
            "[image]".to_string()
        } else {
            content
        };
        let images = std::mem::take(&mut self.attachments)
            .into_iter()
            .map(|image| (image.media_type, image.encoded))
            .collect();
        self.attachment_notice = None;
        self.attachment_preview = None;
        self.content = "".into();
        self.expanded_model_groups.clear();
        // The cleared editor is one line immediately. Waiting for its next
        // paint leaves a tall, empty composer after a wrapped prompt submits.
        self.visual_line_count = 1;
        self.selected_range = 0..0;
        self.marked_range = None;
        self.history.push(content.clone());
        self.history_index = None;
        self.live_draft.clear();
        self.command_selection = 0;
        (self.on_submit)(content, images, queued, window, cx);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn attach_image(
        &mut self,
        image: crate::clipboard_image::ClipboardImage,
        cx: &mut Context<Self>,
    ) {
        let label = image.label();
        let Some(preview) = preview_image(&image.media_type, image.bytes.clone()) else {
            self.attachment_notice = Some("could not preview pasted image".into());
            cx.notify();
            return;
        };
        self.attachments.push(Attachment {
            media_type: image.media_type,
            encoded: base64::engine::general_purpose::STANDARD.encode(image.bytes),
            label: label.clone().into(),
            preview,
            bounds: Default::default(),
        });
        self.attachment_preview = Some(paste_preview::Preview::new(self.attachments.len() - 1));
        self.attachment_notice = Some(match self.attachments.len() {
            1 => format!("image attached ({label})").into(),
            count => format!("{count} images attached").into(),
        });
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        match crate::clipboard_image::read() {
            Ok(Some(image)) => {
                self.attach_image(image, cx);
                return;
            }
            Ok(None) => {}
            Err(error) => {
                self.attachment_notice = Some(format!("image paste unavailable: {error}").into());
            }
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn delete_word_back(
        &mut self,
        _: &DeleteWordBack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn kill_to_start(&mut self, _: &KillToStart, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
        self.replace_text_in_range(None, "", window, cx);
    }

    fn kill_to_end(&mut self, _: &KillToEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
        self.replace_text_in_range(None, "", window, cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(self.content.to_string());
            self.set_content(previous, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.content.to_string());
            self.set_content(next, cx);
        }
    }

    fn history_prev(&mut self, _: &HistoryPrev, _: &mut Window, cx: &mut Context<Self>) {
        let suggestions = self.command_suggestions();
        if !suggestions.is_empty() {
            self.command_selection = self.command_selection.saturating_sub(1);
            self.command_scroll.scroll_to_item(self.command_selection);
            cx.notify();
            return;
        }
        if self.history.is_empty() {
            return;
        }
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.live_draft = self.content.to_string();
                self.history.len() - 1
            }
        };
        self.history_index = Some(index);
        self.set_content(self.history[index].clone(), cx);
    }

    fn history_next(&mut self, _: &HistoryNext, _: &mut Window, cx: &mut Context<Self>) {
        let suggestions = self.command_suggestions();
        if !suggestions.is_empty() {
            self.command_selection = (self.command_selection + 1).min(suggestions.len() - 1);
            self.command_scroll.scroll_to_item(self.command_selection);
            cx.notify();
            return;
        }
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.history_index = Some(index + 1);
            self.set_content(self.history[index + 1].clone(), cx);
        } else {
            self.history_index = None;
            self.set_content(self.live_draft.clone(), cx);
        }
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.on_overlay_cancel {
            if cancel(cx) {
                return;
            }
        }
        if !self.content.is_empty() {
            self.set_content(String::new(), cx);
        }
    }

    /// Append asynchronous dictation without replacing newer edits or attachments.
    pub(crate) fn append_dictation(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.undo.push(self.content.to_string());
        self.redo.clear();
        let mut draft = self.content.to_string();
        if !draft.is_empty() && !draft.ends_with(char::is_whitespace) {
            draft.push(' ');
        }
        draft.push_str(text);
        self.marked_range = None;
        self.history_index = None;
        self.set_content(draft, cx);
        // Dictation completes inside a parent Panel update. Run its callback
        // after that lease ends, just as a later native edit would.
        let input = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = input.update(cx, |input, cx| {
                if let Some(on_change) = &input.on_change {
                    on_change(&input.content, cx);
                }
            });
        });
    }

    fn reset_model_groups_after_exit(&mut self) {
        let content = self.content.trim_start();
        if content.contains('\n') || !(content == "/model" || content.starts_with("/model ")) {
            self.expanded_model_groups.clear();
        }
    }

    pub(crate) fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content.into();
        self.reset_model_groups_after_exit();
        self.command_selection = 0;
        self.command_scroll.scroll_to_item(0);
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(
                self.index_for_mouse_position(event.position, window.line_height()),
                cx,
            );
        } else {
            self.move_to(
                self.index_for_mouse_position(event.position, window.line_height()),
                cx,
            )
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(
                self.index_for_mouse_position(event.position, window.line_height()),
                cx,
            );
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>, line_height: Pixels) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_position(position - bounds.origin, line_height)
            .unwrap_or_else(|index| index)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        previous_word_boundary(&self.content, offset)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        next_word_boundary(&self.content, offset)
    }
}

fn previous_word_boundary(text: &str, offset: usize) -> usize {
    let prefix = &text[..offset];
    let trimmed = prefix.trim_end_matches(char::is_whitespace);
    trimmed
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index + ch.len_utf8()))
        .unwrap_or(0)
}

fn next_word_boundary(text: &str, offset: usize) -> usize {
    let suffix = &text[offset..];
    let word_end = suffix
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(suffix.len());
    let rest = &suffix[word_end..];
    offset
        + word_end
        + rest
            .char_indices()
            .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
            .unwrap_or(rest.len())
}

impl EntityInputHandler for PromptInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.undo.push(self.content.to_string());
        self.redo.clear();
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.reset_model_groups_after_exit();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.command_selection = 0;
        self.command_scroll.scroll_to_item(0);
        if let Some(on_change) = &self.on_change {
            on_change(&self.content, cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.reset_model_groups_after_exit();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.command_selection = 0;
        self.command_scroll.scroll_to_item(0);
        if let Some(on_change) = &self.on_change {
            on_change(&self.content, cx);
        }

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = last_layout.position_for_index(range.start, window.line_height())?;
        let end = last_layout.position_for_index(range.end, window.line_height())?;
        Some(Bounds::from_corners(
            bounds.origin + start,
            point(
                bounds.left() + end.x,
                bounds.top() + end.y + window.line_height(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout
            .closest_index_for_position(local, _window.line_height())
            .unwrap_or_else(|index| index);
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<PromptInput>,
}

struct PrepaintState {
    line: Option<WrappedLine>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    text_bounds: Bounds<Pixels>,
    visual_line_count: usize,
}

fn selection_quads(
    line: &WrappedLine,
    range: Range<usize>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Vec<PaintQuad> {
    let Some(start) = line.position_for_index(range.start, line_height) else {
        return Vec::new();
    };
    let Some(end) = line.position_for_index(range.end, line_height) else {
        return Vec::new();
    };
    let first_row = (start.y / line_height) as usize;
    let last_row = (end.y / line_height) as usize;
    (first_row..=last_row)
        .map(|row| {
            let left = if row == first_row { start.x } else { px(0.) };
            let right = if row == last_row {
                end.x
            } else {
                bounds.size.width
            };
            fill(
                Bounds::new(
                    point(bounds.left() + left, bounds.top() + line_height * row),
                    size((right - left).max(px(0.)), line_height),
                ),
                to_hsla(Theme::global().SELECTION),
            )
        })
        .collect()
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let line_count = self.input.read(cx).visual_line_count.max(1);
        style.size.height = (window.line_height() * line_count).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), to_hsla(Theme::global().TEXT_DIM))
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_text(
                display_text,
                font_size,
                &runs,
                Some(bounds.size.width),
                None,
            )
            .expect("prompt text should shape")
            .into_iter()
            .next()
            .expect("shape_text always returns a line");
        let line_height = window.line_height();
        let cursor_pos = line
            .position_for_index(cursor, line_height)
            .unwrap_or_default();
        let visual_line_count = line.wrap_boundaries().len() + 1;
        let text_bounds = Bounds::new(
            bounds.origin,
            size(bounds.size.width, line_height * visual_line_count),
        );
        let (selection, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                Some(fill(
                    Bounds::new(text_bounds.origin + cursor_pos, size(px(2.), line_height)),
                    to_hsla(Theme::global().CURSOR),
                )),
            )
        } else {
            (
                selection_quads(&line, selected_range, text_bounds, line_height),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
            text_bounds,
            visual_line_count,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(
            prepaint.text_bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            // Reveal edits and keyboard navigation, but do not undo wheel scrolling
            // on every paint. Include viewport size so resizing reveals the caret.
            let viewport = input.editor_scroll.bounds();
            let caret_key = (input.cursor_offset(), input.content.clone(), viewport.size);
            if input.revealed_caret.as_ref() != Some(&caret_key) {
                if let Some(caret) =
                    line.position_for_index(input.cursor_offset(), window.line_height())
                {
                    let top = prepaint.text_bounds.top() + caret.y;
                    let bottom = top + window.line_height();
                    let adjustment = if top < viewport.top() {
                        viewport.top() - top
                    } else if bottom > viewport.bottom() {
                        viewport.bottom() - bottom
                    } else {
                        px(0.)
                    };
                    if adjustment != px(0.) {
                        let offset = input.editor_scroll.offset();
                        input
                            .editor_scroll
                            .set_offset(point(px(0.), (offset.y + adjustment).min(px(0.))));
                        let entity = _cx.entity();
                        _cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()));
                    }
                }
                input.revealed_caret = Some(caret_key);
            }
            input.last_layout = Some(line);
            input.last_bounds = Some(prepaint.text_bounds);
            if input.visual_line_count != prepaint.visual_line_count {
                input.visual_line_count = prepaint.visual_line_count;
                let entity = _cx.entity();
                _cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()));
            }
        });
    }
}

impl Render for PromptInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let editor_height = (f32::from(window.viewport_size().height) * 0.25).clamp(28., 160.);
        let spacious = self.spacious && window.viewport_size().height >= px(400.);
        let suggestions = self.command_suggestions();
        if self.command_selection >= suggestions.len() {
            self.command_selection = 0;
        }
        let command_visible = self.command_completion
            && (!suggestions.is_empty() || self.content.trim_start().starts_with("/model "));
        let menu_duration = jcode_desktop_motion::policy(
            crate::transition::Transition::Menu,
            crate::config::get().appearance.reduce_motion || cx.reduce_motion(),
        )
        .duration;
        let menu_progress =
            self.command_entrance
                .update(command_visible, Instant::now(), menu_duration);
        if self.command_entrance.is_animating() {
            window.request_animation_frame();
        }
        let command_selection = self.command_selection;
        let command_layout_key = (!suggestions.is_empty()).then(|| {
            (
                self.content.clone(),
                suggestions.len(),
                command_selection,
                window.viewport_size(),
            )
        });
        if self.command_layout_key != command_layout_key {
            self.command_layout_key = command_layout_key;
            // ScrollHandle geometry is updated during layout, after render.
            // One follow-up frame keeps the thumb correct without idle polling.
            let input = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = input.update(cx, |_, cx| cx.notify());
            });
        }
        let attachment_overlay = self.render_paste_preview(window);
        let preview_index = attachment_overlay.as_ref().and_then(|_| {
            self.attachment_preview
                .as_ref()
                .map(|preview| preview.index)
        });
        let attachments = self.attachments.iter().enumerate().map(|(index, image)| {
            div()
                .id(("attachment", index))
                .max_w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .rounded_md()
                .p_1()
                .bg(Theme::global().USER_BG)
                .text_size(px(11.0))
                .text_color(Theme::global().TEXT_DIM)
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .w(px(64.0))
                        .h(px(52.0))
                        .child(
                            img(crate::image_cache::source(image.preview.clone()))
                                .size_full()
                                .opacity(if preview_index == Some(index) {
                                    0.0
                                } else {
                                    1.0
                                })
                                .object_fit(gpui::ObjectFit::Contain)
                                .rounded_sm(),
                        )
                        .child(paste_preview::bounds_marker(image.bounds.clone())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap_1()
                        .child(div().truncate().child(image.label.clone()))
                        .child(
                            div()
                                .text_color(Theme::global().TEXT_FAINT)
                                .child("remove ×"),
                        ),
                )
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.remove_attachment(index, cx);
                    }),
                )
        });
        div()
            .debug_selector(|| "prompt-input".into())
            .children(attachment_overlay)
            .flex()
            .flex_col()
            .key_context("PromptInput")
            .track_focus(&self.focus_handle(cx))
            .relative()
            .when(
                command_visible,
                |el| {
                    el.child(
                        div()
                            .id("slash-command-suggestions")
                            .debug_selector(|| "slash-command-overlay".into())
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_full()
                            .mb(px(4.0 + 6.0 * (1.0 - menu_progress)))
                            .opacity(0.65 + 0.35 * menu_progress)
                            .flex()
                            .flex_col()
                            .rounded_lg()
                            .border_1()
                            .border_color(Theme::global().PANEL_BORDER_FOCUS)
                            .bg(Theme::global().HEADER_BG)
                            .shadow_lg()
                            .occlude()
                            .child(
                                div()
                                    .relative()
                                    .p_1()
                                    .child(
                                        div()
                                            .id("slash-command-scroll")
                                            .debug_selector(|| "slash-command-scroll".into())
                                            .flex()
                                            .flex_col()
                                            .max_h(px((f32::from(window.viewport_size().height)
                                                * if self.content.trim_start().starts_with("/model") { 0.6 } else { 0.35 })
                                                .min(if self.content.trim_start().starts_with("/model") { 480. } else { 280. })))
                                            .overflow_y_scroll()
                                            .track_scroll(&self.command_scroll)
                                            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                                            .pr(px(crate::scrollbar::GUTTER - 4.0))
                                            .when(suggestions.is_empty(), |el| {
                                                el.child(
                                                    div()
                                                        .px_3()
                                                        .py_2()
                                                        .text_size(px(12.0))
                                                        .text_color(Theme::global().TEXT_FAINT)
                                                        .child("No models match"),
                                                )
                                            })
                                            .children(suggestions.into_iter().enumerate().map(
                                                |(index, suggestion)| {
                                                    let is_toggle = suggestion.toggle.is_some();
                                                    let selected = index == command_selection;
                                                    let ink = if selected {
                                                        Theme::global().BG
                                                    } else {
                                                        Theme::global().TEXT_DIM
                                                    };
                                                    let model =
                                                        suggestion.value.strip_prefix("/model ");
                                                    let logo = model.map(|model| {
                                                        let provider = self
                                                            .model_logo_providers
                                                            .get(model)
                                                            .map(String::as_str)
                                                            .or_else(|| self.model_details.get(model).and_then(|detail| self.model_logo_providers.get(&detail.model).map(String::as_str)))
                                                            .unwrap_or("");
                                                        let logo: gpui::AnyElement =
                                                            match crate::accounts::logo(provider) {
                                                                Some(bytes) => gpui::svg()
                                                                    .data(bytes)
                                                                    .size(px(18.0))
                                                                    .flex_none()
                                                                    .text_color(ink)
                                                                    .into_any_element(),
                                                                None => div()
                                                                    .size(px(18.0))
                                                                    .flex_none()
                                                                    .text_size(px(10.0))
                                                                    .child(
                                                                        crate::accounts::lettermark(
                                                                            model,
                                                                        ),
                                                                    )
                                                                    .into_any_element(),
                                                            };
                                                        div()
                                                            .debug_selector(move || {
                                                                format!("model-picker-logo-{index}")
                                                            })
                                                            .child(logo)
                                                    });
                                                    let label = model
                                                        .and_then(|model| self.model_details.get(model).map(|detail| detail.model.as_str()))
                                                        .or(model)
                                                        .unwrap_or(&suggestion.value)
                                                        .to_string();
                                                    div().flex_none().flex().flex_col()
                                                        .children(suggestion.header.clone().map(|header| div()
                                                            .debug_selector(move || format!("model-picker-group-{index}"))
                                                            .px_3().pt_2().pb_1().text_size(px(11.0))
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .text_color(Theme::global().TEXT_FAINT).child(header)))
                                                        .child(div()
                                                        .id(("slash-command", index))
                                                        .debug_selector(move || {
                                                            if is_toggle { format!("model-picker-toggle-{index}") } else { format!("slash-command-row-{index}") }
                                                        })
                                                        .flex_none()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap_3()
                                                        .px_3()
                                                        .py_1p5()
                                                        .rounded_md()
                                                        .bg(if selected {
                                                            Theme::global().TEXT
                                                        } else {
                                                            Theme::global().HEADER_BG
                                                        })
                                                        .when(!selected, |row| {
                                                            row.hover(|style| {
                                                                style.bg(Theme::global().INPUT_BG)
                                                            })
                                                        })
                                                        .text_size(px(12.0))
                                                        .text_color(ink)
                                                        .cursor_pointer()
                                                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                                            if this.command_selection != index {
                                                                this.command_selection = index;
                                                                cx.notify();
                                                            }
                                                        }))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .overflow_hidden()
                                                                .items_center()
                                                                .gap_3()
                                                                .children(logo)
                                                                .font_family(
                                                                    Theme::global().FONT_MONO,
                                                                )
                                                                .text_color(ink)
                                                                .when(selected, |label| {
                                                                    label.font_weight(
                                                                        gpui::FontWeight::SEMIBOLD,
                                                                    )
                                                                })
                                                                .child(
                                                                    div()
                                                                        .flex()
                                                                        .flex_col()
                                                                        .min_w_0()
                                                                        .gap_1()
                                                                        .child(div().min_w_0().truncate().child(label))
                                                                        .children(suggestion.detail.map(|detail| div()
                                                                            .debug_selector(move || format!("model-picker-detail-{index}"))
                                                                            .min_w_0()
                                                                            .truncate()
                                                                            .text_size(px(11.0))
                                                                            .font_weight(gpui::FontWeight::NORMAL)
                                                                            .text_color(if selected { ink } else { Theme::global().TEXT_FAINT })
                                                                            .child(detail))),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .max_w(relative(0.4))
                                                                .min_w_0()
                                                                .truncate()
                                                                .text_color(if selected {
                                                                    ink
                                                                } else {
                                                                    Theme::global().TEXT_FAINT
                                                                })
                                                                .child(suggestion.help),
                                                        )
                                                        .child(
                                                            div()
                                                                .w(px(14.0))
                                                                .flex_none()
                                                                .text_color(ink)
                                                                .when(selected, |marker| {
                                                                    marker
                                                                        .debug_selector(|| {
                                                                            "slash-command-selected"
                                                                                .into()
                                                                        })
                                                                        .child("✓")
                                                                }),
                                                        )
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(
                                                                move |this, _, window, cx| {
                                                                    cx.stop_propagation();
                                                                    window.focus(
                                                                        &this.focus_handle,
                                                                        cx,
                                                                    );
                                                                    if let Some(key) = suggestion.toggle.clone() {
                                                                        this.toggle_model_group(key, cx);
                                                                        return;
                                                                    }
                                                                    if matches!(
                                                                        suggestion.value.as_str(),
                                                                        "/model" | "/models"
                                                                    ) {
                                                                        this.set_content(
                                                                            "/model ".into(),
                                                                            cx,
                                                                        );
                                                                        if let Some(on_change) =
                                                                            &this.on_change
                                                                        {
                                                                            on_change(
                                                                                &this.content,
                                                                                cx,
                                                                            );
                                                                        }
                                                                    } else {
                                                                        this.set_content(
                                                                            suggestion
                                                                                .value
                                                                                .clone(),
                                                                            cx,
                                                                        );
                                                                        // Search may also match a longer model name. Click must
                                                                        // submit this exact route, not the first substring match.
                                                                        this.command_selection = this.command_suggestions().iter()
                                                                            .position(|row| row.value == suggestion.value).unwrap_or(0);
                                                                        this.submit(
                                                                            &Submit, window, cx,
                                                                        );
                                                                    }
                                                                },
                                                            ),
                                                        ))
                                                },
                                            )),
                                    )
                                    .child(crate::scrollbar::vertical_with_track(
                                        &self.command_scroll,
                                        "slash-command-scrollbar",
                                    )),
                            ),
                    )
                },
            )
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_back))
            .on_action(cx.listener(Self::delete_word_forward))
            .on_action(cx.listener(Self::kill_to_start))
            .on_action(cx.listener(Self::kill_to_end))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::history_prev))
            .on_action(cx.listener(Self::history_next))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::queue))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .min_w_0()
            .flex_none()
            .bg(Theme::global().INPUT_BG)
            .border_1()
            .border_color(if focused {
                Theme::global().PANEL_BORDER_FOCUS
            } else {
                Theme::global().INPUT_BORDER
            })
            .rounded_lg()
            .when(!self.attachments.is_empty(), |el| {
                el.child(
                    div()
                        .id("prompt-attachments")
                        .max_h(px(editor_height))
                        .overflow_y_scroll()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .px_2()
                        .pt_2()
                        .children(attachments),
                )
            })
            .children(self.attachment_notice.clone().map(|notice| {
                div()
                    .px_3()
                    .pt_1()
                    .text_size(px(10.0))
                    .text_color(Theme::global().TEXT_FAINT)
                    .child(notice)
            }))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .px_3()
                    .py_2()
                    .text_size(px(14.0))
                    .when(spacious, |el| {
                        el.min_h(px(112.0)).px_4().py_4().text_size(px(16.0))
                    })
                    .child(
                        div()
                            .id("prompt-editor")
                            .debug_selector(|| "prompt-editor".into())
                            .flex_1()
                            .min_w_0()
                            .max_h(px(editor_height))
                            .overflow_y_scroll()
                            .track_scroll(&self.editor_scroll)
                            .text_color(Theme::global().TEXT)
                            .child(TextElement { input: cx.entity() }),
                    ),
            )
    }
}

impl Focusable for PromptInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowOptions};

    #[test]
    fn slash_palette_filters_commands_and_models() {
        let models = vec!["gpt-5.6-sol".into(), "claude-fable-5".into()];
        let commands = command_suggestions("/m", &models);
        assert_eq!(commands[0].value, "/model");
        assert!(commands.iter().any(|entry| entry.value == "/models"));

        let picker = command_suggestions("/model claude", &models);
        assert_eq!(picker.len(), 1);
        assert_eq!(picker[0].value, "/model claude-fable-5");
        assert!(command_suggestions("hello /model", &models).is_empty());

        let effort = command_suggestions("/effort x", &models);
        assert_eq!(effort[0].value, "/effort xhigh");

        let tui_commands = command_suggestions("/session", &models);
        assert!(tui_commands.iter().any(|entry| entry.value == "/sessions"));
        assert!(
            command_suggestions("/logical commits", &models)
                .iter()
                .any(|entry| entry.value == "/commit")
        );
    }

    #[test]
    fn slash_palette_keeps_every_command_reachable_by_scrolling() {
        let suggestions = command_suggestions("/", &[]);
        let registered = registered_command_entries().collect::<Vec<_>>();
        assert!(registered.len() > 12);
        assert_eq!(suggestions.len(), registered.len());
        for (suggestion, (command, _)) in suggestions.iter().zip(registered) {
            assert_eq!(suggestion.value, command);
        }
    }

    #[test]
    fn enter_accepts_highlighted_description_match() {
        let suggestions = command_suggestions("/logical commits", &[]);
        assert_eq!(
            accepted_command_submission(&suggestions, 0).as_deref(),
            Some("/commit")
        );
        let aliases = command_suggestions("/models", &[]);
        assert_eq!(
            accepted_command_submission(&aliases, 0).as_deref(),
            Some("/models")
        );
    }

    #[test]
    fn filtered_model_search_accepts_the_full_selected_model_id() {
        let models = vec!["gpt-5.6-sol".into(), "gpt-5.6-luna".into()];
        let suggestions = command_suggestions("/model luna", &models);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(
            accepted_command_submission(&suggestions, 0).as_deref(),
            Some("/model gpt-5.6-luna")
        );
    }

    fn input_window(cx: &mut TestAppContext) -> gpui::WindowHandle<PromptInput> {
        cx.update(|cx| bind_keys(cx));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|cx| PromptInput::new(cx, "test", |_, _, _, _| {}))
            })
            .unwrap()
        });
        window
            .update(cx, |input, window, cx| {
                window.focus(&input.focus_handle, cx)
            })
            .unwrap();
        window
    }

    #[gpui::test]
    fn model_groups_expand_with_enter_without_submitting_and_preserve_exact_routes(
        cx: &mut TestAppContext,
    ) {
        let window = input_window(cx);
        let routes: Vec<_> = (1..=5)
            .map(|n| jcode_sdk::ModelRouteInfo {
                model: format!("atlas-{n}"),
                provider: "OpenAI".into(),
                api_method: "openai-api-key".into(),
                available: true,
                detail: String::new(),
                usage: None,
            })
            .collect();
        window
            .update(cx, |input, _, cx| {
                input.set_model_routes(Vec::new(), &routes, None, cx);
                input.set_content("/model".into(), cx);
                assert_eq!(input.model_picker_rows().len(), 3);
                input.command_selection = 3;
                assert!(accepted_command_submission(&input.command_suggestions(), 3).is_none());
            })
            .unwrap();
        cx.simulate_keystrokes(*window, "enter");
        window
            .update(cx, |input, _, _| {
                assert_eq!(input.content.as_ref(), "/model");
                assert!(input.history.is_empty());
                assert_eq!(input.model_picker_rows().len(), 5);
                assert_eq!(input.command_selection, 5);
            })
            .unwrap();
        cx.simulate_keystrokes(*window, "enter");
        window
            .update(cx, |input, _, cx| {
                assert_eq!(input.model_picker_rows().len(), 3);
                assert_eq!(input.command_selection, 3);
                input.toggle_model_group("OpenAI · openai-api-key".into(), cx);
                input.set_content("/model atlas-5".into(), cx);
                assert!(!input.expanded_model_groups.is_empty());
                assert_eq!(
                    input.command_suggestions()[0].value,
                    "/model openai-api:atlas-5"
                );
            })
            .unwrap();
        cx.simulate_keystrokes(*window, "enter");
        window
            .update(cx, |input, _, _| {
                assert_eq!(
                    input.history.last().map(String::as_str),
                    Some("/model openai-api:atlas-5")
                );
                assert!(input.content.is_empty());
                assert!(input.expanded_model_groups.is_empty());
            })
            .unwrap();
        window
            .update(cx, |input, _, cx| {
                input.set_content("/model".into(), cx);
                input.toggle_model_group("OpenAI · openai-api-key".into(), cx);
                input.set_content(String::new(), cx);
                input.set_content("/model".into(), cx);
                assert_eq!(input.model_picker_rows().len(), 3);
            })
            .unwrap();
    }

    #[gpui::test]
    fn model_group_refresh_reveals_selected_choice_if_it_falls_below_top_three(
        cx: &mut TestAppContext,
    ) {
        let window = input_window(cx);
        let routes = |favored: &str| {
            (1..=5)
                .map(|n| {
                    let model = format!("atlas-{n}");
                    jcode_sdk::ModelRouteInfo {
                        usage: Some(jcode_sdk::ModelUsage {
                            count: if model == favored { 99 } else { n },
                            ..Default::default()
                        }),
                        model,
                        provider: "OpenAI".into(),
                        api_method: "openai-oauth".into(),
                        available: true,
                        detail: String::new(),
                    }
                })
                .collect::<Vec<_>>()
        };
        window
            .update(cx, |input, _, cx| {
                input.set_model_routes(Vec::new(), &routes("atlas-1"), None, cx);
                input.set_content("/model".into(), cx);
                assert_eq!(
                    input.command_suggestions()[0].value,
                    "/model openai-oauth:atlas-1"
                );
                input.set_model_routes(Vec::new(), &routes("atlas-5"), None, cx);
                assert_eq!(
                    input.command_suggestions()[input.command_selection].value,
                    "/model openai-oauth:atlas-1"
                );
                assert_eq!(input.model_picker_rows().len(), 5);
            })
            .unwrap();
    }

    #[gpui::test]
    fn model_usage_ranking_preserves_the_highlighted_choice_on_refresh(cx: &mut TestAppContext) {
        let window = input_window(cx);
        let route = |model: &str, count| jcode_sdk::ModelRouteInfo {
            model: model.into(),
            provider: "openai".into(),
            api_method: "oauth".into(),
            available: true,
            detail: String::new(),
            usage: Some(jcode_sdk::ModelUsage {
                count,
                last_used_unix_secs: Some(100),
                tracking_started_unix_secs: Some(1),
                selection_count: 0,
                last_selected_unix_secs: None,
            }),
        };
        window
            .update(cx, |input, _, cx| {
                input.set_model_routes(
                    vec!["a-rare".into(), "z-favorite".into()],
                    &[route("a-rare", 1), route("z-favorite", 12)],
                    Some("z-favorite".into()),
                    cx,
                );
                input.set_content("/model".into(), cx);
                let suggestions = input.command_suggestions();
                assert_eq!(suggestions[0].value, "/model z-favorite");
                assert_eq!(suggestions[0].help, "Current");
                assert!(
                    suggestions[0]
                        .detail
                        .as_ref()
                        .unwrap()
                        .contains("12 tracked turns")
                );
            })
            .unwrap();
        cx.simulate_keystrokes(*window, "down");
        window
            .update(cx, |input, _, cx| {
                assert_eq!(
                    input.command_suggestions()[input.command_selection].value,
                    "/model a-rare"
                );
                input.set_model_routes(
                    vec!["a-rare".into(), "z-favorite".into()],
                    &[route("a-rare", 20), route("z-favorite", 12)],
                    Some("z-favorite".into()),
                    cx,
                );
                let suggestions = input.command_suggestions();
                assert_eq!(input.command_selection, 1);
                assert_eq!(
                    accepted_command_submission(&suggestions, input.command_selection).as_deref(),
                    Some("/model a-rare")
                );
                input.set_content("/model favorite".into(), cx);
                assert_eq!(input.command_suggestions()[0].value, "/model z-favorite");
            })
            .unwrap();
    }

    #[gpui::test]
    fn long_prompts_wrap_and_expand_vertically(cx: &mut TestAppContext) {
        let window = input_window(cx);
        cx.simulate_input(*window, &"a long prompt ".repeat(200));
        cx.run_until_parked();

        window
            .update(cx, |input, _, _| {
                assert!(
                    input.visual_line_count > 1,
                    "long prompt should occupy multiple visual lines"
                );
            })
            .unwrap();
    }

    /// The workspace binds Ctrl+B to ToggleSidebar while the prompt binds it to
    /// MoveWordLeft. The prompt's context-scoped binding must win while the
    /// prompt has focus, otherwise typing would silently move window chrome.
    #[gpui::test]
    fn ctrl_b_moves_by_word_in_the_prompt_despite_the_sidebar_shortcut(cx: &mut TestAppContext) {
        let window = input_window(cx);
        cx.update(|cx| crate::bind_workspace_keys(cx));
        cx.simulate_input(*window, "one two three");
        cx.run_until_parked();

        let mut cx = gpui::VisualTestContext::from_window(*window, cx);
        cx.simulate_keystrokes("ctrl-b");
        cx.run_until_parked();

        window
            .update(&mut cx, |input, _, _| {
                assert_eq!(
                    &input.content[input.selected_range.start..],
                    "three",
                    "Ctrl+B should move the caret one word left inside the prompt"
                );
            })
            .unwrap();
    }

    #[test]
    fn word_motion_crosses_words_and_whitespace() {
        let text = "one   two three";
        assert_eq!(next_word_boundary(text, 0), 6);
        assert_eq!(next_word_boundary(text, 6), 10);
        assert_eq!(next_word_boundary(text, text.len()), text.len());
        assert_eq!(previous_word_boundary(text, text.len()), 10);
        assert_eq!(previous_word_boundary(text, 10), 6);
        assert_eq!(previous_word_boundary(text, 0), 0);
    }

    #[test]
    fn word_motion_never_splits_multibyte_text() {
        let text = "你好  world 🚀";
        let second_word = next_word_boundary(text, 0);
        assert_eq!(&text[second_word..], "world 🚀");
        let emoji = next_word_boundary(text, second_word);
        assert_eq!(&text[emoji..], "🚀");
        assert_eq!(previous_word_boundary(text, text.len()), emoji);
        for offset in [second_word, emoji, previous_word_boundary(text, emoji)] {
            assert!(text.is_char_boundary(offset));
        }
    }

    #[gpui::test]
    fn ported_editing_chords_dispatch_through_the_real_keymap(cx: &mut TestAppContext) {
        let window = input_window(cx);
        cx.simulate_input(*window, "one two");
        cx.simulate_keystrokes(*window, "alt-b ctrl-w");
        window
            .update(cx, |input, _, _| assert_eq!(input.content.as_ref(), "two"))
            .unwrap();

        cx.simulate_keystrokes(*window, "ctrl-z ctrl-shift-z ctrl-u");
        window
            .update(cx, |input, _, _| assert!(input.content.is_empty()))
            .unwrap();
    }

    #[gpui::test]
    fn ctrl_enter_preserves_images_and_respects_empty_and_disabled_input(cx: &mut TestAppContext) {
        let submitted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = submitted.clone();
        cx.update(bind_keys);
        let (input, vcx) = cx.add_window_view(|_, cx| {
            PromptInput::new_with_queue(cx, "test", move |text, images, queued, _, _| {
                seen.lock().unwrap().push((text, images, queued));
            })
        });
        vcx.update(|window, cx| {
            window.focus(&input.read(cx).focus_handle.clone(), cx);
        });
        vcx.simulate_keystrokes("ctrl-enter");
        assert!(submitted.lock().unwrap().is_empty());
        input.update(vcx, |input, cx| {
            input.attachments.push(Attachment {
                media_type: "image/png".into(),
                encoded: "cG5n".into(),
                label: "4×3".into(),
                preview: preview_image("image/png", Vec::new()).unwrap(),
                bounds: Default::default(),
            });
            input.set_submission_enabled(false, cx);
        });
        vcx.simulate_keystrokes("ctrl-enter");
        assert!(submitted.lock().unwrap().is_empty());
        input.update(vcx, |input, cx| {
            assert_eq!(input.attachments.len(), 1);
            input.set_submission_enabled(true, cx);
        });
        vcx.simulate_keystrokes("ctrl-enter");
        assert_eq!(
            &*submitted.lock().unwrap(),
            &[(
                "[image]".into(),
                vec![("image/png".into(), "cG5n".into())],
                true
            )]
        );
        input.read_with(vcx, |input, _| {
            assert!(input.attachments.is_empty());
            assert_eq!(input.visual_line_count, 1);
            assert_eq!(input.history, vec!["[image]"]);
        });
    }

    #[gpui::test]
    fn history_and_escape_dispatch_through_the_real_keymap(cx: &mut TestAppContext) {
        let submitted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = submitted.clone();
        cx.update(|cx| bind_keys(cx));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|cx| {
                    PromptInput::new(cx, "test", move |text, _, _, _| {
                        seen.lock().unwrap().push(text)
                    })
                })
            })
            .unwrap()
        });
        window
            .update(cx, |input, window, cx| {
                window.focus(&input.focus_handle, cx)
            })
            .unwrap();

        cx.simulate_input(*window, "first");
        cx.simulate_keystrokes(*window, "enter");
        cx.simulate_input(*window, "draft");
        cx.simulate_keystrokes(*window, "ctrl-[ ctrl-] up escape");

        assert_eq!(&*submitted.lock().unwrap(), &["first"]);
        window
            .update(cx, |input, _, _| assert!(input.content.is_empty()))
            .unwrap();
    }

    #[gpui::test]
    fn composer_editor_has_no_prompt_prefix_in_compact_or_spacious_mode(cx: &mut TestAppContext) {
        let (input, vcx) =
            cx.add_window_view(|_, cx| PromptInput::new(cx, "Type something…", |_, _, _, _| {}));
        for spacious in [false, true] {
            input.update(vcx, |input, cx| {
                input.spacious = spacious;
                cx.notify();
            });
            vcx.run_until_parked();
            let composer = vcx.debug_bounds("prompt-input").unwrap();
            let editor = vcx.debug_bounds("prompt-editor").unwrap();
            let left = editor.left() - composer.left();
            let right = composer.right() - editor.right();
            assert!(
                f32::from(left - right).abs() < 1.0,
                "editor must not reserve extra left padding for a prompt marker"
            );
        }
    }

    #[gpui::test]
    fn submitting_wrapped_prompt_resets_height_before_the_next_paint(cx: &mut TestAppContext) {
        let (input, vcx) =
            cx.add_window_view(|_, cx| PromptInput::new(cx, "test", |_, _, _, _| {}));
        vcx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.content = "A wrapped prompt occupying several visual lines".into();
                input.visual_line_count = 12;
                input.submit(&Submit, window, cx);
                assert!(input.content.is_empty());
                assert_eq!(
                    input.visual_line_count, 1,
                    "cleared editor retained old wrapped height"
                );
            });
        });
    }

    #[gpui::test]
    fn image_only_prompt_submits_the_attachment_and_clears_the_composer(cx: &mut TestAppContext) {
        let submitted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = submitted.clone();
        cx.update(|cx| bind_keys(cx));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|cx| {
                    PromptInput::new(cx, "test", move |text, images, _, _| {
                        seen.lock().unwrap().push((text, images));
                    })
                })
            })
            .unwrap()
        });
        window
            .update(cx, |input, window, cx| {
                input.attachments.push(Attachment {
                    media_type: "image/png".into(),
                    encoded: "cG5n".into(),
                    label: "4×3".into(),
                    preview: preview_image("image/png", Vec::new()).unwrap(),
                    bounds: Default::default(),
                });
                window.focus(&input.focus_handle, cx);
            })
            .unwrap();

        cx.simulate_keystrokes(*window, "enter");

        assert_eq!(
            &*submitted.lock().unwrap(),
            &[(
                "[image]".to_string(),
                vec![("image/png".to_string(), "cG5n".to_string())]
            )]
        );
        window
            .update(cx, |input, _, _| {
                assert!(input.content.is_empty());
                assert!(input.attachments.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn removing_an_attachment_updates_the_visible_count(cx: &mut TestAppContext) {
        let window = input_window(cx);
        window
            .update(cx, |input, _, cx| {
                for label in ["4×3", "8×6"] {
                    input.attachments.push(Attachment {
                        media_type: "image/png".into(),
                        encoded: "cG5n".into(),
                        label: label.into(),
                        preview: preview_image("image/png", Vec::new()).unwrap(),
                        bounds: Default::default(),
                    });
                }
                input.remove_attachment(0, cx);
                assert_eq!(input.attachments.len(), 1);
                assert_eq!(input.attachments[0].label.as_ref(), "8×6");
                assert_eq!(input.attachment_notice.as_deref(), Some("1 image attached"));
            })
            .unwrap();
    }
}

#[cfg(test)]
#[path = "input_responsive_tests.rs"]
mod responsive_tests;

#[cfg(test)]
mod pending_session_tests {
    use super::*;

    #[gpui::test]
    fn pending_commands_preserve_editor_and_explain_readiness(cx: &mut gpui::TestAppContext) {
        cx.update(bind_keys);
        let (input, vcx) = cx.add_window_view(|_, cx| {
            let mut input = PromptInput::new(cx, "test", |_, _, _, _| {
                panic!("pending commands must not invoke submit callbacks");
            });
            input.set_pending_session(true, cx);
            input
        });
        vcx.update(|window, cx| {
            window.focus(&input.read(cx).focus_handle.clone(), cx);
        });
        for command in ["/clear", "/login", "/changelog", "/onboarding-sim"] {
            input.update(vcx, |input, cx| {
                input.content = command.into();
                input.selected_range = command.len()..command.len();
                cx.notify();
            });
            vcx.simulate_keystrokes("enter");
            vcx.simulate_keystrokes("ctrl-enter");
            input.read_with(vcx, |input, _| {
                assert_eq!(input.content.as_ref(), command);
                assert_eq!(
                    input.attachment_notice.as_deref(),
                    Some(PENDING_COMMAND_NOTICE)
                );
            });
        }
        input.update(vcx, |input, cx| {
            input.set_pending_session(false, cx);
            assert!(input.attachment_notice.is_none());
            assert_eq!(input.content.as_ref(), "/onboarding-sim");
        });
    }
}
