//! Native code editor for files opened from the Files panel.
//!
//! The text model is [`crate::text_buffer::TextBuffer`]. This view loads and
//! saves off the UI thread, and its element shapes and paints only the lines
//! inside the viewport, so cost is independent of file length. Syntax colors
//! come from an incremental per-line highlighter that re-highlights from the
//! first edited line to the end of the visible region.
use crate::markdown::{LineHighlighter, token_color};
use crate::text_buffer::{self, Selection, TextBuffer};
use crate::theme::{Theme, to_hsla};
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString,
    Style, TextRun, UTF16Selection, Window, actions, div, fill, point, prelude::*, px, relative,
    size,
};
use std::{ops::Range, path::PathBuf};

actions!(
    code_editor,
    [
        Save,
        Undo,
        Redo,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        Newline,
        Indent,
        Outdent,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveLineStart,
        MoveLineEnd,
        SelectLineStart,
        SelectLineEnd,
        MoveDocStart,
        MoveDocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
    ]
);

const CONTEXT: &str = "CodeEditor";
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 20.0;
const TAB_WIDTH: usize = 4;
/// Longer lines are drawn truncated; editing still sees the whole line.
const MAX_DISPLAY_CHARS: usize = 20_000;
/// Lines highlighted per frame; farther jumps finish over following frames.
const HIGHLIGHT_BUDGET: usize = 2_000;
/// A second close within this window discards unsaved changes.
const CLOSE_CONFIRM_WINDOW: std::time::Duration = std::time::Duration::from_secs(4);

pub(crate) fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    let mut bindings = vec![
        KeyBinding::new("secondary-s", Save, c),
        KeyBinding::new("secondary-z", Undo, c),
        KeyBinding::new("secondary-shift-z", Redo, c),
        KeyBinding::new("ctrl-y", Redo, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", Newline, c),
        KeyBinding::new("tab", Indent, c),
        KeyBinding::new("shift-tab", Outdent, c),
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("home", MoveLineStart, c),
        KeyBinding::new("end", MoveLineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
    ];
    if cfg!(target_os = "macos") {
        bindings.extend([
            KeyBinding::new("alt-left", MoveWordLeft, c),
            KeyBinding::new("alt-right", MoveWordRight, c),
            KeyBinding::new("alt-shift-left", SelectWordLeft, c),
            KeyBinding::new("alt-shift-right", SelectWordRight, c),
            KeyBinding::new("alt-backspace", DeleteWordLeft, c),
            KeyBinding::new("alt-delete", DeleteWordRight, c),
            KeyBinding::new("cmd-backspace", DeleteToLineStart, c),
            KeyBinding::new("cmd-left", MoveLineStart, c),
            KeyBinding::new("cmd-right", MoveLineEnd, c),
            KeyBinding::new("cmd-shift-left", SelectLineStart, c),
            KeyBinding::new("cmd-shift-right", SelectLineEnd, c),
            KeyBinding::new("cmd-up", MoveDocStart, c),
            KeyBinding::new("cmd-down", MoveDocEnd, c),
            KeyBinding::new("cmd-shift-up", SelectDocStart, c),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, c),
            KeyBinding::new("ctrl-a", MoveLineStart, c),
            KeyBinding::new("ctrl-e", MoveLineEnd, c),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-left", MoveWordLeft, c),
            KeyBinding::new("ctrl-right", MoveWordRight, c),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
            KeyBinding::new("ctrl-delete", DeleteWordRight, c),
            KeyBinding::new("ctrl-home", MoveDocStart, c),
            KeyBinding::new("ctrl-end", MoveDocEnd, c),
            KeyBinding::new("ctrl-shift-home", SelectDocStart, c),
            KeyBinding::new("ctrl-shift-end", SelectDocEnd, c),
        ]);
    }
    cx.bind_keys(bindings);
}

enum Content {
    Loading,
    Ready(TextBuffer),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SaveState {
    Idle,
    Saving,
    Failed(String),
}

/// A line as drawn: tabs expanded to spaces, maybe truncated.
struct DisplayLine {
    text: String,
    /// Display byte offset for every char column, plus one past the end.
    columns: Vec<usize>,
}

impl DisplayLine {
    fn new(line: &str) -> Self {
        let mut text = String::with_capacity(line.len());
        let mut columns = Vec::with_capacity(line.len() + 1);
        let mut visual = 0usize;
        for ch in line.chars().take(MAX_DISPLAY_CHARS) {
            columns.push(text.len());
            if ch == '\t' {
                let spaces = TAB_WIDTH - visual % TAB_WIDTH;
                text.extend(std::iter::repeat_n(' ', spaces));
                visual += spaces;
            } else {
                text.push(ch);
                visual += 1;
            }
        }
        columns.push(text.len());
        Self { text, columns }
    }

