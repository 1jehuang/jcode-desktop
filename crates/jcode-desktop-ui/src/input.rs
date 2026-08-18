//! Prompt input with full IME support, adapted from gpui's input example.
//! Long prompts soft-wrap and grow vertically. Enter submits via a callback.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, ImageSource, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    SharedString, Style, StyledImage, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine,
    actions, div, fill, img, point, prelude::*, px, relative, size,
};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::commands::registered_command_entries;
use crate::theme::{Theme, to_hsla};

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
    ]
);

pub fn bind_keys(cx: &mut App) {
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
    attachment_preview: Option<(usize, Instant)>,
    on_submit: Box<dyn Fn(String, Vec<(String, String)>, &mut Window, &mut App)>,
    on_change: Option<Box<dyn Fn(&str, &mut App)>>,
    command_models: Vec<String>,
    command_selection: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommandSuggestion {
    value: String,
    help: String,
}

fn command_suggestions(input: &str, models: &[String]) -> Vec<CommandSuggestion> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') || trimmed.contains('\n') {
        return Vec::new();
    }
    if trimmed == "/model" || trimmed == "/models" || trimmed.starts_with("/model ") {
        let query = trimmed
            .split_once(' ')
            .map(|(_, query)| query.trim().to_ascii_lowercase())
            .unwrap_or_default();
        return models
            .iter()
            .filter(|model| query.is_empty() || model.to_ascii_lowercase().contains(&query))
            .take(8)
            .map(|model| CommandSuggestion {
                value: format!("/model {model}"),
                help: "Switch this session".into(),
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
        .take(12)
        .map(|(value, help, _)| CommandSuggestion {
            value: value.into(),
            help: help.into(),
        })
        .collect()
}

fn accepted_command_submission(
    content: &str,
    suggestions: &[CommandSuggestion],
    selection: usize,
) -> Option<String> {
    let suggestion = suggestions.get(selection.min(suggestions.len().saturating_sub(1)))?;
    let trimmed = content.trim();
    (trimmed == "/model"
        || trimmed == "/models"
        || trimmed.starts_with("/model ")
        || trimmed == "/effort"
        || trimmed.starts_with("/effort ")
        || (!trimmed.contains(' ') && suggestion.value.starts_with(trimmed)))
    .then(|| suggestion.value.clone())
}

#[derive(Clone)]
struct Attachment {
    media_type: String,
    encoded: String,
    label: SharedString,
    preview: Arc<gpui::Image>,
}

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

fn preview_image(media_type: &str, bytes: Vec<u8>) -> Option<Arc<gpui::Image>> {
    Some(Arc::new(gpui::Image {
        format: gpui::ImageFormat::from_mime_type(media_type)?,
        bytes,
        id: NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
    }))
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
            on_submit: Box::new(on_submit),
            on_change: None,
            command_models: Vec::new(),
            command_selection: 0,
        }
    }

    pub fn with_on_change(mut self, on_change: impl Fn(&str, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(on_change));
        self
    }

    pub fn set_command_models(&mut self, models: Vec<String>, cx: &mut Context<Self>) {
        if self.command_models != models {
            self.command_models = models;
            self.command_selection = 0;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn command_models(&self) -> &[String] {
        &self.command_models
    }

    fn command_suggestions(&self) -> Vec<CommandSuggestion> {
        command_suggestions(&self.content, &self.command_models)
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        let mut content = self.content.trim().to_string();
        if content.is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.attachments.is_empty() {
            let suggestions = self.command_suggestions();
            if let Some(accepted) =
                accepted_command_submission(&content, &suggestions, self.command_selection)
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
        self.selected_range = 0..0;
        self.marked_range = None;
        self.history.push(content.clone());
        self.history_index = None;
        self.live_draft.clear();
        self.command_selection = 0;
        (self.on_submit)(content, images, window, cx);
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

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        match crate::clipboard_image::read() {
            Ok(Some(image)) => {
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
                });
                self.attachment_preview = Some((self.attachments.len() - 1, Instant::now()));
                self.attachment_notice = Some(match self.attachments.len() {
                    1 => format!("image attached ({label})").into(),
                    count => format!("{count} images attached").into(),
                });
                cx.notify();
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
        if !self.content.is_empty() {
            self.set_content(String::new(), cx);
        }
    }

    fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content.into();
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
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
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
                to_hsla(Theme::SELECTION),
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
            (input.placeholder.clone(), to_hsla(Theme::TEXT_DIM))
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
                    to_hsla(Theme::CURSOR),
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
            input.last_layout = Some(line);
            input.last_bounds = Some(prepaint.text_bounds);
            if input.visual_line_count != prepaint.visual_line_count {
                input.visual_line_count = prepaint.visual_line_count;
                _cx.notify();
            }
        });
    }
}

