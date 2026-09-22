//! Reusable read-only text selection for GPUI text elements.
//!
//! GPUI's `StyledText` exposes its laid-out geometry but does not select text by
//! default. This controller keeps the selection state shared by all selectable
//! leaves in a transcript while each leaf retains its own styling and links.

use std::ops::Range;

use gpui::{
    App, ClipboardItem, Context, CursorStyle, Entity, FocusHandle, HighlightStyle, KeyBinding,
    MouseButton, MouseMoveEvent, MouseUpEvent, SharedString, StyledText, TextLayout, Window,
    actions, canvas, div, prelude::*,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::{Theme, to_hsla};

actions!(transcript_text, [Copy]);

const KEY_CONTEXT: &str = "TranscriptText";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-c", Copy, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-c", Copy, Some(KEY_CONTEXT)),
        KeyBinding::new("super-c", Copy, Some(KEY_CONTEXT)),
    ]);
}

#[derive(Clone, Debug)]
struct Selection {
    key: SharedString,
    text: SharedString,
    range: Range<usize>,
    reversed: bool,
    mode: SelectMode,
}

#[derive(Clone, Debug)]
enum SelectMode {
    Character,
    Word(Range<usize>),
    Line(Range<usize>),
    All,
}

/// Selection and focus shared by the selectable leaves in one transcript.
pub struct TextSelection {
    focus_handle: FocusHandle,
    selection: Option<Selection>,
    selecting: bool,
}

impl TextSelection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            selection: None,
            selecting: false,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn key_context() -> &'static str {
        KEY_CONTEXT
    }

    /// The visual highlight for a leaf, if that leaf owns the active selection.
    pub fn highlight(&self, key: &str, text_len: usize) -> Option<(Range<usize>, HighlightStyle)> {
        let selection = self.selection.as_ref()?;
        if selection.key.as_ref() != key || selection.range.is_empty() {
            return None;
        }
        let start = selection.range.start.min(text_len);
        let end = selection.range.end.min(text_len);
        (start < end).then(|| {
            (
                start..end,
                HighlightStyle {
                    background_color: Some(to_hsla(Theme::global().ACCENT_DIM)),
                    ..Default::default()
                },
            )
        })
    }

    pub fn copy(&self, cx: &mut App) {
        let Some(selection) = &self.selection else {
            return;
        };
        if selection.range.is_empty() || selection.range.end > selection.text.len() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            selection.text[selection.range.clone()].to_owned(),
        ));
    }

    pub fn finish(&mut self) {
        self.selecting = false;
    }

    fn begin(
        &mut self,
        key: SharedString,
        text: SharedString,
        offset: usize,
        click_count: usize,
        shift: bool,
    ) {
        let offset = nearest_char_boundary(&text, offset.min(text.len()));
        let (range, reversed, mode) = match click_count {
            1 if shift => {
                if let Some(previous) = self.selection.as_ref().filter(|item| item.key == key) {
                    let anchor = previous.tail();
                    (
                        anchor.min(offset)..anchor.max(offset),
                        offset < anchor,
                        SelectMode::Character,
                    )
                } else {
                    (offset..offset, false, SelectMode::Character)
                }
            }
            1 => (offset..offset, false, SelectMode::Character),
            2 => {
                let range = surrounding_word(&text, offset);
                (range.clone(), false, SelectMode::Word(range))
            }
            3 => {
                let range = surrounding_line(&text, offset);
                (range.clone(), false, SelectMode::Line(range))
            }
            _ => (0..text.len(), false, SelectMode::All),
        };
        self.selection = Some(Selection {
            key,
            text,
            range,
            reversed,
            mode,
        });
        self.selecting = true;
    }

    fn is_selecting(&self, key: &str) -> bool {
        self.selecting
            && self
                .selection
                .as_ref()
                .is_some_and(|selection| selection.key.as_ref() == key)
    }

    fn drag_to(&mut self, key: &str, offset: usize) {
        if !self.selecting {
            return;
        }
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        if selection.key.as_ref() != key {
            return;
        }
        let head = nearest_char_boundary(&selection.text, offset.min(selection.text.len()));
        selection.set_head(head);
    }
}

impl Selection {
    fn tail(&self) -> usize {
        if self.reversed {
            self.range.end
        } else {
            self.range.start
        }
    }

    fn set_head(&mut self, head: usize) {
        match &self.mode {
            SelectMode::Character => self.set_character_head(head),
            SelectMode::Word(original) => {
                let head_range = surrounding_word(&self.text, head);
                self.set_unit_head(head, original.clone(), head_range);
            }
            SelectMode::Line(original) => {
                let head_range = surrounding_line(&self.text, head);
                self.set_unit_head(head, original.clone(), head_range);
            }
            SelectMode::All => {
                self.range = 0..self.text.len();
                self.reversed = false;
            }
        }
    }

    fn set_character_head(&mut self, head: usize) {
        let tail = self.tail();
        self.range = tail.min(head)..tail.max(head);
        self.reversed = head < tail;
    }

    fn set_unit_head(&mut self, head: usize, original: Range<usize>, head_range: Range<usize>) {
        if head < original.start {
            self.range = head_range.start..original.end;
            self.reversed = true;
        } else if head >= original.end {
            self.range = original.start..head_range.end;
            self.reversed = false;
        } else {
            self.range = original;
            self.reversed = false;
        }
    }
}

/// Wrap a styled text leaf with mouse selection behavior while preserving its
/// original layout, highlights, and optional interactive child.
pub fn selectable(
    model: Entity<TextSelection>,
    key: impl Into<SharedString>,
    text: impl Into<SharedString>,
    layout: TextLayout,
    child: impl IntoElement,
    cx: &App,
) -> gpui::AnyElement {
    selectable_with_prefix(model, key, text, layout, child, 0, cx)
}

