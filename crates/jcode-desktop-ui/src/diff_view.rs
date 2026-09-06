//! Bounded, selectable diff cards. The model is source data, never evidence that an edit succeeded.
use std::collections::HashSet;

use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, HighlightStyle, MouseButton, Render,
    SharedString, StyledText, Window, div, prelude::*, px, relative,
};

use crate::diff_model::{DiffFile, DiffLine, DiffPreview, LineKind};
use crate::markdown::{flatten_highlights, highlight_code};
use crate::text_selection::{self, TextSelection};
use crate::theme::{Theme, to_hsla};

const PAGE_LINES: usize = 80;
const PAGE_FILES: usize = 8;
const CONTEXT_EDGE: usize = 3;
const MAX_LINE_CHARS: usize = 4096;

fn visible_line_text(text: &str) -> &str {
    if text.len() <= MAX_LINE_CHARS {
        return text;
    }
    let end = text
        .char_indices()
        .nth(MAX_LINE_CHARS)
        .map_or(text.len(), |(index, _)| index);
    &text[..end]
}

fn gutter_digits(file: &DiffFile) -> usize {
    file.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .flat_map(|line| [line.old_line, line.new_line])
        .flatten()
        .max()
        .map_or(4, |number| number.to_string().len().max(4))
}

fn line_highlights(
    line: &DiffLine,
    lang: &str,
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let visible = visible_line_text(&line.text);
    if line.kind != LineKind::Meta {
        highlight_code(visible, lang)
    } else {
        (visible.to_owned(), Vec::new())
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Row {
    Header(usize),
    Line {
        hunk: usize,
        left: Option<usize>,
        right: Option<usize>,
    },
    Fold {
        hunk: usize,
        start: usize,
        end: usize,
    },
}

/// Pair only adjacent removal/addition runs, never across context or hunk boundaries.
fn split_pairs(
    lines: &[DiffLine],
    start: usize,
    end: usize,
) -> Vec<(Option<usize>, Option<usize>)> {
    let mut rows = Vec::new();
    let mut i = start;
    while i < end {
        match lines[i].kind {
            LineKind::Removed => {
                let removed_start = i;
                while i < end && lines[i].kind == LineKind::Removed {
                    i += 1;
                }
                let added_start = i;
                while i < end && lines[i].kind == LineKind::Added {
                    i += 1;
                }
                for n in 0..(added_start - removed_start).max(i - added_start) {
                    rows.push((
                        (removed_start + n < added_start).then_some(removed_start + n),
                        (added_start + n < i).then_some(added_start + n),
                    ));
                }
            }
            LineKind::Added => {
                rows.push((None, Some(i)));
                i += 1;
            }
            LineKind::Context => {
                rows.push((Some(i), Some(i)));
                i += 1;
            }
            LineKind::Meta => {
                rows.push((Some(i), None));
                i += 1;
            }
        }
    }
    rows
}

fn view_rows(file: &DiffFile, split: bool, revealed: &HashSet<(usize, usize)>) -> Vec<Row> {
    let mut rows = Vec::new();
    for (hunk, source) in file.hunks.iter().enumerate() {
        if !source.header.is_empty() {
            rows.push(Row::Header(hunk));
        }
        let lines = &source.lines;
        let mut i = 0;
        while i < lines.len() {
            let start = i;
            let context = lines[i].kind == LineKind::Context;
            while i < lines.len() && (lines[i].kind == LineKind::Context) == context {
                i += 1;
            }
            let mut append = |start, end| {
                if split {
                    rows.extend(
                        split_pairs(lines, start, end)
                            .into_iter()
                            .map(|(left, right)| Row::Line { hunk, left, right }),
                    );
                } else {
                    rows.extend((start..end).map(|line| Row::Line {
                        hunk,
                        left: Some(line),
                        right: None,
                    }));
                }
            };
            if context && i - start > CONTEXT_EDGE * 2 + 1 && !revealed.contains(&(hunk, start)) {
                append(start, start + CONTEXT_EDGE);
                rows.push(Row::Fold {
                    hunk,
                    start,
                    end: i,
                });
                // End context always remains visible, including trailing context.
                for line in i - CONTEXT_EDGE..i {
                    rows.push(Row::Line {
                        hunk,
                        left: Some(line),
                        right: split.then_some(line),
                    });
                }
            } else {
                append(start, i);
            }
        }
    }
    rows
}

struct FileState {
    collapsed: bool,
    page: usize,
    revealed: HashSet<(usize, usize)>,
    rows: Vec<Row>,
    counts: (usize, usize),
    gutter_digits: usize,
}

#[derive(Clone)]
enum Control {
    Unified,
    Split,
    Wrap,
    Collapse(usize),
    Reveal {
        file: usize,
        hunk: usize,
        start: usize,
    },
    Page {
        file: usize,
        next: bool,
    },
    Files(bool),
    CopyDiff(Option<usize>),
    CopyPath(usize),
}

pub(crate) struct DiffView {
    preview: DiffPreview,
    files: Vec<FileState>,
    selection: Entity<TextSelection>,
    split: bool,
    wrap: bool,
    file_page: usize,
    done: bool,
    failed: bool,
    copied: Option<String>,
    counts: (usize, usize),
}

impl DiffView {
    pub(crate) fn new(preview: DiffPreview, cx: &mut Context<Self>) -> Self {
        let selection = cx.new(TextSelection::new);
        cx.observe(&selection, |_, _, cx| cx.notify()).detach();
        let files = preview
            .files
            .iter()
            .map(|file| FileState {
                collapsed: false,
                page: 0,
                revealed: HashSet::new(),
                rows: view_rows(file, false, &HashSet::new()),
                counts: file.counts(),
                gutter_digits: gutter_digits(file),
            })
            .collect();
        let counts = preview.counts();
        Self {
            preview,
            files,
            selection,
            split: false,
            wrap: true,
            file_page: 0,
            done: false,
            failed: false,
            copied: None,
            counts,
        }
    }

    pub fn set_status(&mut self, done: bool, failed: bool, cx: &mut Context<Self>) {
        if self.done != done || self.failed != failed {
            self.done = done;
            self.failed = failed;
            cx.notify();
        }
    }

    fn status(&self) -> &'static str {
        if self.failed {
            "Apply failed · proposed changes"
        } else if self.done {
            "Completed · requested changes"
        } else {
            "Proposed changes"
        }
    }

    fn activate(&mut self, control: Control, key: String, cx: &mut Context<Self>) {
        self.copied = None;
        match control {
            Control::Unified | Control::Split => {
                self.split = matches!(control, Control::Split);
                for (file, state) in self.preview.files.iter().zip(&mut self.files) {
                    state.rows = view_rows(file, self.split, &state.revealed);
                    state.page = 0;
                }
            }
            Control::Wrap => self.wrap = !self.wrap,
            Control::Collapse(file) => self.files[file].collapsed = !self.files[file].collapsed,
            Control::Reveal { file, hunk, start } => {
                let state = &mut self.files[file];
                state.revealed.insert((hunk, start));
                state.rows = view_rows(&self.preview.files[file], self.split, &state.revealed);
            }
            Control::Page { file, next } => {
                let state = &mut self.files[file];
                state.page = if next {
                    (state.page + 1).min(state.rows.len().saturating_sub(1) / PAGE_LINES)
                } else {
                    state.page.saturating_sub(1)
                };
            }
            Control::Files(next) => {
                self.file_page = if next {
                    (self.file_page + 1).min(self.files.len().saturating_sub(1) / PAGE_FILES)
                } else {
                    self.file_page.saturating_sub(1)
                };
            }
            Control::CopyDiff(file) => {
                let text = match file {
                    Some(file) => self.preview.files[file].plain_text(),
                    None => self
                        .preview
                        .files
                        .iter()
                        .map(DiffFile::plain_text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.copied = Some(key);
            }
            Control::CopyPath(file) => {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    self.preview.files[file].path.clone(),
                ));
                self.copied = Some(key);
            }
        }
        cx.notify();
    }

    fn control(
        &self,
        id: impl Into<String>,
        label: impl Into<String>,
        active: bool,
        action: Control,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = id.into();
        let selector = id.clone();
        let label = if self.copied.as_ref() == Some(&id) {
            "Copied!".to_owned()
        } else {
            label.into()
        };
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || selector.clone())
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(if active {
                Theme::global().TEXT
            } else {
                Theme::global().TEXT_DIM
            })
            .when(active, |el| el.bg(Theme::global().ACCENT_DIM))
            .hover(|el| {
                el.bg(Theme::global().HEADER_BG)
                    .text_color(Theme::global().TEXT)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.activate(action.clone(), id.clone(), cx);
                }),
            )
            .child(label)
            .into_any_element()
    }

    fn line_cell(
        &self,
        file: usize,
        hunk: usize,
        index: Option<usize>,
        side: &str,
        gutter_width: gpui::Pixels,
        cx: &App,
    ) -> AnyElement {
        let theme = Theme::global();
        let Some(index) = index else {
            return div().flex_1().min_w_0().min_h(px(19.)).into_any_element();
        };
        let source = &self.preview.files[file];
        let line = &source.hunks[hunk].lines[index];
        let (marker, tint) = match line.kind {
            LineKind::Added => ("+", Some(theme.OK)),
            LineKind::Removed => ("−", Some(theme.ERROR)),
            LineKind::Context => (" ", None),
            LineKind::Meta => ("·", None),
        };
        let key: SharedString = format!("diff-{file}-{hunk}-{index}-{side}").into();
        let lang = source.path.rsplit('.').next().unwrap_or("text");
        // Token spans are disjoint. Overlay intraline and selection spans in a linear
        // merge rather than feeding a giant minified line into the quadratic flattener.
        let (plain, syntax) = line_highlights(line, lang);
        let clipped = plain.len() < line.text.len();
        let mut overlays = Vec::new();
        if let (Some(range), Some(color)) = (&line.emphasis, tint) {
            let range = range.start.min(plain.len())..range.end.min(plain.len());
            if range.start < range.end
                && plain.is_char_boundary(range.start)
                && plain.is_char_boundary(range.end)
            {
                overlays.push((
                    range.clone(),
                    HighlightStyle {
                        background_color: Some(to_hsla(color).opacity(0.28)),
                        ..Default::default()
                    },
                ));
            }
        }
        if let Some(highlight) = self.selection.read(cx).highlight(&key, plain.len()) {
            overlays.push(highlight);
        }
        let highlights = overlay_highlights(syntax, flatten_highlights(&overlays));
        let styled = StyledText::new(plain.clone()).with_highlights(highlights);
        let layout = styled.layout().clone();
        let text =
            text_selection::selectable(self.selection.clone(), key, plain, layout, styled, cx);
        let number = |n: Option<usize>, column: &str| {
            let selector = format!("diff-gutter-{file}-{hunk}-{index}-{side}-{column}");
            div()
                .debug_selector(move || selector.clone())
                .w(gutter_width)
                .flex_none()
                .whitespace_nowrap()
                .text_right()
                .pr_1()
                .text_color(theme.CODE_GUTTER)
                .child(n.map(|n| n.to_string()).unwrap_or_default())
        };
        let row_selector = format!("diff-line-{file}-{hunk}-{index}-{side}");
        let notice_selector = format!("diff-long-line-{file}-{hunk}-{index}-{side}");
        div()
            .debug_selector(move || row_selector.clone())
            .flex()
            .flex_row()
            .items_start()
            .flex_1()
            .min_w_0()
            .min_h(px(19.))
            .when_some(tint, |el, color| el.bg(to_hsla(color).opacity(0.09)))
            .when(line.kind == LineKind::Meta, |el| {
                el.text_color(theme.TEXT_DIM)
            })
            .when(!self.split || side == "old", |el| {
                el.child(number(line.old_line, "old"))
            })
            .when(!self.split || side == "new", |el| {
                el.child(number(line.new_line, "new"))
            })
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .text_color(tint.unwrap_or(theme.TEXT_DIM))
                    .child(marker),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .pr_2()
                    .child(div().when(!self.wrap, |el| el.whitespace_nowrap()).child(text))
                    .when(clipped, |el| el.child(div()
                        .debug_selector(move || notice_selector.clone())
                        .py_1().text_size(px(10.)).text_color(theme.TEXT_DIM)
                        .child("Long line preview: first 4096 characters. Copy diff retains the full line."))),
            )
            .into_any_element()
    }

    fn file_card(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let file = &self.preview.files[index];
        let state = &self.files[index];
        let theme = Theme::global();
        let mut style = window.text_style();
        style.font_family = theme.FONT_MONO.into();
        let digits = "8".repeat(state.gutter_digits);
        let gutter_width = (window
            .text_system()
            .shape_line(
                digits.clone().into(),
                px(11.5),
                &[style.to_run(digits.len())],
                None,
            )
            .width
            + px(4.))
        .max(px(34.));
        let selector = format!("diff-file-{index}");
        let mut card = div()
            .debug_selector(move || selector.clone())
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(theme.CODE_BORDER)
            .bg(theme.CODE_BG)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_1()
                    .gap_1()
                    .bg(theme.CODE_HEADER_BG)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .child(self.control(
                                format!("diff-collapse-{index}"),
                                if state.collapsed {
                                    "▸ Expand"
                                } else {
                                    "▾ Collapse"
                                },
                                false,
                                Control::Collapse(index),
                                cx,
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .font_family(theme.FONT_MONO)
                                    .text_color(theme.TEXT)
                                    .child(file.path.clone()),
                            )
                            .child(div().text_color(theme.TEXT_DIM).child(file.kind.clone()))
                            .child(stats(state.counts)),
                    )
                    .when_some(file.previous_path.as_ref(), |el, old| {
                        el.child(
                            div()
                                .px_2()
                                .text_color(theme.TEXT_DIM)
                                .child(format!("From {old}")),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_1()
                            .child(self.control(
                                format!("diff-copy-file-{index}"),
                                "Copy diff",
                                false,
                                Control::CopyDiff(Some(index)),
                                cx,
                            ))
                            .child(self.control(
                                format!("diff-copy-path-{index}"),
                                "Copy path",
                                false,
                                Control::CopyPath(index),
                                cx,
                            )),
                    ),
            );
        if state.collapsed {
            return card.into_any_element();
        }
        if let Some(note) = &file.note {
            card = card.child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(theme.TEXT_DIM)
                    .child(note.clone()),
            );
        }
        let start = state.page * PAGE_LINES;
        let end = (start + PAGE_LINES).min(state.rows.len());
        if state.rows.len() > PAGE_LINES {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.CODE_BORDER)
                    .when(state.page > 0, |el| {
                        el.child(self.control(
                            format!("diff-prev-{index}"),
                            "Previous 80 lines",
                            false,
                            Control::Page {
                                file: index,
                                next: false,
                            },
                            cx,
                        ))
                    })
                    .child(div().px_2().text_color(theme.TEXT_DIM).child(format!(
                        "Rows {}–{} of {}",
                        start + 1,
                        end,
                        state.rows.len()
                    )))
                    .when(end < state.rows.len(), |el| {
                        el.child(self.control(
                            format!("diff-more-{index}"),
                            "Show next 80 lines",
                            false,
                            Control::Page {
                                file: index,
                                next: true,
                            },
                            cx,
                        ))
                    }),
            );
        }
        if self.split {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .text_color(theme.TEXT_DIM)
                    .child(div().flex_1().px_2().child("Old"))
                    .child(div().flex_1().px_2().child("New")),
            );
        }
        // Give the scroll viewport a genuinely wide child. Text overflow alone
        // does not contribute to GPUI's scroll extent.
        let mut width = px(0.);
        if !self.wrap {
            let mut style = window.text_style();
            style.font_family = theme.FONT_MONO.into();
            for row in &state.rows[start..end] {
                if let Row::Line { hunk, left, right } = row {
                    for line in [left, right].into_iter().flatten() {
                        let text = visible_line_text(&file.hunks[*hunk].lines[*line].text);
                        let measured = window
                            .text_system()
                            .shape_line(
                                text.to_owned().into(),
                                px(11.5),
                                &[style.to_run(text.len())],
                                None,
                            )
                            .width;
                        let gutter = gutter_width * if self.split { 1. } else { 2. } + px(26.);
                        width = width.max((measured + gutter) * if self.split { 2. } else { 1. });
                    }
                }
            }
        }
        let mut body = div()
            .debug_selector(move || format!("diff-content-{index}"))
            .w_full()
            .when(!self.wrap, |el| el.w(width).min_w(relative(1.)))
            .font_family(theme.FONT_MONO)
            .text_size(px(11.5))
            .line_height(px(19.))
            .text_color(theme.CODE_TEXT);
        for row in &state.rows[start..end] {
            body =
                body.child(match row {
                    Row::Header(hunk) => div()
                        .px_2()
                        .py_1()
                        .bg(theme.CODE_HEADER_BG)
                        .text_color(theme.TEXT_DIM)
                        .child(file.hunks[*hunk].header.clone())
                        .into_any_element(),
                    Row::Fold { hunk, start, end } => self.control(
                        format!("diff-reveal-{index}-{hunk}-{start}"),
                        format!("↕ Show {} unchanged lines", end - start - CONTEXT_EDGE * 2),
                        false,
                        Control::Reveal {
                            file: index,
                            hunk: *hunk,
                            start: *start,
                        },
                        cx,
                    ),
                    Row::Line { hunk, left, right } => {
                        if self.split {
                            div()
                                .flex()
                                .flex_row()
                                .w_full()
                                .min_w_0()
                                .child(
                                    div()
                                        .w(relative(0.5))
                                        .min_w_0()
                                        .flex()
                                        .border_r_1()
                                        .border_color(theme.CODE_BORDER)
                                        .child(self.line_cell(
                                            index,
                                            *hunk,
                                            *left,
                                            "old",
                                            gutter_width,
                                            cx,
                                        )),
                                )
                                .child(div().w(relative(0.5)).min_w_0().flex().child(
                                    self.line_cell(index, *hunk, *right, "new", gutter_width, cx),
                                ))
                                .into_any_element()
                        } else {
                            self.line_cell(index, *hunk, *left, "unified", gutter_width, cx)
                        }
                    }
                });
        }
        card.child(
            div()
                .id(SharedString::from(format!("diff-scroll-{index}")))
                .debug_selector(move || format!("diff-scroll-{index}"))
                .w_full()
                .min_w_0()
                .when(!self.wrap, |el| el.overflow_x_scroll())
                .child(body),
        )
        .into_any_element()
    }
}

