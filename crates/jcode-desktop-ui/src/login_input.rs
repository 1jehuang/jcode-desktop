//! Ephemeral secret input. Only bullets reach GPUI's text layout and input readback.
//! No history, undo, persistence, copy, or cut support. This is not locked memory.
use crate::theme::{Theme, to_hsla};
use gpui::{
    App, Bounds, Context, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, GlobalElementId, LayoutId, MouseButton, PaintQuad, Pixels, SharedString, Style,
    TextRun, UTF16Selection, Window, WrappedLine, div, fill, point, prelude::*, px, relative, size,
};
use std::ops::Range;

#[derive(Default)]
struct Secret {
    text: String,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    reversed: bool,
}
impl Secret {
    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }
    fn move_to(&mut self, offset: usize, extend: bool) {
        let anchor = if !extend {
            offset
        } else if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        self.selection = anchor.min(offset)..anchor.max(offset);
        self.reversed = offset < anchor;
    }
    fn masked(&self) -> String {
        "•".repeat(self.text.encode_utf16().count())
    }
    fn utf16(&self, offset: usize) -> usize {
        self.text[..offset].encode_utf16().count()
    }
    fn byte(&self, offset: usize) -> usize {
        byte_offset(&self.text, offset)
    }
    fn range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.byte(range.start);
        start..self.byte(range.end).max(start)
    }
    fn replace(&mut self, range: Range<usize>, text: &str) {
        self.text.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.move_to(end, false);
        self.marked = None;
    }
    fn take(&mut self) -> String {
        self.move_to(0, false);
        self.marked = None;
        std::mem::take(&mut self.text)
    }
    fn clear(&mut self) {
        self.take().into_bytes().fill(0);
    }
}
fn byte_offset(text: &str, offset: usize) -> usize {
    let mut count = 0;
    for (index, ch) in text.char_indices() {
        if count + ch.len_utf16() > offset {
            return index;
        }
        count += ch.len_utf16();
    }
    text.len()
}

pub struct LoginInput {
    pub focus_handle: FocusHandle,
    secret: Secret,
    placeholder: SharedString,
    last_layout: Option<WrappedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}
impl LoginInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<String>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            secret: Secret::default(),
            placeholder: placeholder.into().into(),
            last_layout: None,
            last_bounds: None,
        }
    }
    pub fn content_empty(&self) -> bool {
        self.secret.text.is_empty()
    }
    pub fn take(&mut self, cx: &mut Context<Self>) -> String {
        let text = self.secret.take();
        self.last_layout = None;
        cx.notify();
        text
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.secret.clear();
        self.last_layout = None;
        cx.notify();
    }
    pub fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
        self.focus_handle.focus(window, cx);
    }
    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform {
            match key {
                "v" => self.paste(window, cx),
                "a" => {
                    self.secret.selection = 0..self.secret.text.len();
                    self.secret.reversed = false;
                }
                // Explicitly swallow secret extraction and history commands.
                "c" | "x" | "z" | "y" => {}
                _ => return,
            }
        } else {
            let range = self.secret.selection.clone();
            let cursor = self.secret.cursor();
            let previous = self.secret.text[..cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
            let next = self.secret.text[cursor..]
                .chars()
                .next()
                .map_or(cursor, |ch| cursor + ch.len_utf8());
            match key {
                "backspace" => self.secret.replace(
                    if range.is_empty() {
                        previous..range.end
                    } else {
                        range
                    },
                    "",
                ),
                "delete" => self.secret.replace(
                    if range.is_empty() {
                        range.start..next
                    } else {
                        range
                    },
                    "",
                ),
                "left" | "home" => {
                    let offset = if key == "home" {
                        0
                    } else if range.is_empty() || modifiers.shift {
                        previous
                    } else {
                        range.start
                    };
                    self.secret.move_to(offset, modifiers.shift);
                }
                "right" | "end" => {
                    let offset = if key == "end" {
                        self.secret.text.len()
                    } else if range.is_empty() || modifiers.shift {
                        next
                    } else {
                        range.end
                    };
                    self.secret.move_to(offset, modifiers.shift);
                }
                _ => return,
            }
        }
        self.secret.marked = None;
        cx.stop_propagation();
        cx.notify();
    }
}
impl Drop for LoginInput {
    fn drop(&mut self) {
        self.secret.clear();
    }
}
impl Focusable for LoginInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl EntityInputHandler for LoginInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.secret.range(range);
        let range = self.secret.utf16(range.start)..self.secret.utf16(range.end);
        *actual = Some(range.clone());
        Some("•".repeat(range.len()))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.secret.utf16(self.secret.selection.start)
                ..self.secret.utf16(self.secret.selection.end),
            reversed: self.secret.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.secret
            .marked
            .as_ref()
            .map(|r| self.secret.utf16(r.start)..self.secret.utf16(r.end))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.secret.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.secret.range(r))
            .or(self.secret.marked.clone())
            .unwrap_or(self.secret.selection.clone());
        let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
        self.secret.replace(range, &text);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.secret.range(r))
            .or(self.secret.marked.clone())
            .unwrap_or(self.secret.selection.clone());
        let start = range.start;
        self.secret.replace(range, text);
        self.secret.marked = (!text.is_empty()).then_some(start..start + text.len());
        if let Some(r) = selected {
            let a = byte_offset(text, r.start);
            self.secret.selection = start + a..start + byte_offset(text, r.end).max(a);
        }
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.secret.range(range);
        let start = line.position_for_index(
            self.secret.utf16(range.start) * "•".len(),
            window.line_height(),
        )?;
        let end = line.position_for_index(
            self.secret.utf16(range.end) * "•".len(),
            window.line_height(),
        )?;
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
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self.last_bounds?.localize(&point)?;
        let index = self
            .last_layout
            .as_ref()?
            .closest_index_for_position(local, window.line_height())
            .unwrap_or_else(|i| i);
        Some((index / "•".len()).min(self.secret.text.encode_utf16().count()))
    }
}

