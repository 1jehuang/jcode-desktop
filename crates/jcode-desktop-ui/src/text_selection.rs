//! Reusable read-only text selection for GPUI text elements.
//!
//! GPUI's `StyledText` exposes its laid-out geometry but does not select text by
//! default. This controller keeps the selection state shared by all selectable
//! leaves in a transcript while each leaf retains its own styling and links.

use std::{
    collections::HashMap,
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Entity, FocusHandle, HighlightStyle,
    KeyBinding, ListState, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    StyledText, Task, TextLayout, Window, actions, canvas, div, prelude::*, px,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct Endpoint {
    key: SharedString,
    offset: usize,
}

struct LeafGeometry {
    bounds: Bounds<Pixels>,
    layout: TextLayout,
    prefix_len: usize,
}

/// Selection and focus shared by the selectable leaves in one transcript.
pub struct TextSelection {
    focus_handle: FocusHandle,
    selection: Option<Selection>,
    selecting: bool,
    document: Vec<(SharedString, SharedString)>,
    document_index: HashMap<SharedString, usize>,
    geometry: HashMap<SharedString, LeafGeometry>,
    surface_bounds: Option<Bounds<Pixels>>,
    cross_head: Option<Endpoint>,
    pointer: Option<Point<Pixels>>,
    last_scroll: Option<Instant>,
    /// Painted bounds of the last selected visual line, in window space.
    tail_bounds: Option<Bounds<Pixels>>,
    /// Visible union of every painted selected line, in window space.
    selection_bounds: Option<Bounds<Pixels>>,
    /// A brief confirmation beside the text that was just copied.
    copied: Option<Copied>,
}

struct Copied {
    at: Instant,
    _clear: Task<()>,
}

/// How long the "Copied" confirmation stays beside the selection.
pub(crate) const COPIED_DURATION: Duration = Duration::from_millis(1000);

impl TextSelection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            selection: None,
            selecting: false,
            document: Vec::new(),
            document_index: HashMap::new(),
            geometry: HashMap::new(),
            surface_bounds: None,
            cross_head: None,
            pointer: None,
            last_scroll: None,
            tail_bounds: None,
            selection_bounds: None,
            copied: None,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn key_context() -> &'static str {
        KEY_CONTEXT
    }

    /// A complete logical document, including rows the virtual list has never
    /// mounted. Geometry is separate and only retained for the current paint.
    pub(crate) fn set_document(&mut self, document: Vec<(SharedString, SharedString)>) {
        if self.document == document {
            return;
        }
        let index: HashMap<_, _> = document
            .iter()
            .enumerate()
            .map(|(index, (key, _))| (key.clone(), index))
            .collect();
        // Keep an anchored selection through appended streaming text, but never
        // copy a stale source snapshot after replacement, reordering or removal.
        if let Some((first, _, last, _)) = self.document_range() {
            let mut previous = None;
            let valid = self.document[first..=last].iter().all(|(key, text)| {
                let Some(&position) = index.get(key) else {
                    return false;
                };
                let ordered = previous.is_none_or(|previous| position > previous);
                previous = Some(position);
                ordered && document[position].1.starts_with(text.as_ref())
            });
            if !valid {
                self.selection = None;
                self.cross_head = None;
                self.finish();
            }
        }
        if let Some(selection) = self.selection.as_mut()
            && let Some(&position) = index.get(&selection.key)
        {
            selection.text = document[position].1.clone();
        }
        self.document = document;
        self.document_index = index;
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.selecting
    }

    fn document_drag(&self) -> bool {
        self.selecting
            && self
                .selection
                .as_ref()
                .is_some_and(|selection| self.document_index.contains_key(&selection.key))
    }

    fn document_range(&self) -> Option<(usize, usize, usize, usize)> {
        let selection = self.selection.as_ref()?;
        let &anchor_index = self.document_index.get(&selection.key)?;
        let Some(head) = &self.cross_head else {
            return Some((
                anchor_index,
                selection.range.start,
                anchor_index,
                selection.range.end,
            ));
        };
        let &head_index = self.document_index.get(&head.key)?;
        let forward = head_index >= anchor_index;
        let anchor = match &selection.mode {
            SelectMode::Character => selection.tail(),
            SelectMode::Word(range) | SelectMode::Line(range) => {
                if forward {
                    range.start
                } else {
                    range.end
                }
            }
            SelectMode::All => {
                if forward {
                    0
                } else {
                    selection.text.len()
                }
            }
        };
        Some(if forward {
            (anchor_index, anchor, head_index, head.offset)
        } else {
            (head_index, head.offset, anchor_index, anchor)
        })
    }

    fn range_for(&self, key: &str, text_len: usize) -> Option<Range<usize>> {
        if let Some((first, start, last, end)) = self.document_range() {
            let &index = self.document_index.get(key)?;
            if index < first || index > last {
                return None;
            }
            return Some(if first == last {
                start.min(text_len)..end.min(text_len)
            } else {
                (if index == first {
                    start.min(text_len)
                } else {
                    0
                })..(if index == last {
                    end.min(text_len)
                } else {
                    text_len
                })
            });
        }
        self.selection
            .as_ref()
            .filter(|selection| selection.key.as_ref() == key)
            .map(|selection| selection.range.start.min(text_len)..selection.range.end.min(text_len))
    }

    /// The selected range for this leaf, including fully selected middle blocks.
    pub fn highlight(&self, key: &str, text_len: usize) -> Option<(Range<usize>, HighlightStyle)> {
        let range = self.range_for(key, text_len)?;
        (!range.is_empty()).then(|| {
            (
                range,
                // The rounded card behind the text is painted by the leaf.
                // Keep text legible on it with the prompt card's text color.
                HighlightStyle {
                    color: Some(to_hsla(Theme::global().TEXT_USER)),
                    ..Default::default()
                },
            )
        })
    }

    fn selected_text(&self) -> Option<String> {
        if let Some((first, _, last, _)) = self.document_range() {
            let mut text = String::new();
            let mut previous: Option<&SharedString> = None;
            for (key, segment) in &self.document[first..=last] {
                let range = self.range_for(key, segment.len())?;
                let part = segment.get(range)?;
                if let Some(previous) = previous {
                    // A tool's name pill reads inline with its command.
                    text.push(if is_inline_label(previous) { ' ' } else { '\n' });
                }
                text.push_str(part);
                previous = Some(key);
            }
            return (!text.is_empty()).then_some(text);
        }
        let selection = self.selection.as_ref()?;
        (!selection.range.is_empty())
            .then(|| selection.text.get(selection.range.clone()))
            .flatten()
            .map(str::to_owned)
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.show_copied(cx);
        }
    }

    /// End a pointer gesture and, like a terminal, copy what it selected.
    fn finish_and_copy(&mut self, cx: &mut Context<Self>) {
        self.finish();
        if let Some(text) = self.selected_text() {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            cx.write_to_primary(ClipboardItem::new_string(text.clone()));
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.show_copied(cx);
        }
    }

    fn show_copied(&mut self, cx: &mut Context<Self>) {
        let at = Instant::now();
        let clear = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_DURATION).await;
            let _ = this.update(cx, |selection, cx| {
                if selection.copied.as_ref().is_some_and(|copied| copied.at == at) {
                    selection.copied = None;
                    cx.notify();
                }
            });
        });
        self.copied = Some(Copied { at, _clear: clear });
        cx.notify();
    }

    /// Whether the brief "Copied" confirmation is showing.
    pub(crate) fn copied_visible(&self) -> bool {
        self.copied.is_some() && !self.selecting
    }

    /// The leaf holding the selection's end, beside which feedback appears.
    fn tail_key(&self) -> Option<&SharedString> {
        if let Some((_, _, last, _)) = self.document_range() {
            return self.document.get(last).map(|(key, _)| key);
        }
        self.selection.as_ref().map(|selection| &selection.key)
    }

    pub fn finish(&mut self) {
        self.selecting = false;
        self.pointer = None;
        self.last_scroll = None;
    }

    fn register_geometry(
        &mut self,
        key: SharedString,
        bounds: Bounds<Pixels>,
        layout: TextLayout,
        prefix_len: usize,
    ) {
        let bounds = self
            .surface_bounds
            .map_or(bounds, |surface| bounds.intersect(&surface));
        if self.document_index.contains_key(&key)
            && bounds.size.width > px(0.)
            && bounds.size.height > px(0.)
        {
            self.geometry.insert(
                key,
                LeafGeometry {
                    bounds,
                    layout,
                    prefix_len,
                },
            );
        }
    }

    /// Resolve the nearest painted text in two dimensions, rather than clamping
    /// a captured gesture to the leaf on which it started. Document order, not
    /// painting/measurement order, decides what is selected between endpoints.
    fn drag_at(&mut self, position: Point<Pixels>) -> bool {
        self.pointer = Some(position);
        let target = self
            .geometry
            .iter()
            .min_by(|(left_key, left), (right_key, right)| {
                let distance = |bounds: &Bounds<Pixels>| {
                    let dx = f32::from(
                        (bounds.left() - position.x)
                            .max(position.x - bounds.right())
                            .max(px(0.)),
                    );
                    let dy = f32::from(
                        (bounds.top() - position.y)
                            .max(position.y - bounds.bottom())
                            .max(px(0.)),
                    );
                    // Prefer text on the pointer's visual line, then its column.
                    (dy, dx)
                };
                let l = distance(&left.bounds);
                let r = distance(&right.bounds);
                l.0.total_cmp(&r.0).then(l.1.total_cmp(&r.1)).then_with(|| {
                    self.document_index[*left_key].cmp(&self.document_index[*right_key])
                })
            })
            .map(|(key, geometry)| {
                let text = &self.document[self.document_index[key]].1;
                let offset = source_index(
                    layout_index(&geometry.layout, position),
                    geometry.prefix_len,
                    text.len(),
                );
                (key.clone(), nearest_char_boundary(text, offset))
            });
        let Some((key, offset)) = target else {
            return false;
        };
        self.drag_to_endpoint(key, offset)
    }

    fn drag_to_endpoint(&mut self, key: SharedString, offset: usize) -> bool {
        if !self.selecting {
            return false;
        }
        let previous = self.document_range();
        let Some(selection) = self.selection.as_mut() else {
            return false;
        };
        if selection.key == key {
            self.cross_head = None;
            selection.set_head(nearest_char_boundary(
                &selection.text,
                offset.min(selection.text.len()),
            ));
        } else if let Some(&index) = self.document_index.get(&key) {
            let text = &self.document[index].1;
            let offset = nearest_char_boundary(text, offset.min(text.len()));
            let offset = match &selection.mode {
                SelectMode::Word(_) | SelectMode::Line(_) => {
                    let unit = if matches!(selection.mode, SelectMode::Word(_)) {
                        surrounding_word(text, offset)
                    } else {
                        surrounding_line(text, offset)
                    };
                    if index < self.document_index[&selection.key] {
                        unit.start
                    } else {
                        unit.end
                    }
                }
                _ => offset,
            };
            self.cross_head = Some(Endpoint { key, offset });
        }
        self.document_range() != previous
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
        if shift
            && click_count == 1
            && self.selection.as_ref().is_some_and(|previous| {
                self.document_index.contains_key(&previous.key)
                    && self.document_index.contains_key(&key)
            })
        {
            self.selecting = true;
            self.drag_to_endpoint(key, offset);
            return;
        }
        self.cross_head = None;
        self.copied = None;
        self.tail_bounds = None;
        self.selection_bounds = None;
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
            && !self.document_drag()
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
    let copied = {
        let selection = model.read(cx);
        (selection.copied_visible() && selection.tail_key() == Some(&key))
            .then(|| selection.selection_bounds.or(selection.tail_bounds))
            .flatten()
            .map(copied_pill)
    };

    div()
        .relative()
        .id(element_id.clone())
        .debug_selector(move || element_id.to_string())
        .cursor(CursorStyle::IBeam)
        .track_focus(&focus_handle)
        // Leaves also appear in pinned tasks and document headers, outside a
        // transcript key context. Keep keyboard copy local to the focused leaf.
        .key_context(KEY_CONTEXT)
        .on_action({
            let model = model.clone();
            move |_: &Copy, _, cx| model.update(cx, |selection, cx| selection.copy(cx))
        })
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
                move |bounds, _, window, cx| {
                    if let Some(range) = model.read(cx).range_for(&key, text.len())
                        && !range.is_empty()
                    {
                        let lines = selected_line_bounds(
                            &layout,
                            range.start + prefix_len..range.end + prefix_len,
                            window.text_style().text_align,
                        );
                        paint_selection(&lines, window);
                        let mask = window.content_mask().bounds;
                        let visible = lines
                            .iter()
                            .map(|(line, _, _)| line.intersect(&mask))
                            .filter(|line| line.size.width > px(0.) && line.size.height > px(0.))
                            .reduce(|a, b| a.union(&b));
                        if let Some(visible) = visible {
                            model.update(cx, |selection, _| {
                                selection.selection_bounds = Some(
                                    selection
                                        .selection_bounds
                                        .map_or(visible, |bounds| bounds.union(&visible)),
                                );
                            });
                        }
                        if let Some((last, _, row_end)) = lines.last() {
                            let mut last = *last;
                            last.size.width = (*row_end - last.left()).max(last.size.width);
                            model.update(cx, |selection, _| {
                                if selection.tail_key() == Some(&key) {
                                    selection.tail_bounds = Some(last);
                                }
                            });
                        }
                    }
                    model.update(cx, |selection, _| {
                        selection.register_geometry(
                            key.clone(),
                            bounds.intersect(&window.content_mask().bounds),
                            layout.clone(),
                            prefix_len,
                        );
                    });
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
                                selection.finish_and_copy(cx);
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
        .children(copied)
        .into_any_element()
}

/// A pill centered over the copied text, drawn above other content.
fn copied_pill(selected: Bounds<Pixels>) -> gpui::AnyElement {
    let theme = Theme::global();
    let height = 22.;
    // A fixed box centered on the selection centers the pill without
    // measuring its text first.
    let slot = gpui::size(px(160.), px(height));
    let center = selected.center();
    let origin = gpui::point(center.x - slot.width / 2., center.y - slot.height / 2.);
    use gpui::AnimationExt as _;
    let pill = div()
        .debug_selector(|| "selection-copied".into())
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(height))
        .px(px(10.))
        .rounded_full()
        .bg(theme.ACCENT)
        .shadow_md()
        .font_family(theme.FONT_MONO)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(theme.BG)
        .whitespace_nowrap()
        .child("\u{2713} Copied")
        .with_animation(
            "selection-copied-pop",
            gpui::Animation::new(COPIED_DURATION),
            |pill, progress| {
                // Pop in, hold, then fade during the final fifth.
                let appear = (progress / 0.08).clamp(0., 1.);
                let fade = ((1. - progress) / 0.2).clamp(0., 1.);
                pill.opacity(appear.min(fade))
            },
        );
    gpui::deferred(
        gpui::anchored()
            .position(origin)
            .snap_to_window_with_margin(px(4.))
            .child(
                div()
                    .w(slot.width)
                    .h(slot.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(pill),
            ),
    )
    .with_priority(80)
    .into_any_element()
}

/// Keys whose text reads inline with the following segment when copied.
fn is_inline_label(key: &str) -> bool {
    key.starts_with("tool-name-")
}

/// Install once, before the text children, on the scrollable transcript. The
/// capture lives on the surface so it survives virtualization of the anchor.
pub(crate) fn surface(model: Entity<TextSelection>, list: Option<ListState>) -> gpui::AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            model.update(cx, |selection, _| {
                selection.geometry.clear();
                selection.surface_bounds = Some(bounds);
                // Leaves repaint after the surface and rebuild the union.
                selection.selection_bounds = None;
            });
            let moved = model.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if !phase.capture() || !moved.read(cx).document_drag() {
                    return;
                }
                moved.update(cx, |selection, cx| {
                    if event.pressed_button == Some(MouseButton::Left) {
                        let previous_pointer = selection.pointer;
                        if selection.drag_at(event.position)
                            || previous_pointer != Some(event.position)
                        {
                            cx.notify();
                        }
                    } else {
                        selection.finish();
                    }
                });
            });
            let released = model.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase.capture()
                    && event.button == MouseButton::Left
                    && released.read(cx).document_drag()
                {
                    released.update(cx, |selection, cx| {
                        // A fast drag may finish before its last move is delivered.
                        // A release outside the transcript keeps the last head.
                        if selection
                            .surface_bounds
                            .is_some_and(|surface| surface.contains(&event.position))
                            && selection.drag_at(event.position)
                        {
                            cx.notify();
                        }
                        selection.finish_and_copy(cx);
                    });
                }
            });
            if model.read(cx).document_drag() && model.read(cx).pointer.is_some() {
                let model = model.clone();
                let list = list.clone();
                // Resolve again after all leaves have painted. This follows a
                // stationary drag pointer while wheel/edge scrolling changes
                // the geometry under it, without an idle animation loop.
                window.on_next_frame(move |_, cx| {
                    model.update(cx, |selection, cx| {
                        if !selection.document_drag() {
                            return;
                        }
                        let Some(pointer) = selection.pointer else {
                            return;
                        };
                        let mut changed = selection.drag_at(pointer);
                        if let Some(list) = &list {
                            let viewport = list.viewport_bounds().intersect(&bounds);
                            let speed = edge_scroll_speed(pointer, viewport);
                            if speed != 0. {
                                let now = Instant::now();
                                let elapsed =
                                    selection.last_scroll.replace(now).map_or(1. / 60., |last| {
                                        now.duration_since(last).as_secs_f32().min(0.05)
                                    });
                                let before = list.logical_scroll_top();
                                let offset = -list.scroll_px_offset_for_scrollbar().y;
                                let maximum = list.max_offset_for_scrollbar().y;
                                let target =
                                    (offset + px(speed * elapsed)).max(px(0.)).min(maximum);
                                list.scroll_by(target - offset);
                                let after = list.logical_scroll_top();
                                changed |= before.item_ix != after.item_ix
                                    || before.offset_in_item != after.offset_in_item;
                            } else {
                                selection.last_scroll = None;
                            }
                        }
                        if changed {
                            cx.notify();
                        }
                    });
                });
            }
        },
    )
    .absolute()
    .size_full()
    .into_any_element()
}