/// Merge two sorted, disjoint highlight lists without quadratic token work.
fn overlay_highlights(
    syntax: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    overlays: Vec<(std::ops::Range<usize>, HighlightStyle)>,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let mut bounds: Vec<_> = syntax
        .iter()
        .chain(&overlays)
        .flat_map(|(r, _)| [r.start, r.end])
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    let (mut s, mut o) = (0, 0);
    let mut result = Vec::new();
    for pair in bounds.windows(2) {
        while s < syntax.len() && syntax[s].0.end <= pair[0] {
            s += 1;
        }
        while o < overlays.len() && overlays[o].0.end <= pair[0] {
            o += 1;
        }
        let base = syntax
            .get(s)
            .filter(|(r, _)| r.start <= pair[0])
            .map(|(_, style)| *style);
        let overlay = overlays
            .get(o)
            .filter(|(r, _)| r.start <= pair[0])
            .map(|(_, style)| *style);
        let style = match (base, overlay) {
            (Some(a), Some(b)) => Some(a.highlight(b)),
            (a, b) => a.or(b),
        };
        if let Some(style) = style {
            result.push((pair[0]..pair[1], style));
        }
    }
    result
}

fn stats((added, removed): (usize, usize)) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .gap_2()
        .px_1()
        .child(
            div()
                .text_color(Theme::global().OK)
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(Theme::global().ERROR)
                .child(format!("−{removed}")),
        )
        .into_any_element()
}

impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.selection.read(cx).focus_handle();
        let mut view = div()
            .id("diff-view")
            .debug_selector(|| "diff-view".into())
            .key_context(TextSelection::key_context())
            .track_focus(&focus)
            .on_action(cx.listener(|this, _: &text_selection::Copy, _, cx| {
                this.selection
                    .update(cx, |selection, cx| selection.copy(cx));
            }))
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap_2()
            .text_size(px(11.))
            .text_color(Theme::global().TEXT)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .debug_selector(|| "diff-status".into())
                            .text_color(if self.failed {
                                Theme::global().ERROR
                            } else {
                                Theme::global().TEXT
                            })
                            .child(self.status()),
                    )
                    .child(format!(
                        "{} file{}",
                        self.files.len(),
                        if self.files.len() == 1 { "" } else { "s" }
                    ))
                    .child(stats(self.counts)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .child(self.control(
                        "diff-unified",
                        "Unified",
                        !self.split,
                        Control::Unified,
                        cx,
                    ))
                    .child(self.control("diff-split", "Split", self.split, Control::Split, cx))
                    .child(self.control(
                        "diff-wrap",
                        if self.wrap {
                            "Wrap: on"
                        } else {
                            "Wrap: off ↔"
                        },
                        self.wrap,
                        Control::Wrap,
                        cx,
                    ))
                    .child(self.control(
                        "diff-copy",
                        "Copy diff",
                        false,
                        Control::CopyDiff(None),
                        cx,
                    )),
            );
        let start = self.file_page * PAGE_FILES;
        let end = (start + PAGE_FILES).min(self.files.len());
        if self.files.len() > PAGE_FILES {
            view = view.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .when(start > 0, |el| {
                        el.child(self.control(
                            "diff-files-prev",
                            "Previous files",
                            false,
                            Control::Files(false),
                            cx,
                        ))
                    })
                    .child(format!(
                        "Files {}–{} of {}",
                        start + 1,
                        end,
                        self.files.len()
                    ))
                    .when(end < self.files.len(), |el| {
                        el.child(self.control(
                            "diff-files-next",
                            "Show next files",
                            false,
                            Control::Files(true),
                            cx,
                        ))
                    }),
            );
        }
        for file in start..end {
            view = view.child(self.file_card(file, window, cx));
        }
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_model::DiffHunk;

    #[test]
    fn long_unicode_previews_are_bounded_without_changing_source_or_copy() {
        let mut source = line(LineKind::Added, 123456);
        source.text = format!("{}HIDDEN_TAIL", "🦀é\t".repeat(5000));
        let original = source.text.clone();
        let visible = visible_line_text(&source.text);
        assert_eq!(visible.chars().count(), MAX_LINE_CHARS);
        assert!(source.text.is_char_boundary(visible.len()));
        assert!(!visible.contains("HIDDEN_TAIL"));
        let (highlighted, spans) = line_highlights(&source, "rs");
        assert_eq!(highlighted, visible);
        assert!(spans.iter().all(|(range, _)| range.end <= visible.len()));
        let file = file(vec![source]);
        assert_eq!(gutter_digits(&file), 6);
        assert_eq!(file.hunks[0].lines[0].text, original);
        assert!(file.plain_text().contains(&original));
        assert_eq!(
            visible_line_text(&"é".repeat(MAX_LINE_CHARS))
                .chars()
                .count(),
            MAX_LINE_CHARS
        );
        assert_eq!(visible_line_text(""), "");
    }

    #[gpui::test]
    fn long_line_notice_full_copy_and_six_digit_gutters(cx: &mut gpui::TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, cx| {
            let mut source = line(LineKind::Added, 123456);
            source.text = format!("{}HIDDEN_TAIL", "🦀é".repeat(3000));
            let mut view = DiffView::new(
                DiffPreview {
                    files: vec![file(vec![source])],
                },
                cx,
            );
            view.wrap = false;
            view
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-long-line-0-0-0-unified").is_some());
        let gutter = vcx.debug_bounds("diff-gutter-0-0-0-unified-new").unwrap();
        assert!(gutter.size.width > px(34.));
        assert!(gutter.size.height <= px(19.));
        let copy = vcx.debug_bounds("diff-copy-file-0").unwrap();
        vcx.simulate_click(copy.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let copied = vcx.read(|cx| cx.read_from_clipboard().unwrap().text().unwrap());
        assert!(copied.contains("HIDDEN_TAIL"));
        assert!(copied.contains(&"🦀é".repeat(3000)));
    }

    #[test]
    fn metadata_is_not_syntax_colored_and_tabs_remain_literal() {
        let mut notice = line(LineKind::Meta, 0);
        notice.text = "\\ No newline at end of file".into();
        let (text, spans) = line_highlights(&notice, "rs");
        assert_eq!(text, notice.text);
        assert!(spans.is_empty());
        let mut code = line(LineKind::Added, 1);
        code.text = "\tlet value = \"a\tb\";".into();
        let (text, spans) = line_highlights(&code, "rs");
        assert_eq!(text, code.text);
        assert!(!spans.is_empty());
    }

    struct NarrowHost {
        view: Entity<DiffView>,
        outer_clicks: usize,
    }

    impl Render for NarrowHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("outer-tool-card")
                .w(px(320.))
                .child(self.view.clone())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.outer_clicks += 1),
                )
        }
    }

    #[gpui::test]
    fn narrow_panels_wrap_and_scroll_without_toggling_parent(cx: &mut gpui::TestAppContext) {
        let (host, vcx) = cx.add_window_view(|_, cx| {
            let mut long = line(LineKind::Added, 7);
            long.text = "let long_variable = 123; ".repeat(50);
            let view = cx.new(|cx| {
                DiffView::new(
                    DiffPreview {
                        files: vec![file(vec![long])],
                    },
                    cx,
                )
            });
            NarrowHost {
                view,
                outer_clicks: 0,
            }
        });
        vcx.run_until_parked();
        let viewport = vcx.debug_bounds("diff-scroll-0").unwrap();
        assert!(viewport.size.width <= px(320.));
        assert!(
            vcx.debug_bounds("diff-line-0-0-0-unified")
                .unwrap()
                .size
                .height
                > px(19.)
        );
        let split = vcx.debug_bounds("diff-split").unwrap();
        assert!(split.right() <= px(320.));
        vcx.simulate_click(split.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let added = vcx.debug_bounds("diff-line-0-0-0-new").unwrap();
        assert!(added.size.width <= px(160.));
        assert!(host.read_with(vcx, |host, cx| host.view.read(cx).split));
        let wrap = vcx.debug_bounds("diff-wrap").unwrap();
        vcx.simulate_click(wrap.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let viewport = vcx.debug_bounds("diff-scroll-0").unwrap();
        let content = vcx.debug_bounds("diff-content-0").unwrap();
        assert!(viewport.size.width <= px(320.));
        assert!(content.size.width > viewport.size.width * 2.);
        assert_eq!(host.read_with(vcx, |host, _| host.outer_clicks), 0);
    }

    #[test]
    fn syntax_and_intraline_backgrounds_are_composed_without_overlapping_ranges() {
        let syntax = HighlightStyle {
            color: Some(to_hsla(Theme::global().CODE_KEYWORD)),
            ..Default::default()
        };
        let emphasis = HighlightStyle {
            background_color: Some(to_hsla(Theme::global().ACCENT_DIM)),
            ..Default::default()
        };
        let merged =
            overlay_highlights(vec![(0..2, syntax), (4..8, syntax)], vec![(1..6, emphasis)]);
        assert_eq!(
            merged.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            vec![0..1, 1..2, 2..4, 4..6, 6..8]
        );
        assert!(merged[1].1.color.is_some());
        assert!(merged[1].1.background_color.is_some());
        assert!(merged[2].1.color.is_none());
        assert!(merged[2].1.background_color.is_some());
    }

    fn line(kind: LineKind, n: usize) -> DiffLine {
        let (old_line, new_line) = match kind {
            LineKind::Context => (Some(n), Some(n)),
            LineKind::Removed => (Some(n), None),
            LineKind::Added => (None, Some(n)),
            LineKind::Meta => (None, None),
        };
        DiffLine {
            kind,
            text: format!("let item_{n} = {n};"),
            old_line,
            new_line,
            emphasis: None,
        }
    }
    fn file(lines: Vec<DiffLine>) -> DiffFile {
        DiffFile {
            path: "src/test.rs".into(),
            previous_path: None,
            kind: "edit".into(),
            hunks: vec![DiffHunk {
                header: String::new(),
                lines,
            }],
            note: None,
        }
    }

    #[test]
    fn split_pairs_unequal_runs_and_keeps_context_boundaries() {
        let lines = vec![
            line(LineKind::Removed, 1),
            line(LineKind::Removed, 2),
            line(LineKind::Added, 1),
            line(LineKind::Context, 3),
            line(LineKind::Added, 4),
            line(LineKind::Meta, 0),
        ];
        assert_eq!(
            split_pairs(&lines, 0, lines.len()),
            vec![
                (Some(0), Some(2)),
                (Some(1), None),
                (Some(3), Some(3)),
                (None, Some(4)),
                (Some(5), None)
            ]
        );
    }

    #[test]
    fn folding_preserves_edges_and_reveal_restores_every_line() {
        let file = file((0..20).map(|n| line(LineKind::Context, n)).collect());
        let folded = view_rows(&file, false, &HashSet::new());
        assert_eq!(folded.len(), 7);
        assert_eq!(
            folded[3],
            Row::Fold {
                hunk: 0,
                start: 0,
                end: 20
            }
        );
        let revealed = view_rows(&file, false, &HashSet::from([(0, 0)]));
        assert_eq!(revealed.len(), 20);
        for (index, row) in revealed.iter().enumerate() {
            assert_eq!(
                *row,
                Row::Line {
                    hunk: 0,
                    left: Some(index),
                    right: None
                }
            );
        }
    }

    #[test]
    fn changed_lines_never_fold_and_hunks_never_pair() {
        let mut file = file((0..160).map(|n| line(LineKind::Removed, n)).collect());
        file.hunks.push(DiffHunk {
            header: "@@ separate @@".into(),
            lines: vec![line(LineKind::Added, 1)],
        });
        let rows = view_rows(&file, true, &HashSet::new());
        assert_eq!(rows.len(), 162);
        assert!(!rows.iter().any(|r| matches!(r, Row::Fold { .. })));
        assert_eq!(
            rows.last(),
            Some(&Row::Line {
                hunk: 1,
                left: None,
                right: Some(0)
            })
        );
    }

    #[gpui::test]
    fn controls_toggle_modes_collapse_and_copy(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            DiffView::new(
                DiffPreview {
                    files: vec![file(vec![
                        line(LineKind::Removed, 1),
                        line(LineKind::Added, 1),
                    ])],
                },
                cx,
            )
        });
        vcx.run_until_parked();
        for selector in [
            "diff-split",
            "diff-wrap",
            "diff-collapse-0",
            "diff-copy-path-0",
        ] {
            let bounds = vcx.debug_bounds(selector).expect(selector);
            vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
        }
        view.read_with(vcx, |view, cx| {
            assert!(view.split);
            assert!(!view.wrap);
            assert!(view.files[0].collapsed);
            assert_eq!(view.copied.as_deref(), Some("diff-copy-path-0"));
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "src/test.rs"
            );
            assert_eq!(view.status(), "Proposed changes");
        });
        view.update(vcx, |view, cx| view.set_status(true, false, cx));
        assert_eq!(
            view.read_with(vcx, |view, _| view.status()),
            "Completed · requested changes"
        );
        view.update(vcx, |view, cx| view.set_status(true, true, cx));
        assert_eq!(
            view.read_with(vcx, |view, _| view.status()),
            "Apply failed · proposed changes"
        );
    }

    #[gpui::test]
    fn show_more_pages_real_rows_and_previous_restores_them(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            DiffView::new(
                DiffPreview {
                    files: vec![file((0..100).map(|n| line(LineKind::Added, n)).collect())],
                },
                cx,
            )
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-line-0-0-99-unified").is_none());
        let more = vcx.debug_bounds("diff-more-0").unwrap();
        vcx.simulate_click(more.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(view.read_with(vcx, |view, _| view.files[0].page), 1);
        assert!(vcx.debug_bounds("diff-line-0-0-99-unified").is_some());
        assert!(vcx.debug_bounds("diff-line-0-0-0-unified").is_none());
        let previous = vcx.debug_bounds("diff-prev-0").unwrap();
        vcx.simulate_click(previous.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(view.read_with(vcx, |view, _| view.files[0].page), 0);
    }

    #[gpui::test]
    fn clicking_context_reveal_displays_hidden_lines(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            DiffView::new(
                DiffPreview {
                    files: vec![file((0..20).map(|n| line(LineKind::Context, n)).collect())],
                },
                cx,
            )
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-line-0-0-10-unified").is_none());
        let reveal = vcx.debug_bounds("diff-reveal-0-0-0").unwrap();
        vcx.simulate_click(reveal.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(view.read_with(vcx, |view, _| view.files[0].revealed.contains(&(0, 0))));
        assert!(vcx.debug_bounds("diff-line-0-0-10-unified").is_some());
    }

    #[gpui::test]
    fn file_pagination_and_copy_include_offscreen_changes(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            DiffView::new(
                DiffPreview {
                    files: (0..9)
                        .map(|n| {
                            let mut file = file(vec![line(LineKind::Added, n)]);
                            file.path = format!("file-{n}.rs");
                            file
                        })
                        .collect(),
                },
                cx,
            )
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-file-8").is_none());
        let next = vcx.debug_bounds("diff-files-next").unwrap();
        vcx.simulate_click(next.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-file-8").is_some());
        assert!(vcx.debug_bounds("diff-file-0").is_none());
        let copy = vcx.debug_bounds("diff-copy").unwrap();
        vcx.simulate_click(copy.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        view.read_with(vcx, |view, cx| {
            assert_eq!(view.file_page, 1);
            assert_eq!(view.copied.as_deref(), Some("diff-copy"));
            let copied = cx.read_from_clipboard().unwrap().text().unwrap();
            assert!(copied.contains("file-0.rs"));
            assert!(copied.contains("file-8.rs"));
        });
    }
}
