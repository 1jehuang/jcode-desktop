//! A full, numbered tool diff that counts down before folding into its file header.
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, relative, uniform_list};

use super::{DiffRow, FileDiff, numbered_rows};
use crate::theme::{Theme, to_hsla};

#[path = "edit_preview_timer.rs"]
mod timer;
use timer::{EditPreviewState, EditPreviewTimer};

#[path = "edit_preview_header.rs"]
mod header;
pub(super) use header::PreviewHeader;

#[path = "edit_preview_highlight.rs"]
mod syntax;

const LINE_HEIGHT: f32 = 22.;
const MAX_HEIGHT: f32 = 352.;

#[derive(Default)]
pub(crate) struct EditPreviews {
    views: std::cell::RefCell<std::collections::HashMap<String, gpui::Entity<TimedPreview>>>,
}

impl EditPreviews {
    pub(crate) fn view<T: 'static>(
        &self,
        key: String,
        files: Arc<Vec<FileDiff>>,
        index: usize,
        done: bool,
        header: PreviewHeader,
        cx: &mut Context<T>,
    ) -> gpui::Entity<TimedPreview> {
        let mut views = self.views.borrow_mut();
        let view = views
            .entry(key)
            .or_insert_with(|| {
                let view =
                    cx.new(|_| TimedPreview::new(files.clone(), index, done, header.clone()));
                cx.observe(&view, |_, _, cx| cx.notify()).detach();
                view
            })
            .clone();
        view.update(cx, |view, cx| {
            let changed_files = !Arc::ptr_eq(&view.files, &files) && view.files != files;
            let changed = changed_files || view.done != done || view.header != header;
            if changed_files {
                view.files = files;
                view.prepare_rows();
            }
            view.done = done;
            view.header = header;
            if changed {
                cx.notify();
            }
        });
        view
    }
}

pub(crate) struct TimedPreview {
    files: Arc<Vec<FileDiff>>,
    index: usize,
    rows: Vec<DiffRow>,
    indent: usize,
    syntax: Vec<syntax::StyledLine>,
    syntax_theme: usize,
    gutter_width: f32,
    timer: EditPreviewTimer,
    done: bool,
    header: PreviewHeader,
    last_frame: Instant,
}

impl TimedPreview {
    fn new(files: Arc<Vec<FileDiff>>, index: usize, done: bool, header: PreviewHeader) -> Self {
        let mut this = Self {
            files,
            index,
            rows: Vec::new(),
            indent: 0,
            syntax: Vec::new(),
            syntax_theme: 0,
            gutter_width: 32.,
            timer: EditPreviewTimer::new(),
            done,
            header,
            last_frame: Instant::now(),
        };
        this.prepare_rows();
        this.timer.update(done, Duration::ZERO);
        this
    }

    fn prepare_rows(&mut self) {
        let file = &self.files[self.index];
        self.syntax.clear();
        self.indent = common_indent(&file.lines);
        self.rows = numbered_rows(&file.lines);
        // Parse hunk boundaries before hiding them so numbering still resets
        // correctly, without reserving a row for hunk-position headers.
        self.rows.retain(|row| !row.text.starts_with("@@"));
        self.gutter_width = self
            .rows
            .iter()
            .flat_map(|row| [row.old.len(), row.new.len()])
            .max()
            .unwrap_or(0)
            .max(3) as f32
            * 8.
            + 8.;
    }