impl Render for PromptInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let suggestions = self.command_suggestions();
        if self.command_selection >= suggestions.len() {
            self.command_selection = 0;
        }
        let command_selection = self.command_selection;
        const SETTLE: Duration = Duration::from_millis(280);
        let settling_attachment = self.attachment_preview.and_then(|(index, started)| {
            let elapsed = Instant::now().saturating_duration_since(started);
            if elapsed >= SETTLE {
                self.attachment_preview = None;
                return None;
            }
            window.request_animation_frame();
            let linear = (elapsed.as_secs_f32() / SETTLE.as_secs_f32()).clamp(0.0, 1.0);
            let progress = linear * linear * (3.0 - 2.0 * linear);
            Some((index, progress))
        });
        let attachments = self.attachments.iter().enumerate().map(|(index, image)| {
            // Keep the animation in normal layout flow. An absolutely positioned
            // child here is positioned against a distant GPUI containing block,
            // which can leave the pasted image floating at the top of the panel.
            let progress = settling_attachment
                .filter(|(settling_index, _)| *settling_index == index)
                .map(|(_, progress)| progress)
                .unwrap_or(1.0);
            let preview_width = 64.0 + 20.0 * (1.0 - progress);
            let preview_height = 52.0 + 16.0 * (1.0 - progress);
            div()
                .id(("attachment", index))
                .flex()
                .items_center()
                .gap_1()
                .rounded_md()
                .p_1()
                .bg(Theme::USER_BG)
                .text_size(px(11.0))
                .text_color(Theme::TEXT_DIM)
                .child(
                    img(ImageSource::Image(image.preview.clone()))
                        .w(px(preview_width))
                        .h(px(preview_height))
                        .object_fit(gpui::ObjectFit::Contain)
                        .rounded_sm(),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(image.label.clone())
                        .child(div().text_color(Theme::TEXT_FAINT).child("remove ×")),
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
            .flex()
            .flex_col()
            .key_context("PromptInput")
            .track_focus(&self.focus_handle(cx))
            .relative()
            .when(!suggestions.is_empty(), |el| {
                el.child(
                    div()
                        .debug_selector(|| "slash-command-overlay".into())
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(42.0))
                        .mb_1()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded_lg()
                        .border_1()
                        .border_color(Theme::PANEL_BORDER_FOCUS)
                        .bg(Theme::HEADER_BG)
                        .shadow_lg()
                        .occlude()
                        .children(suggestions.into_iter().enumerate().map(
                            |(index, suggestion)| {
                                let selected = index == command_selection;
                                div()
                                    .id(("slash-command", index))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .px_3()
                                    .py_1p5()
                                    .bg(if selected {
                                        Theme::USER_BG
                                    } else {
                                        Theme::HEADER_BG
                                    })
                                    .text_size(px(12.0))
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .font_family(Theme::FONT_MONO)
                                            .text_color(if selected {
                                                Theme::TEXT
                                            } else {
                                                Theme::TEXT_DIM
                                            })
                                            .child(suggestion.value.clone()),
                                    )
                                    .child(
                                        div().text_color(Theme::TEXT_FAINT).child(suggestion.help),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.set_content(suggestion.value.clone(), cx);
                                            this.command_selection = 0;
                                            this.submit(&Submit, window, cx);
                                        }),
                                    )
                            },
                        )),
                )
            })
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .bg(Theme::INPUT_BG)
            .border_1()
            .border_color(if focused {
                Theme::PANEL_BORDER_FOCUS
            } else {
                Theme::INPUT_BORDER
            })
            .rounded_lg()
            .when(!self.attachments.is_empty(), |el| {
                el.child(
                    div()
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
                    .text_color(Theme::TEXT_FAINT)
                    .child(notice)
            }))
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .px_3()
                    .py_2()
                    .text_size(px(14.0))
                    .text_color(Theme::TEXT)
                    .child(TextElement { input: cx.entity() }),
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
    fn filtered_model_search_accepts_the_full_selected_model_id() {
        let models = vec!["gpt-5.6-sol".into(), "gpt-5.6-luna".into()];
        let suggestions = command_suggestions("/model luna", &models);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(
            accepted_command_submission("/model luna", &suggestions, 0).as_deref(),
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