/// Select a native shaped text leaf whose display has a layout-only prefix.
/// Selection state and clipboard text always use the original source offsets.
pub(crate) fn selectable_with_prefix(
    model: Entity<TextSelection>,
    key: impl Into<SharedString>,
    text: impl Into<SharedString>,
    layout: TextLayout,
    child: impl IntoElement,
    prefix_len: usize,
    cx: &App,
) -> gpui::AnyElement {
    let key = key.into();
    let text = text.into();
    let focus_handle = model.read(cx).focus_handle();
    let element_id: SharedString = format!("selectable-text-{key}").into();

    div()
        .relative()
        .id(element_id.clone())
        .debug_selector(move || element_id.to_string())
        .cursor(CursorStyle::IBeam)
        .track_focus(&focus_handle)
        .on_mouse_down(MouseButton::Left, {
            let model = model.clone();
            let key = key.clone();
            let text = text.clone();
            let layout = layout.clone();
            move |event, window, cx| {
                let offset = source_index(
                    layout_index(&layout, event.position),
                    prefix_len,
                    text.len(),
                );
                model.update(cx, |selection, cx| {
                    selection.begin(
                        key.clone(),
                        text.clone(),
                        offset,
                        event.click_count,
                        event.modifiers.shift,
                    );
                    cx.notify();
                });
                window.focus(&focus_handle, cx);
                cx.stop_propagation();
            }
        })
        // A selection is a captured gesture, not a hover interaction. Continue
        // tracking outside this leaf and finish even if released over other UI.
        // Register during paint without adding another hitbox over links/text.
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let moved = model.clone();
                    let moved_key = key.clone();
                    let moved_layout = layout.clone();
                    let text_len = text.len();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if !phase.capture() || !moved.read(cx).is_selecting(&moved_key) {
                            return;
                        }
                        moved.update(cx, |selection, cx| {
                            if event.pressed_button == Some(MouseButton::Left) {
                                let offset = source_index(
                                    layout_index(&moved_layout, event.position),
                                    prefix_len,
                                    text_len,
                                );
                                selection.drag_to(&moved_key, offset);
                            } else {
                                // A release outside the window may not be delivered.
                                selection.finish();
                            }
                            cx.notify();
                        });
                    });
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase.capture()
                            && event.button == MouseButton::Left
                            && model.read(cx).is_selecting(&key)
                        {
                            model.update(cx, |selection, cx| {
                                selection.finish();
                                cx.notify();
                            });
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        )
        .child(child)
        .into_any_element()
}

/// Build a selectable leaf with the inherited GPUI text style.
pub fn plain(
    model: Entity<TextSelection>,
    key: impl Into<SharedString>,
    text: impl Into<SharedString>,
    window: &Window,
    cx: &App,
) -> gpui::AnyElement {
    let key = key.into();
    let text = text.into();
    let mut highlights = Vec::new();
    if let Some(highlight) = model.read(cx).highlight(&key, text.len()) {
        highlights.push(highlight);
    }
    let styled =
        StyledText::new(text.clone()).with_default_highlights(&window.text_style(), highlights);
    let layout = styled.layout().clone();
    selectable(model, key, text, layout, styled, cx)
}

fn source_index(display_index: usize, prefix_len: usize, text_len: usize) -> usize {
    display_index.saturating_sub(prefix_len).min(text_len)
}

fn layout_index(layout: &TextLayout, position: gpui::Point<gpui::Pixels>) -> usize {
    match layout.index_for_position(position) {
        Ok(index) | Err(index) => index,
    }
}

fn nearest_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn surrounding_word(text: &str, offset: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let offset = nearest_char_boundary(text, offset.min(text.len()));
    let probe = if offset == text.len() {
        text[..offset]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    } else {
        offset
    };
    text.split_word_bound_indices()
        .find_map(|(start, word)| {
            let end = start + word.len();
            (start <= probe && probe < end).then_some(start..end)
        })
        .unwrap_or(offset..offset)
}

fn surrounding_line(text: &str, offset: usize) -> Range<usize> {
    let offset = nearest_char_boundary(text, offset.min(text.len()));
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_prefix_maps_back_to_source_without_copying_spacer() {
        let text = "βeta and code";
        let prefix = 8;
        for offset in 0..=prefix {
            assert_eq!(source_index(offset, prefix, text.len()), 0);
        }
        for (offset, _) in text.char_indices() {
            assert_eq!(source_index(prefix + offset, prefix, text.len()), offset);
        }
        assert_eq!(source_index(usize::MAX, prefix, text.len()), text.len());
        let start = source_index(prefix, prefix, text.len());
        let end = source_index(prefix + "βeta".len(), prefix, text.len());
        assert_eq!(&text[start..end], "βeta");
    }

    #[test]
    fn drag_selection_preserves_utf8_boundaries_and_direction() {
        let text: SharedString = "a βeta z".into();
        let mut selection = Selection {
            key: "leaf".into(),
            text: text.clone(),
            range: 2..2,
            reversed: false,
            mode: SelectMode::Character,
        };
        selection.set_head(nearest_char_boundary(&text, 7));
        assert_eq!(selection.text.get(selection.range.clone()), Some("βeta"));
        selection.set_head(0);
        assert_eq!(selection.text.get(selection.range.clone()), Some("a "));
    }

    #[test]
    fn double_and_triple_click_select_word_and_line() {
        let text = "hello world\nnext line";
        assert_eq!(&text[surrounding_word(text, 8)], "world");
        assert_eq!(&text[surrounding_line(text, 15)], "next line");
    }
}