    pub(super) fn toggle_inline(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.timer.state(),
            EditPreviewState::Collapsed | EditPreviewState::Collapsing
        ) {
            self.timer.expand();
        } else {
            self.timer.collapse();
        }
        self.last_frame = Instant::now();
        cx.notify();
    }

    fn row(&self, index: usize) -> gpui::Div {
        let theme = Theme::global();
        let row = &self.rows[index];
        let line = &row.text;
        let base = div()
            .debug_selector(move || format!("edit-preview-line-{index}").into())
            .h(px(LINE_HEIGHT))
            .min_h(px(LINE_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .font_family(theme.FONT_MONO)
            .text_size(px(12.5))
            .line_height(px(LINE_HEIGHT));
        if line.starts_with("@@") || line.starts_with('\\') {
            let label = if line.starts_with("@@") && line.matches('@').count() == 2 {
                "Change".to_owned()
            } else {
                line.trim_start_matches("\\ ").to_owned()
            };
            return base
                .px_3()
                .text_size(px(10.))
                .text_color(theme.TEXT_DIM)
                .child(label);
        }
        let (marker, _text, tint) = if let Some(text) = line.strip_prefix('+') {
            ("+", text, Some(theme.OK))
        } else if let Some(text) = line.strip_prefix('-') {
            ("−", text, Some(theme.ERROR))
        } else {
            ("", line.strip_prefix(' ').unwrap_or(line), None)
        };
        let (plain, highlights) = &self.syntax[index];
        let gutter = |number: &str| {
            div()
                .debug_selector(move || format!("edit-preview-number-{index}").into())
                .w(px(self.gutter_width))
                .flex_none()
                .text_right()
                .pr_1()
                .text_size(px(10.))
                .text_color(theme.CODE_GUTTER)
                .child(number.to_owned())
        };
        base.when_some(tint, |el, color| el.bg(to_hsla(color).opacity(0.09)))
            .child(gutter(if row.new.is_empty() {
                &row.old
            } else {
                &row.new
            }))
            .child(
                div()
                    .w(px(22.))
                    .flex_none()
                    .text_center()
                    .text_color(tint.unwrap_or(theme.TEXT_DIM))
                    .child(marker),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pr_3()
                    .truncate()
                    .text_color(theme.CODE_TEXT)
                    .child(
                        gpui::StyledText::new(plain.clone()).with_highlights(highlights.clone()),
                    ),
            )
    }
}

impl Render for TimedPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        self.timer
            .update(self.done, now.saturating_duration_since(self.last_frame));
        self.last_frame = now;
        if self.timer.needs_animation() {
            let view = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = view.update(cx, |_, cx| cx.notify());
            });
        }
        let theme = Theme::global();
        let theme_key = theme as *const Theme as usize;
        if self.syntax.is_empty() || self.syntax_theme != theme_key {
            let file = &self.files[self.index];
            self.syntax = syntax::prepare(&file.lines, &file.path, self.indent)
                .into_iter()
                .zip(&file.lines)
                .filter(|(_, line)| !line.starts_with("@@"))
                .map(|(styled, _)| styled)
                .collect();
            self.syntax_theme = theme_key;
        }
        let expansion = self.timer.expansion_fraction();
        let countdown = self.timer.state() == EditPreviewState::Countdown;
        let open = expansion > 0.;
        let mut body = div()
            .id("edit-inline-toggle")
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_inline(cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .debug_selector(|| "edit-timed-preview".into())
            .min_w_0()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.render_header())
            .when(open, |el| {
                el.child(
                    div().h(px(2.)).w_full().bg(theme.CODE_HEADER_BG).child(
                        div()
                            .debug_selector(|| "edit-countdown-bar".into())
                            .h_full()
                            .w(relative(if countdown {
                                self.timer.remaining_fraction()
                            } else {
                                0.
                            }))
                            .bg(theme.ACCENT),
                    ),
                )
            });
        if open && !self.rows.is_empty() {
            let height = (self.rows.len() as f32 * LINE_HEIGHT).min(MAX_HEIGHT);
            body = body.child(
                div()
                    .debug_selector(|| "edit-expanded-body".into())
                    .h(px(height * expansion))
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "edit-full-lines",
                            self.rows.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                                range.map(|index| this.row(index)).collect()
                            }),
                        )
                        .h(px(height))
                        .w_full(),
                    )
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        this.timer.expand();
                        cx.notify();
                    })),
            );
        } else if open {
            body = body.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(theme.TEXT_DIM)
                    .child("No changed lines to preview."),
            );
        }
        // GPUI clips overflow to a rectangle. Only an expanded diff needs
        // clearance below its rectangular line fills for the rounded corners.
        // A collapsed card ends at the metadata row itself, with no footer.
        body.when(open && !self.header.review.failed, |el| {
            el.child(div().h(px(8.)).rounded_b_lg().bg(theme.CODE_BG))
        })
    }
}