    fn byte_for_column(&self, column: usize) -> usize {
        self.columns[column.min(self.columns.len() - 1)]
    }

    /// The column whose display start is closest at or before `byte`.
    fn column_for_byte(&self, byte: usize) -> usize {
        match self.columns.binary_search(&byte) {
            Ok(column) => column,
            Err(column) => column.saturating_sub(1),
        }
    }
}

/// Map highlight tokens (byte ranges of the source line) to display runs.
fn runs_for(
    source: &str,
    display: &DisplayLine,
    tokens: &[(Range<usize>, u8)],
    font: &gpui::Font,
    theme: &Theme,
) -> Vec<TextRun> {
    let run = |len: usize, color: gpui::Rgba| TextRun {
        len,
        font: font.clone(),
        color: to_hsla(color),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let total = display.text.len();
    if tokens.is_empty() {
        return vec![run(total, theme.CODE_TEXT)];
    }
    // Source byte -> char column -> display byte.
    let mut byte_to_column = Vec::with_capacity(source.len() + 1);
    for (column, ch) in source.chars().enumerate() {
        byte_to_column.extend(std::iter::repeat_n(column, ch.len_utf8()));
    }
    byte_to_column.push(source.chars().count());
    let display_byte = |source_byte: usize| {
        let column = byte_to_column[source_byte.min(byte_to_column.len() - 1)];
        display.byte_for_column(column)
    };
    let mut runs = Vec::new();
    let mut cursor = 0;
    for (range, id) in tokens {
        let start = display_byte(range.start).max(cursor);
        let end = display_byte(range.end).min(total);
        if start >= end {
            continue;
        }
        if start > cursor {
            runs.push(run(start - cursor, theme.CODE_TEXT));
        }
        runs.push(run(end - start, token_color(*id, theme)));
        cursor = end;
    }
    if cursor < total || runs.is_empty() {
        runs.push(run(total - cursor, theme.CODE_TEXT));
    }
    runs
}

/// What the last paint drew, for hit testing and IME geometry.
struct Layout {
    bounds: Bounds<Pixels>,
    /// Window x of column 0 on every line (after horizontal scroll).
    text_left: Pixels,
    first_line: usize,
    /// Window y of `first_line`'s top.
    first_top: Pixels,
    lines: Vec<(ShapedLine, DisplayLine)>,
    line_count: usize,
}

pub struct CodeEditor {
    pub focus_handle: FocusHandle,
    path: PathBuf,
    content: Content,
    highlighter: LineHighlighter,
    scroll: Point<f32>,
    reveal_cursor: bool,
    selecting: bool,
    marked: Option<Range<usize>>,
    save: SaveState,
    layout: Option<Layout>,
    /// When closing was refused because of unsaved changes.
    close_warning: Option<std::time::Instant>,
}

impl CodeEditor {
    pub fn open(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let load_path = path.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { text_buffer::load(&load_path) })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.content = match loaded {
                    Ok(buffer) => Content::Ready(buffer),
                    Err(error) => Content::Failed(error.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
        Self {
            focus_handle: cx.focus_handle(),
            highlighter: LineHighlighter::for_path(&path),
            path,
            content: Content::Loading,
            scroll: Point::default(),
            reveal_cursor: false,
            selecting: false,
            marked: None,
            save: SaveState::Idle,
            layout: None,
            close_warning: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        matches!(&self.content, Content::Ready(buffer) if buffer.is_dirty())
    }

    /// Whether the panel may close. With unsaved changes the first request is
    /// refused and explained; a second one shortly after discards them.
    pub fn confirm_close(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.is_dirty()
            || self
                .close_warning
                .is_some_and(|warned| warned.elapsed() < CLOSE_CONFIRM_WINDOW)
        {
            return true;
        }
        self.close_warning = Some(std::time::Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CLOSE_CONFIRM_WINDOW).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
        false
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> Option<String> {
        match &self.content {
            Content::Ready(buffer) => Some(buffer.text()),
            _ => None,
        }
    }

    fn buffer(&mut self) -> Option<&mut TextBuffer> {
        match &mut self.content {
            Content::Ready(buffer) => Some(buffer),
            _ => None,
        }
    }

    /// Run a buffer operation, then invalidate caches and reveal the caret.
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut TextBuffer)) {
        let Content::Ready(buffer) = &mut self.content else {
            return;
        };
        f(buffer);
        if let Some(offset) = buffer.take_changed_from() {
            let (line, _) = buffer.position(offset);
            self.highlighter.invalidate_from(line);
            self.marked = None;
            if matches!(self.save, SaveState::Failed(_)) {
                self.save = SaveState::Idle;
            }
            self.close_warning = None;
        }
        self.reveal_cursor = true;
        cx.notify();
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let Content::Ready(buffer) = &self.content else {
            return;
        };
        if self.save == SaveState::Saving {
            return;
        }
        let token = buffer.state_token();
        let text = buffer.text_for_save();
        let path = self.path.clone();
        self.save = SaveState::Saving;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { text_buffer::save_atomic(&path, &text) })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.save = match result {
                    Ok(()) => {
                        if let Some(buffer) = editor.buffer() {
                            buffer.mark_saved_at(token);
                        }
                        SaveState::Idle
                    }
                    Err(error) => SaveState::Failed(error.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(buffer) = self.buffer() {
            let text = buffer.copy_text();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let mut text = None;
        self.edit(cx, |buffer| text = Some(buffer.cut()));
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(cx, |buffer| buffer.insert(&text));
        }
    }

    fn page_lines(&self) -> isize {
        self.layout
            .as_ref()
            .map_or(30, |layout| {
                (f32::from(layout.bounds.size.height) / LINE_HEIGHT) as isize - 1
            })
            .max(1)
    }

    // ---- Mouse -----------------------------------------------------------

    /// Document offset under a window position, clamped to the visible lines.
    fn offset_for_point(&self, position: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let Content::Ready(buffer) = &self.content else {
            return None;
        };
        let row = ((f32::from(position.y - layout.first_top)) / LINE_HEIGHT).floor();
        if row < 0.0 && layout.first_line == 0 {
            return Some(0);
        }
        let row = row.max(0.0) as usize;
        let line = layout.first_line + row;
        if line >= layout.line_count {
            return Some(buffer.len_chars());
        }
        let Some((shaped, display)) = layout.lines.get(row) else {
            return Some(buffer.offset(line, usize::MAX));
        };
        let x = (position.x - layout.text_left).max(px(0.0));
        let byte = shaped.closest_index_for_x(x);
        Some(buffer.offset(line, display.column_for_byte(byte)))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let Some(offset) = self.offset_for_point(event.position) else {
            return;
        };
        let extend = event.modifiers.shift;
        let clicks = event.click_count;
        if let Some(buffer) = self.buffer() {
            match clicks {
                2 => buffer.select_word_at(offset),
                3.. => buffer.select_line_at(offset),
                _ => {
                    let anchor = if extend {
                        buffer.selection().anchor
                    } else {
                        offset
                    };
                    buffer.set_selection(Selection {
                        anchor,
                        head: offset,
                    });
                }
            }
        }
        self.selecting = true;
        cx.stop_propagation();
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || !event.dragging() {
            return;
        }
        let Some(offset) = self.offset_for_point(event.position) else {
            return;
        };
        if let Some(buffer) = self.buffer() {
            let anchor = buffer.selection().anchor;
            buffer.set_selection(Selection {
                anchor,
                head: offset,
            });
        }
        self.reveal_cursor = true;
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(LINE_HEIGHT));
        self.scroll.y -= f32::from(delta.y);
        self.scroll.x -= f32::from(delta.x);
        // Clamped against the content size at the next prepaint.
        let _ = window;
        cx.stop_propagation();
        cx.notify();
    }

    fn render_status(&self) -> impl IntoElement {
        let theme = Theme::global();
        let mut left = Vec::new();
        let mut save_text: Option<(String, gpui::Rgba)> = None;
        if let Content::Ready(buffer) = &self.content {
            let (line, column) = buffer.position(buffer.selection().head);
            left.push(format!("Ln {}, Col {}", line + 1, column + 1));
            let selected = buffer.selection().range().len();
            if selected > 0 {
                left.push(format!("{selected} selected"));
            }
            left.push(match buffer.indent_unit() {
                "\t" => "Tabs".to_owned(),
                unit => format!("Spaces: {}", unit.len()),
            });
            left.push(match buffer.line_ending() {
                text_buffer::LineEnding::Lf => "LF".to_owned(),
                text_buffer::LineEnding::CrLf => "CRLF".to_owned(),
            });
            let warned = self
                .close_warning
                .is_some_and(|warned| warned.elapsed() < CLOSE_CONFIRM_WINDOW);
            save_text = Some(match &self.save {
                _ if warned && buffer.is_dirty() => (
                    if cfg!(target_os = "macos") {
                        "Unsaved changes · ⌘W again to discard · ⌘S to save".into()
                    } else {
                        "Unsaved changes · close again to discard · Ctrl+S to save".into()
                    },
                    theme.ERROR,
                ),
                SaveState::Saving => ("Saving…".into(), theme.TEXT_DIM),
                SaveState::Failed(error) => (format!("Save failed: {error}"), theme.ERROR),
                SaveState::Idle if buffer.is_dirty() => (
                    if cfg!(target_os = "macos") {
                        "Unsaved · ⌘S to save".into()
                    } else {
                        "Unsaved · Ctrl+S to save".into()
                    },
                    theme.ACCENT,
                ),
                SaveState::Idle => ("Saved".into(), theme.TEXT_DIM),
            });
        }
        div()
            .debug_selector(|| "code-editor-status".into())
            .flex_none()
            .h(px(24.0))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .border_t_1()
            .border_color(theme.CODE_BORDER)
            .bg(theme.CODE_HEADER_BG)
            .text_size(px(11.0))
            .text_color(theme.TEXT_DIM)
            .overflow_hidden()
            .whitespace_nowrap()
            .children(left.into_iter().map(|text| div().flex_none().child(text)))
            .child(div().flex_1())
            .children(save_text.map(|(text, color)| {
                div()
                    .debug_selector(|| "code-editor-save-state".into())
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(color)
                    .child(text)
            }))
    }
}

macro_rules! movement {
    ($($name:ident => $action:ident, $body:expr;)*) => {
        impl CodeEditor {
            $(
                fn $name(&mut self, _: &$action, _: &mut Window, cx: &mut Context<Self>) {
                    let page = self.page_lines();
                    let _ = page;
                    let f: &dyn Fn(&mut TextBuffer, isize) = &$body;
                    self.edit(cx, |buffer| f(buffer, page));
                }
            )*
        }
        fn register_actions<E: InteractiveElement>(el: E, cx: &mut Context<CodeEditor>) -> E {
            el$(.on_action(cx.listener(CodeEditor::$name)))*
        }
    };
}

movement! {
    undo => Undo, |b, _| { b.undo(); };
    redo => Redo, |b, _| { b.redo(); };
    select_all => SelectAll, |b, _| b.select_all();
    backspace => Backspace, |b, _| b.backspace();
    delete => Delete, |b, _| b.delete_forward();
    delete_word_left => DeleteWordLeft, |b, _| b.delete_word_left();
    delete_word_right => DeleteWordRight, |b, _| b.delete_word_right();
    delete_to_line_start => DeleteToLineStart, |b, _| b.delete_to_line_start();
    newline => Newline, |b, _| b.newline();
    indent => Indent, |b, _| b.indent();
    outdent => Outdent, |b, _| b.outdent();
    move_left => MoveLeft, |b, _| b.move_left(false);
    move_right => MoveRight, |b, _| b.move_right(false);
    move_up => MoveUp, |b, _| b.move_up(false);
    move_down => MoveDown, |b, _| b.move_down(false);
    select_left => SelectLeft, |b, _| b.move_left(true);
    select_right => SelectRight, |b, _| b.move_right(true);
    select_up => SelectUp, |b, _| b.move_up(true);
    select_down => SelectDown, |b, _| b.move_down(true);
    move_word_left => MoveWordLeft, |b, _| b.move_word_left(false);
    move_word_right => MoveWordRight, |b, _| b.move_word_right(false);
    select_word_left => SelectWordLeft, |b, _| b.move_word_left(true);
    select_word_right => SelectWordRight, |b, _| b.move_word_right(true);
    move_line_start => MoveLineStart, |b, _| b.move_line_start(false);
    move_line_end => MoveLineEnd, |b, _| b.move_line_end(false);
    select_line_start => SelectLineStart, |b, _| b.move_line_start(true);
    select_line_end => SelectLineEnd, |b, _| b.move_line_end(true);
    move_doc_start => MoveDocStart, |b, _| b.move_doc_start(false);
    move_doc_end => MoveDocEnd, |b, _| b.move_doc_end(false);
    select_doc_start => SelectDocStart, |b, _| b.move_doc_start(true);
    select_doc_end => SelectDocEnd, |b, _| b.move_doc_end(true);
    page_up => PageUp, |b, page| b.move_vertically(-page, false);
    page_down => PageDown, |b, page| b.move_vertically(page, false);
    select_page_up => SelectPageUp, |b, page| b.move_vertically(-page, true);
    select_page_down => SelectPageDown, |b, page| b.move_vertically(page, true);
}

impl Focusable for CodeEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CodeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global();
        let body: gpui::AnyElement = match &self.content {
            Content::Loading => div()
                .flex_1()
                .p_4()
                .text_color(theme.TEXT_DIM)
                .child("Loading…")
                .into_any_element(),
            Content::Failed(error) => div()
                .debug_selector(|| "code-editor-error".into())
                .flex_1()
                .p_4()
                .text_color(theme.ERROR)
                .child(error.clone())
                .into_any_element(),
            Content::Ready(_) => div()
                .id("code-editor-text")
                .debug_selector(|| "code-editor-text".into())
                .flex_1()
                .min_h_0()
                .min_w_0()
                .cursor(gpui::CursorStyle::IBeam)
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|editor, _, _, _| editor.selecting = false),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|editor, _, _, _| editor.selecting = false),
                )
                .on_scroll_wheel(cx.listener(Self::on_scroll))
                .child(EditorElement {
                    editor: cx.entity(),
                })
                .into_any_element(),
        };
        let root = div()
            .id("code-editor")
            .debug_selector(|| "code-editor".into())
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.CODE_BG)
            .font_family(theme.FONT_MONO)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .child(body)
            .child(self.render_status());
        register_actions(root, cx)
    }
}