struct TextElement {
    input: Entity<LoginInput>,
}

struct PrepaintState {
    line: Option<WrappedLine>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    text_bounds: Bounds<Pixels>,
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
        let line_count = 1;
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
        let content: SharedString = input.secret.masked().into();
        let selected_range = input.secret.utf16(input.secret.selection.start) * "•".len()
            ..input.secret.utf16(input.secret.selection.end) * "•".len();
        let cursor = input.secret.utf16(input.secret.cursor()) * "•".len();
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
        let runs = vec![run];

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_text(display_text, font_size, &runs, None, None)
            .expect("masked login text should shape")
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
        });
    }
}

impl Render for LoginInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("login-input")
            .key_context("LoginInput")
            .track_focus(&self.focus_handle)
            .w_full()
            .px_3()
            .py_2()
            .overflow_hidden()
            .cursor_text()
            .bg(to_hsla(Theme::global().BG))
            .text_color(to_hsla(Theme::global().TEXT))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window, cx);
                    if let Some(offset) = this.character_index_for_point(event.position, window, cx)
                    {
                        let offset = this.secret.byte(offset);
                        this.secret.move_to(offset, false);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(TextElement { input: cx.entity() })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn native_input_masks_layout_and_readback_and_clears(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(gpui::WindowOptions::default(), |_, cx| {
                cx.new(|cx| LoginInput::new(cx, "API key"))
            })
            .unwrap()
        });
        window
            .update(cx, |input, window, cx| {
                window.focus(&input.focus_handle, cx)
            })
            .unwrap();
        cx.simulate_input(*window, "secret-😀");
        cx.run_until_parked();
        window
            .update(cx, |input, window, cx| {
                assert!(!input.content_empty());
                assert_eq!(input.secret.masked(), "•".repeat(9));
                assert_eq!(
                    input.last_layout.as_ref().unwrap().text.as_ref(),
                    "•".repeat(9)
                );
                let mut actual = None;
                assert_eq!(
                    input.text_for_range(0..99, &mut actual, window, cx),
                    Some("•".repeat(9))
                );
                assert_eq!(actual, Some(0..9));
                assert_eq!(input.take(cx), "secret-😀");
                assert!(input.content_empty());
                input.replace_and_mark_text_in_range(None, "😀ab", Some(2..3), window, cx);
                assert_eq!(input.secret.selection, 4..5);
                input.clear(cx);
                assert!(input.content_empty());
                assert!(input.secret.marked.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn keyboard_editing_and_clipboard_do_not_export_secret(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(gpui::WindowOptions::default(), |_, cx| {
                cx.new(|cx| LoginInput::new(cx, "API key"))
            })
            .unwrap()
        });
        window
            .update(cx, |input, window, cx| {
                window.focus(&input.focus_handle, cx)
            })
            .unwrap();
        cx.simulate_input(*window, "abcd");
        let mut cx = gpui::VisualTestContext::from_window(*window, cx);
        cx.simulate_keystrokes("shift-left shift-left shift-right backspace");
        cx.run_until_parked();
        window
            .update(&mut cx, |input, _, cx| {
                assert_eq!(input.secret.text, "abc");
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("clipboard".into()));
            })
            .unwrap();
        cx.simulate_keystrokes("ctrl-a ctrl-c ctrl-x ctrl-z");
        cx.run_until_parked();
        window
            .update(&mut cx, |input, window, cx| {
                assert_eq!(input.secret.text, "abc");
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("clipboard")
                );
                input.paste(window, cx);
                assert_eq!(input.take(cx), "clipboard");
            })
            .unwrap();
    }

    #[test]
    fn secret_is_never_display_text() {
        let mut secret = Secret::default();
        secret.replace(0..0, "private-token-😀é");
        let display = secret.masked();
        assert!(display.chars().all(|ch| ch == '•'));
        assert_eq!(display.chars().count(), secret.text.encode_utf16().count());
        assert!(!display.contains("private-token"));
    }
    #[test]
    fn take_and_clear_remove_content_and_composition() {
        let mut secret = Secret::default();
        secret.replace(0..0, "secret");
        secret.marked = Some(0..6);
        assert_eq!(secret.take(), "secret");
        assert!(secret.text.is_empty());
        assert!(secret.masked().is_empty());
        assert_eq!(secret.selection, 0..0);
        assert_eq!(secret.marked, None);
        secret.replace(0..0, "another");
        secret.clear();
        assert!(secret.text.is_empty());
        assert!(secret.masked().is_empty());
    }
    #[test]
    fn utf16_ranges_preserve_unicode_boundaries() {
        let mut secret = Secret::default();
        secret.replace(0..0, "a😀é");
        assert_eq!(secret.range(1..3), 1..5);
        assert_eq!(secret.range(2..999), 1..7);
        secret.replace(secret.range(1..3), "x");
        assert_eq!(secret.text, "axé");
    }
}