/// Remove common leading indentation, never relative indentation. Two-column
/// tab stops and a shaping cap keep deeply nested/generated code readable.
fn common_indent(lines: &[String]) -> usize {
    lines
        .iter()
        .filter_map(|line| {
            let text = line.strip_prefix(['+', '-', ' '])?;
            if text.trim().is_empty() {
                return None;
            }
            Some(
                text.chars()
                    .take_while(|ch| matches!(ch, ' ' | '\t'))
                    .fold(0, |column, ch| {
                        column + if ch == '\t' { 2 - column % 2 } else { 1 }
                    }),
            )
        })
        .min()
        .unwrap_or(0)
}

#[cfg(test)]
fn compact_code(text: &str, indent: usize) -> String {
    let mut result = String::new();
    let mut column = 0;
    let mut leading = true;
    for (count, ch) in text.chars().enumerate() {
        if count == 1024 {
            result.push('…');
            break;
        }
        if ch == '\t' {
            let spaces = 2 - column % 2;
            for _ in 0..spaces {
                if !leading || column >= indent {
                    result.push(' ');
                }
                column += 1;
            }
        } else {
            if ch != ' ' {
                leading = false;
            }
            if !leading || column >= indent {
                result.push(ch);
            }
            column += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture() -> Arc<Vec<FileDiff>> {
        super::super::inline_files(
            "edit",
            &serde_json::json!({
                "file_path": "src/example.rs",
                "old_string": "\t\tfn label() {\n\t\t    old();\n\t\t}\n",
                "new_string": "\t\tfn label() {\n\t\t    new();\n\t\t}\n"
            })
            .to_string(),
        )
    }

    pub(super) fn header<T: 'static>(cx: &mut Context<T>) -> PreviewHeader {
        PreviewHeader {
            intent: Some("Clarify navigation".into()),
            review: crate::workspace::change_review::OpenChangeReview {
                source: cx.entity_id(),
                output: String::new(),
                name: "edit".into(),
                input: "{}".into(),
                selected: 0,
                done: true,
                failed: false,
            },
            focus: cx.focus_handle(),
        }
    }

    struct PreviewHost {
        previews: EditPreviews,
        visible: bool,
        done: bool,
        files: Arc<Vec<FileDiff>>,
    }

    impl Render for PreviewHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(420.)).when(self.visible, |el| {
                el.child(self.previews.view(
                    "stable-tool".into(),
                    self.files.clone(),
                    0,
                    self.done,
                    header(cx),
                    cx,
                ))
            })
        }
    }

    #[gpui::test]
    fn timed_diff_completion_and_collapse_survive_remount(cx: &mut gpui::TestAppContext) {
        let (host, vcx) = cx.add_window_view(|_, _| PreviewHost {
            previews: EditPreviews::default(),
            visible: true,
            done: false,
            files: fixture(),
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("edit-keep-open").is_none());
        let view = host.read_with(vcx, |host, _| {
            host.previews.views.borrow()["stable-tool"].clone()
        });
        host.update(vcx, |host, cx| {
            host.done = true;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(view.read_with(vcx, |view, _| view.timer.state()) == EditPreviewState::Countdown);
        view.update(vcx, |view, cx| {
            view.timer.update(true, Duration::from_secs(10));
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("edit-expanded-body").is_none());
        let preview = vcx.debug_bounds("edit-timed-preview").unwrap();
        let metadata = vcx.debug_bounds("edit-preview-metadata-0").unwrap();
        assert_eq!(
            preview.bottom(),
            metadata.bottom(),
            "no third row below a collapsed edit"
        );
        assert_eq!(preview.size.height, px(52.));
        assert!(vcx.debug_bounds("edit-countdown-bar").is_none());
        host.update(vcx, |host, cx| {
            host.visible = false;
            cx.notify();
        });
        vcx.run_until_parked();
        host.update(vcx, |host, cx| {
            host.visible = true;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("edit-expanded-body").is_none());
        assert_eq!(
            view.entity_id(),
            host.read_with(vcx, |host, _| host.previews.views.borrow()["stable-tool"]
                .entity_id())
        );
    }

    #[gpui::test]
    fn timed_diff_bar_drains_then_body_collapses_and_reopens(cx: &mut gpui::TestAppContext) {
        let (view, vcx) =
            cx.add_window_view(|_, cx| TimedPreview::new(fixture(), 0, true, header(cx)));
        vcx.run_until_parked();
        let width = vcx.debug_bounds("edit-countdown-bar").unwrap().size.width;
        let height = vcx.debug_bounds("edit-expanded-body").unwrap().size.height;
        assert!(vcx.debug_bounds("edit-preview-number-1").is_some());
        view.update(vcx, |view, cx| {
            view.timer.update(true, Duration::from_millis(2500));
            view.last_frame = Instant::now();
            cx.notify();
        });
        vcx.run_until_parked();
        let half = vcx.debug_bounds("edit-countdown-bar").unwrap().size.width;
        assert!(half < width * 0.55 && half > width * 0.4);
        assert_eq!(
            vcx.debug_bounds("edit-expanded-body").unwrap().size.height,
            height
        );
        view.update(vcx, |view, cx| {
            view.timer.update(true, Duration::from_millis(2600));
            view.last_frame = Instant::now();
            cx.notify();
        });
        vcx.run_until_parked();
        let shrinking = vcx.debug_bounds("edit-expanded-body").unwrap().size.height;
        assert!(shrinking < height && shrinking > px(0.));
        view.update(vcx, |view, cx| {
            view.timer.update(true, Duration::from_secs(1));
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("edit-expanded-body").is_none());
        let toggle = vcx.debug_bounds("edit-preview-intent-0").unwrap();
        vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("edit-expanded-body").is_some());
        view.update(vcx, |view, cx| {
            view.timer.update(true, Duration::from_secs(30));
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(
            view.read_with(vcx, |view, _| view.timer.state()),
            EditPreviewState::PinnedOpen
        );
        assert!(vcx.debug_bounds("edit-expanded-body").is_some());
    }

    #[gpui::test]
    fn timed_diff_card_click_collapses_and_file_click_reopens(cx: &mut gpui::TestAppContext) {
        let (view, vcx) =
            cx.add_window_view(|_, cx| TimedPreview::new(fixture(), 0, true, header(cx)));
        vcx.run_until_parked();
        let intent = vcx.debug_bounds("edit-preview-intent-0").unwrap();
        vcx.simulate_click(intent.center(), gpui::Modifiers::default());
        view.update(vcx, |view, cx| {
            assert_eq!(view.timer.state(), EditPreviewState::Collapsing);
            view.timer.update(true, Duration::from_secs(1));
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("edit-expanded-body").is_none());
        let path = vcx.debug_bounds("diff-file-0").unwrap();
        vcx.simulate_click(path.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            view.read_with(vcx, |view, _| view.timer.state()),
            EditPreviewState::PinnedOpen
        );
        assert!(vcx.debug_bounds("edit-expanded-body").is_some());
    }

    #[test]
    fn compact_indentation_preserves_relative_whitespace_and_source() {
        let lines = vec![
            "-\t\tif ready {".into(),
            "+\t\t  run();".into(),
            " \t\t}".into(),
        ];
        assert_eq!(common_indent(&lines), 4);
        assert_eq!(compact_code("\t\tif ready {", 4), "if ready {");
        assert_eq!(compact_code("\t\t  run();", 4), "  run();");
        assert_eq!(lines[0], "-\t\tif ready {");
        assert_eq!(compact_code(" a\tb", 0), " a  b");
        assert_eq!(
            compact_code("界".repeat(2048).as_str(), 0).chars().count(),
            1025
        );
    }
}