const SELECTION_PAD_X: f32 = 3.;
const SELECTION_RADIUS: f32 = 6.;
const SELECTION_GAP: f32 = 2.;

/// Visual line rectangles covering a display-index range of a shaped layout.
fn selected_line_bounds(
    layout: &TextLayout,
    range: Range<usize>,
    align: gpui::TextAlign,
) -> Vec<(Bounds<Pixels>, usize, Pixels)> {
    let bounds = layout.bounds();
    let height = layout.line_height();
    let mut result = Vec::new();
    if height <= px(0.) {
        return result;
    }
    let mut y = bounds.top();
    let mut line_start = 0;
    for (logical, line) in layout.line_layouts().iter().enumerate() {
        let unwrapped = &line.unwrapped_layout;
        let ends = line
            .wrap_boundaries
            .iter()
            .map(|boundary| unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
            .chain([line.len()]);
        let mut start = 0;
        for end in ends {
            let segment = line_start + start..line_start + end;
            // A selected newline keeps a blank line visibly selected.
            let includes_break = end == line.len() && range.end > segment.end;
            let from = range.start.max(segment.start);
            let to = range.end.min(segment.end);
            if from < to || (includes_break && range.start <= segment.end) {
                let line_width = unwrapped.x_for_index(end) - unwrapped.x_for_index(start);
                let offset = match align {
                    gpui::TextAlign::Left => px(0.),
                    gpui::TextAlign::Center => (bounds.size.width - line_width) / 2.,
                    gpui::TextAlign::Right => bounds.size.width - line_width,
                };
                let base = unwrapped.x_for_index(start);
                let left = unwrapped.x_for_index(from.min(segment.end) - line_start) - base;
                let mut right = unwrapped.x_for_index(to.max(from) - line_start) - base;
                if includes_break {
                    right = right.max(left + px(4.));
                }
                result.push((
                    Bounds::new(
                        gpui::point(bounds.left() + offset + left - px(SELECTION_PAD_X), y),
                        gpui::size(right - left + px(2. * SELECTION_PAD_X), height),
                    ),
                    logical,
                    // The whole visual row's end, where feedback never
                    // covers unselected text.
                    bounds.left() + offset + line_width,
                ));
            }
            start = end;
            y += height;
        }
        line_start += line.len() + 1;
    }
    result
}

/// Paint the selection as rounded cards in the prompt card style. Soft-wrapped
/// rows of one source line form one stepped shape. Every explicit newline
/// starts a new card, so the highlight follows the text's own line structure.
fn paint_selection(lines: &[(Bounds<Pixels>, usize, Pixels)], window: &mut Window) {
    let color = Theme::global().SELECTION;
    let lines: Vec<_> = lines.iter().map(|(bounds, logical, _)| (*bounds, *logical)).collect();
    for group in selection_groups(&lines) {
        if let Some(path) = crate::prompt_background::rounded_union(&group, SELECTION_RADIUS) {
            window.paint_path(path, color);
        }
    }
}

/// Split visual rows into cards at explicit newlines and horizontal gaps.
/// A hairline inset keeps vertically adjacent cards visibly separate.
fn selection_groups(lines: &[(Bounds<Pixels>, usize)]) -> Vec<Vec<Bounds<Pixels>>> {
    let mut groups: Vec<Vec<Bounds<Pixels>>> = Vec::new();
    for (i, (bounds, logical)) in lines.iter().enumerate() {
        let joins = i > 0 && {
            let (previous, previous_logical) = &lines[i - 1];
            previous_logical == logical
                && bounds.right() > previous.left()
                && bounds.left() < previous.right()
        };
        match groups.last_mut() {
            Some(group) if joins => group.push(*bounds),
            _ => groups.push(vec![*bounds]),
        }
    }
    for group in &mut groups {
        let inset = px(SELECTION_GAP / 2.);
        if let Some(first) = group.first_mut() {
            first.origin.y += inset;
            first.size.height -= inset;
        }
        if let Some(last) = group.last_mut() {
            last.size.height -= inset;
        }
    }
    groups
}

fn edge_scroll_speed(pointer: Point<Pixels>, bounds: Bounds<Pixels>) -> f32 {
    if bounds.size.height <= px(0.) {
        return 0.;
    }
    let edge = 28_f32.min(f32::from(bounds.size.height) / 3.);
    let top = f32::from(pointer.y - bounds.top());
    let bottom = f32::from(bounds.bottom() - pointer.y);
    if top < edge {
        -900. * ((edge - top) / edge).clamp(0., 1.)
    } else if bottom < edge {
        900. * ((edge - bottom) / edge).clamp(0., 1.)
    } else {
        0.
    }
}

/// Build a selectable leaf with the inherited GPUI text style.
pub fn plain(
    model: Entity<TextSelection>,
    key: impl Into<SharedString>,
    text: impl Into<SharedString>,
    _window: &Window,
    cx: &App,
) -> gpui::AnyElement {
    let key = key.into();
    let text = text.into();
    let mut highlights = Vec::new();
    if let Some(highlight) = model.read(cx).highlight(&key, text.len()) {
        highlights.push(highlight);
    }
    // Resolve inherited fonts and colors during layout, after parent styling.
    let styled = StyledText::new(text.clone()).with_highlights(highlights);
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
    fn document() -> Vec<(SharedString, SharedString)> {
        vec![
            ("prompt".into(), "Ask βeta".into()),
            ("tool".into(), "cargo test".into()),
            ("answer".into(), "All passed".into()),
        ]
    }

    #[gpui::test]
    fn document_selection_spans_unmounted_leaves_in_both_directions(cx: &mut gpui::TestAppContext) {
        cx.new(|cx| {
            let mut selection = TextSelection::new(cx);
            selection.set_document(document());
            selection.begin("prompt".into(), "Ask βeta".into(), 4, 1, false);
            selection.drag_to_endpoint("answer".into(), 3);
            assert_eq!(
                selection.selected_text().as_deref(),
                Some("βeta\ncargo test\nAll")
            );
            assert_eq!(selection.range_for("tool", 10), Some(0..10));
            selection.begin("answer".into(), "All passed".into(), 3, 1, false);
            selection.drag_to_endpoint("prompt".into(), 4);
            assert_eq!(
                selection.selected_text().as_deref(),
                Some("βeta\ncargo test\nAll")
            );
            selection.drag_to_endpoint("answer".into(), 10);
            assert_eq!(selection.selected_text().as_deref(), Some(" passed"));
            selection
        });
    }

    #[gpui::test]
    fn shift_click_extends_across_blocks_and_word_drag_keeps_units(cx: &mut gpui::TestAppContext) {
        cx.new(|cx| {
            let mut selection = TextSelection::new(cx);
            selection.set_document(document());
            selection.begin("prompt".into(), "Ask βeta".into(), 4, 1, false);
            selection.finish();
            selection.begin("answer".into(), "All passed".into(), 3, 1, true);
            assert_eq!(
                selection.selected_text().as_deref(),
                Some("βeta\ncargo test\nAll")
            );
            selection.begin("prompt".into(), "Ask βeta".into(), 5, 2, false);
            selection.drag_to_endpoint("answer".into(), 1);
            assert_eq!(
                selection.selected_text().as_deref(),
                Some("βeta\ncargo test\nAll")
            );
            selection
        });
    }

    #[gpui::test]
    fn document_replacement_clears_stale_selection_but_stream_append_preserves_it(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.new(|cx| {
            let mut selection = TextSelection::new(cx);
            selection.set_document(document());
            selection.begin("prompt".into(), "Ask βeta".into(), 4, 1, false);
            selection.drag_to_endpoint("answer".into(), 3);
            let mut appended = document();
            appended[2].1 = "All passed today".into();
            selection.set_document(appended);
            assert_eq!(
                selection.selected_text().as_deref(),
                Some("βeta\ncargo test\nAll")
            );
            let mut replacement = document();
            replacement[1].1 = "different command".into();
            selection.set_document(replacement);
            assert!(selection.selected_text().is_none());
            assert!(!selection.is_dragging());
            selection
        });
    }

    #[test]
    fn explicit_newlines_split_selection_cards_but_soft_wraps_join() {
        let row = |x: f32, y: f32, w: f32| {
            Bounds::new(gpui::point(px(x), px(y)), gpui::size(px(w), px(20.)))
        };
        // Two soft-wrapped rows of line 0, then line 1, then line 2.
        let lines = [
            (row(0., 0., 200.), 0),
            (row(0., 20., 120.), 0),
            (row(0., 40., 180.), 1),
            (row(0., 60., 60.), 2),
        ];
        let groups = selection_groups(&lines);
        assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1, 1]);
        // Adjacent cards never touch, so each newline reads as a break.
        for pair in groups.windows(2) {
            let above = pair[0].last().unwrap().bottom();
            let below = pair[1].first().unwrap().top();
            assert!(below - above >= px(SELECTION_GAP) - px(0.01));
        }
    }

    #[gpui::test]
    fn tool_name_joins_its_command_and_copy_shows_feedback(cx: &mut gpui::TestAppContext) {
        let selection = cx.new(|cx| {
            let mut selection = TextSelection::new(cx);
            selection.set_document(vec![
                ("tool-name-1".into(), "bash".into()),
                ("tool-summary-1".into(), "cargo test".into()),
                ("2-0".into(), "Done".into()),
            ]);
            selection.begin("tool-name-1".into(), "bash".into(), 0, 1, false);
            selection.drag_to_endpoint("2-0".into(), 4);
            assert_eq!(
                selection.selected_text().as_deref(),
                Some("bash cargo test\nDone")
            );
            assert_eq!(selection.tail_key().map(|key| key.as_ref()), Some("2-0"));
            selection
        });
        selection.update(cx, |selection, cx| {
            assert!(!selection.copied_visible());
            selection.finish_and_copy(cx);
            assert!(selection.copied_visible());
        });
        cx.executor().advance_clock(COPIED_DURATION + Duration::from_millis(50));
        cx.run_until_parked();
        selection.read_with(cx, |selection, _| assert!(!selection.copied_visible()));
    }

    #[test]
    fn edge_autoscroll_is_bounded_and_does_not_run_in_the_middle() {
        let bounds = Bounds::new(
            gpui::point(px(0.), px(100.)),
            gpui::size(px(500.), px(400.)),
        );
        assert_eq!(
            edge_scroll_speed(gpui::point(px(20.), px(300.)), bounds),
            0.
        );
        assert_eq!(
            edge_scroll_speed(gpui::point(px(20.), px(80.)), bounds),
            -900.
        );
        assert_eq!(
            edge_scroll_speed(gpui::point(px(20.), px(520.)), bounds),
            900.
        );
        assert!(edge_scroll_speed(gpui::point(px(20.), px(485.)), bounds) > 0.);
    }
}