// ---- Platform text input (typing, IME, dictation) -------------------------

impl EntityInputHandler for CodeEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let Content::Ready(buffer) = &self.content else {
            return None;
        };
        let range =
            buffer.offset_from_utf16(range_utf16.start)..buffer.offset_from_utf16(range_utf16.end);
        actual_range
            .replace(buffer.offset_to_utf16(range.start)..buffer.offset_to_utf16(range.end));
        Some(buffer.slice(range))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let Content::Ready(buffer) = &self.content else {
            return None;
        };
        let selection = buffer.selection();
        let range = selection.range();
        Some(UTF16Selection {
            range: buffer.offset_to_utf16(range.start)..buffer.offset_to_utf16(range.end),
            reversed: selection.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let Content::Ready(buffer) = &self.content else {
            return None;
        };
        self.marked
            .as_ref()
            .map(|range| buffer.offset_to_utf16(range.start)..buffer.offset_to_utf16(range.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        self.edit(cx, |buffer| {
            let range = range_utf16
                .map(|range| {
                    buffer.offset_from_utf16(range.start)..buffer.offset_from_utf16(range.end)
                })
                .or(marked);
            match range {
                Some(range) if range != buffer.selection().range() => buffer.replace(range, text),
                _ => buffer.insert(text),
            }
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        let mut start = 0;
        self.edit(cx, |buffer| {
            let range = range_utf16
                .map(|range| {
                    buffer.offset_from_utf16(range.start)..buffer.offset_from_utf16(range.end)
                })
                .or(marked)
                .unwrap_or_else(|| buffer.selection().range());
            start = range.start;
            buffer.replace(range, new_text);
            // The IME's selection is relative to `new_text`, in UTF-16 units.
            if let Some(selected) = &new_selected_range_utf16 {
                let to_chars = |utf16: usize| {
                    let mut units = 0;
                    new_text
                        .chars()
                        .take_while(|ch| {
                            units += ch.len_utf16();
                            units <= utf16
                        })
                        .count()
                };
                buffer.set_selection(Selection {
                    anchor: start + to_chars(selected.start),
                    head: start + to_chars(selected.end),
                });
            }
        });
        if !new_text.is_empty() {
            self.marked = Some(start..start + new_text.chars().count());
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let Content::Ready(buffer) = &self.content else {
            return None;
        };
        let (line, column) = buffer.position(buffer.offset_from_utf16(range_utf16.start));
        let row = line.checked_sub(layout.first_line)?;
        let (shaped, display) = layout.lines.get(row)?;
        let x = layout.text_left + shaped.x_for_index(display.byte_for_column(column));
        let y = layout.first_top + px(LINE_HEIGHT * row as f32);
        Some(Bounds::new(point(x, y), size(px(2.0), px(LINE_HEIGHT))))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_point(point)?;
        let Content::Ready(buffer) = &self.content else {
            return None;
        };
        Some(buffer.offset_to_utf16(offset))
    }
}

// ---- Element ----------------------------------------------------------------

struct EditorElement {
    editor: Entity<CodeEditor>,
}

struct Prepaint {
    gutter_width: Pixels,
    numbers: Vec<(ShapedLine, Pixels)>,
    lines: Vec<(ShapedLine, Pixels)>,
    text_left: Pixels,
    selections: Vec<Bounds<Pixels>>,
    current_line: Option<Bounds<Pixels>>,
    cursor: Option<Bounds<Pixels>>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let theme = Theme::global();
        let font = gpui::font(theme.FONT_MONO);
        let font_size = px(FONT_SIZE);
        let line_height = px(LINE_HEIGHT);
        let focused = self.editor.read(cx).focus_handle.is_focused(window);
        self.editor.update(cx, |editor, _| {
            let Content::Ready(buffer) = &editor.content else {
                editor.layout = None;
                return Prepaint {
                    gutter_width: px(0.0),
                    numbers: Vec::new(),
                    lines: Vec::new(),
                    text_left: px(0.0),
                    selections: Vec::new(),
                    current_line: None,
                    cursor: None,
                };
            };
            let text_system = window.text_system().clone();
            let shape = |text: String, runs: &[TextRun]| {
                text_system.shape_line(SharedString::from(text), font_size, runs, None)
            };
            let plain_run = |len: usize, color: gpui::Rgba| TextRun {
                len,
                font: font.clone(),
                color: to_hsla(color),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let char_width = shape("0".into(), &[plain_run(1, theme.CODE_TEXT)]).width();
            let line_count = buffer.line_count();
            let digits = line_count.to_string().len().max(3);
            let gutter_width = char_width * digits as f32 + px(28.0);
            let text_width = (bounds.size.width - gutter_width).max(px(0.0));

            // Keep the caret on screen after keyboard input.
            let selection = buffer.selection();
            let (cursor_line, cursor_column) = buffer.position(selection.head);
            let height = f32::from(bounds.size.height);
            if editor.reveal_cursor {
                let top = cursor_line as f32 * LINE_HEIGHT;
                let margin = (LINE_HEIGHT * 2.0).min(height / 3.0);
                if top - margin < editor.scroll.y {
                    editor.scroll.y = top - margin;
                } else if top + LINE_HEIGHT + margin > editor.scroll.y + height {
                    editor.scroll.y = top + LINE_HEIGHT + margin - height;
                }
            }
            let max_y = ((line_count.saturating_sub(1)) as f32 * LINE_HEIGHT).max(0.0);
            editor.scroll.y = editor.scroll.y.clamp(0.0, max_y);

            let first_line = (editor.scroll.y / LINE_HEIGHT).floor() as usize;
            let visible = (height / LINE_HEIGHT).ceil() as usize + 1;
            let last_line = (first_line + visible).min(line_count).saturating_sub(1);
            let complete = editor.highlighter.is_plain()
                || editor
                    .highlighter
                    .ensure(last_line, HIGHLIGHT_BUDGET, |line| buffer.line(line));
            if !complete {
                window.request_animation_frame();
            }

            let first_top = bounds.top() - px(editor.scroll.y - first_line as f32 * LINE_HEIGHT);
            let mut layout_lines = Vec::new();
            let mut lines = Vec::new();
            let mut numbers = Vec::new();
            let mut widest = px(0.0);
            for (row, line) in (first_line..=last_line).enumerate() {
                let source = buffer.line(line);
                let display = DisplayLine::new(&source);
                let runs = runs_for(
                    &source,
                    &display,
                    editor.highlighter.tokens(line),
                    &font,
                    theme,
                );
                let shaped = shape(display.text.clone(), &runs);
                widest = widest.max(shaped.width());
                let top = first_top + line_height * row as f32;
                let number = (line + 1).to_string();
                let color = if line == cursor_line {
                    theme.TEXT
                } else {
                    theme.CODE_GUTTER
                };
                let number_shaped = shape(number.clone(), &[plain_run(number.len(), color)]);
                numbers.push((number_shaped, top));
                layout_lines.push((shaped, display));
                lines.push(top);
            }

            // Horizontal scroll: reveal the caret, clamp to the widest line.
            let cursor_x = (cursor_line >= first_line && cursor_line <= last_line).then(|| {
                let (shaped, display) = &layout_lines[cursor_line - first_line];
                shaped.x_for_index(display.byte_for_column(cursor_column))
            });
            if editor.reveal_cursor
                && let Some(x) = cursor_x
            {
                let x = f32::from(x);
                let view = f32::from(text_width) - 16.0;
                if x < editor.scroll.x {
                    editor.scroll.x = (x - 32.0).max(0.0);
                } else if x > editor.scroll.x + view {
                    editor.scroll.x = x - view + 32.0;
                }
            }
            editor.reveal_cursor = false;
            let max_x = (f32::from(widest) - f32::from(text_width) + 32.0).max(0.0);
            editor.scroll.x = editor
                .scroll
                .x
                .clamp(0.0, max_x.max(editor.scroll.x.min(max_x)));
            let text_left = bounds.left() + gutter_width + px(8.0) - px(editor.scroll.x);

            // Selection rectangles per visible line.
            let range = selection.range();
            let mut selections = Vec::new();
            if !range.is_empty() {
                let (start_line, start_column) = buffer.position(range.start);
                let (end_line, end_column) = buffer.position(range.end);
                for (row, (shaped, display)) in layout_lines.iter().enumerate() {
                    let line = first_line + row;
                    if line < start_line || line > end_line {
                        continue;
                    }
                    let from = if line == start_line {
                        shaped.x_for_index(display.byte_for_column(start_column))
                    } else {
                        px(0.0)
                    };
                    let to = if line == end_line {
                        shaped.x_for_index(display.byte_for_column(end_column))
                    } else {
                        // Show the selected line break.
                        shaped.width() + char_width
                    };
                    if to > from {
                        selections.push(Bounds::new(
                            point(text_left + from, lines[row]),
                            size(to - from, line_height),
                        ));
                    }
                }
            }
            let cursor_row = cursor_line
                .checked_sub(first_line)
                .filter(|row| *row < lines.len());
            let current_line = cursor_row.filter(|_| range.is_empty()).map(|row| {
                Bounds::new(
                    point(bounds.left(), lines[row]),
                    size(bounds.size.width, line_height),
                )
            });
            let cursor = cursor_row
                .zip(cursor_x)
                .filter(|_| focused)
                .map(|(row, x)| {
                    Bounds::new(point(text_left + x, lines[row]), size(px(2.0), line_height))
                });

            let shaped_lines = layout_lines
                .iter()
                .zip(&lines)
                .map(|((shaped, _), top)| (shaped.clone(), *top))
                .collect();
            editor.layout = Some(Layout {
                bounds,
                text_left,
                first_line,
                first_top,
                lines: layout_lines,
                line_count,
            });
            Prepaint {
                gutter_width,
                numbers,
                lines: shaped_lines,
                text_left,
                selections,
                current_line,
                cursor,
            }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = Theme::global();
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let line_height = px(LINE_HEIGHT);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(current) = prepaint.current_line {
                window.paint_quad(fill(current, to_hsla(theme.CODE_HEADER_BG)));
            }
            for selection in &prepaint.selections {
                window.paint_quad(fill(*selection, to_hsla(theme.SELECTION)));
            }
            for (shaped, top) in &prepaint.lines {
                let _ = shaped.paint(
                    point(prepaint.text_left, *top),
                    line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some(cursor) = prepaint.cursor {
                window.paint_quad(fill(cursor, to_hsla(theme.ACCENT)));
            }
            // The gutter covers horizontally scrolled text.
            window.paint_quad(fill(
                Bounds::new(
                    bounds.origin,
                    size(prepaint.gutter_width, bounds.size.height),
                ),
                to_hsla(theme.CODE_BG),
            ));
            for (number, top) in &prepaint.numbers {
                let left = bounds.left() + prepaint.gutter_width - px(14.0) - number.width();
                let _ = number.paint(
                    point(left, *top),
                    line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOD: &str = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    fn open_editor<'a>(
        text: &str,
        cx: &'a mut gpui::TestAppContext,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        Entity<CodeEditor>,
        &'a mut gpui::VisualTestContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        std::fs::write(&path, text).unwrap();
        cx.update(|cx| crate::input::bind_keys(cx));
        let open_path = path.clone();
        let (editor, vcx) = cx.add_window_view(|_, cx| CodeEditor::open(open_path, cx));
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let handle = editor.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();
        (dir, path, editor, vcx)
    }

    #[gpui::test]
    fn typing_editing_and_saving_reach_the_file(cx: &mut gpui::TestAppContext) {
        let (_dir, path, editor, cx) = open_editor("fn main() {\n}\n", cx);
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.text()),
            Some("fn main() {\n}\n".into())
        );
        assert!(cx.debug_bounds("code-editor-text").is_some());

        // Caret after the brace, Enter auto-indents, then type through IME input.
        cx.simulate_keystrokes("end");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("println!(\"ğ\");");
        cx.run_until_parked();
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.text()).unwrap(),
            "fn main() {\n    println!(\"ğ\");\n}\n"
        );
        assert!(editor.read_with(cx, |editor, _| editor.is_dirty()));

        cx.simulate_keystrokes(&format!("{MOD}-s"));
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn main() {\n    println!(\"ğ\");\n}\n"
        );
        assert!(!editor.read_with(cx, |editor, _| editor.is_dirty()));

        cx.simulate_keystrokes(&format!("{MOD}-z"));
        cx.run_until_parked();
        assert!(editor.read_with(cx, |editor, _| editor.is_dirty()));
        cx.simulate_keystrokes(&format!("{MOD}-shift-z"));
        cx.run_until_parked();
        assert!(!editor.read_with(cx, |editor, _| editor.is_dirty()));

        // Clicking the last line moves the caret there.
        let text = cx.debug_bounds("code-editor-text").unwrap();
        let line_three = point(text.left() + px(200.0), text.top() + px(LINE_HEIGHT * 2.5));
        cx.simulate_click(line_three, gpui::Modifiers::none());
        cx.run_until_parked();
        let caret = editor.read_with(cx, |editor, _| match &editor.content {
            Content::Ready(buffer) => buffer.position(buffer.selection().head),
            _ => unreachable!(),
        });
        assert_eq!(
            caret,
            (2, 1),
            "a click past the closing brace lands after it"
        );
    }

    #[gpui::test]
    fn closing_with_unsaved_changes_needs_a_second_request(cx: &mut gpui::TestAppContext) {
        let (_dir, _path, editor, cx) = open_editor("a", cx);
        assert!(
            editor.update(cx, |editor, cx| editor.confirm_close(cx)),
            "clean closes"
        );
        cx.simulate_input("b");
        cx.run_until_parked();
        assert!(!editor.update(cx, |editor, cx| editor.confirm_close(cx)));
        assert!(
            cx.debug_bounds("code-editor-save-state").is_some(),
            "the refusal is explained in the status bar"
        );
        assert!(editor.update(cx, |editor, cx| editor.confirm_close(cx)));
        // Editing again re-arms the guard.
        cx.simulate_input("c");
        cx.run_until_parked();
        assert!(!editor.update(cx, |editor, cx| editor.confirm_close(cx)));
    }

    #[gpui::test]
    fn binary_files_show_an_error_instead_of_text(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, [0u8, 159, 146, 150]).unwrap();
        let (editor, cx) = cx.add_window_view(|_, cx| CodeEditor::open(path, cx));
        cx.run_until_parked();
        assert_eq!(editor.read_with(cx, |editor, _| editor.text()), None);
        assert!(cx.debug_bounds("code-editor-error").is_some());
    }

    #[test]
    fn display_lines_expand_tabs_to_stops_and_map_columns_both_ways() {
        let line = DisplayLine::new("\ta\tbc");
        assert_eq!(line.text, "    a   bc");
        assert_eq!(line.byte_for_column(0), 0);
        assert_eq!(line.byte_for_column(1), 4);
        assert_eq!(line.byte_for_column(2), 5);
        assert_eq!(line.byte_for_column(3), 8);
        assert_eq!(line.byte_for_column(99), 10);
        assert_eq!(line.column_for_byte(2), 0, "inside a tab maps to the tab");
        assert_eq!(line.column_for_byte(8), 3);
        let unicode = DisplayLine::new("ğx");
        assert_eq!(unicode.byte_for_column(1), 2);
        assert_eq!(unicode.column_for_byte(2), 1);
    }

    #[test]
    fn highlight_runs_cover_the_display_text_exactly() {
        let theme = Theme::global();
        let font = gpui::font(theme.FONT_MONO);
        let source = "\tlet ğ = 1;";
        let display = DisplayLine::new(source);
        // "let" keyword and "1" number, as byte ranges of the source.
        let one = source.find('1').unwrap();
        let tokens = vec![(1..4, 1u8), (one..one + 1, 4u8)];
        let runs = runs_for(source, &display, &tokens, &font, theme);
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            display.text.len()
        );
        assert_eq!(runs[0].len, 4, "the expanded tab is default colored");
        assert_eq!(runs[1].len, 3);
        assert_eq!(runs[1].color, to_hsla(theme.CODE_KEYWORD));
        assert!(runs_for("", &DisplayLine::new(""), &[], &font, theme)[0].len == 0);
    }
}
