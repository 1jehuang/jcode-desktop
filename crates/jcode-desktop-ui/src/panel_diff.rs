//! Clickable tool-edit metadata and an isolated, read-only change review panel.
use super::*;
use crate::diff::{FileDiff, tool_diffs};

#[path = "edit_preview.rs"]
mod edit_preview;
pub(super) use edit_preview::EditPreviews;

pub(super) struct DiffReview {
    files: Arc<Vec<FileDiff>>,
    selected: usize,
    source: String,
    collapsed: HashSet<String>,
    rows: Vec<DiffRow>,
    scroll: ListState,
    rich: Option<crate::diff_review_content::ReviewContent>,
}

#[derive(Clone)]
struct DiffRow {
    old: String,
    new: String,
    text: String,
}

fn numbered_rows(lines: &[String]) -> Vec<DiffRow> {
    let (mut old, mut new) = (None, None);
    lines
        .iter()
        .map(|text| {
            if text.starts_with("@@") {
                if text.contains("snippet-relative") {
                    old = None;
                    new = None;
                    return DiffRow {
                        old: String::new(),
                        new: String::new(),
                        text: text.clone(),
                    };
                }
                let mut parts = text.split_whitespace();
                parts.next();
                old = parts
                    .next()
                    .and_then(|s| s.strip_prefix('-'))
                    .and_then(|s| s.split(',').next()?.parse::<usize>().ok());
                new = parts
                    .next()
                    .and_then(|s| s.strip_prefix('+'))
                    .and_then(|s| s.split(',').next()?.parse::<usize>().ok());
                return DiffRow {
                    old: String::new(),
                    new: String::new(),
                    text: text.clone(),
                };
            }
            let mut next = |is_old: bool| {
                let counter = if is_old { &mut old } else { &mut new };
                counter
                    .as_mut()
                    .map(|n| {
                        let label = n.to_string();
                        *n = n.saturating_add(1);
                        label
                    })
                    .unwrap_or_default()
            };
            let before = if text.starts_with('-') || text.starts_with(' ') {
                next(true)
            } else {
                String::new()
            };
            let after = if text.starts_with('+') || text.starts_with(' ') {
                next(false)
            } else {
                String::new()
            };
            DiffRow {
                old: before,
                new: after,
                text: text.clone(),
            }
        })
        .collect()
}

impl DiffReview {
    fn new(files: Arc<Vec<FileDiff>>, selected: usize, source: String) -> Self {
        let rows = numbered_rows(&files[selected].lines);
        let scroll = ListState::new(rows.len(), ListAlignment::Top, px(200.));
        Self {
            files,
            selected,
            source,
            collapsed: HashSet::new(),
            rows,
            scroll,
            rich: None,
        }
    }

    fn select(&mut self, index: usize, cx: &mut App) {
        self.selected = index;
        if let Some(rich) = &mut self.rich {
            rich.select(index, cx);
        }
        self.rows = numbered_rows(&self.files[index].lines);
        self.scroll = ListState::new(self.rows.len(), ListAlignment::Top, px(200.));
    }
}

fn counts(file: &FileDiff) -> gpui::Div {
    div()
        .flex()
        .gap_2()
        .flex_none()
        .font_family(Theme::global().FONT_MONO)
        .child(
            div()
                .text_color(Theme::global().OK)
                .child(format!("+{}", file.added())),
        )
        .child(
            div()
                .text_color(Theme::global().ERROR)
                .child(format!("−{}", file.removed())),
        )
}

fn source_label(done: bool, failed: bool) -> &'static str {
    if failed {
        "Failed · requested changes"
    } else if done {
        "Completed · requested changes"
    } else {
        "Running · proposed changes"
    }
}

type PreviewCacheEntry = (String, String, String, Arc<Vec<FileDiff>>);

thread_local! {
    // Transcript repainting must not rerun snippet comparison. Cache data only,
    // never themed elements, and bound retained arguments for large tool calls.
    static PREVIEW_CACHE: std::cell::RefCell<std::collections::VecDeque<PreviewCacheEntry>> = const {
        std::cell::RefCell::new(std::collections::VecDeque::new())
    };
}

#[cfg(test)]
fn inline_files(name: &str, input: &str) -> Arc<Vec<FileDiff>> {
    inline_result_files(name, input, "")
}

fn inline_result_files(name: &str, input: &str, output: &str) -> Arc<Vec<FileDiff>> {
    PREVIEW_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, _, _, files)) = cache.iter().find(|(tool, arguments, result, _)| {
            tool == name && arguments == input && result == output
        }) {
            return files.clone();
        }
        let mut files = tool_diffs(name, input);
        // Keep file identity/order aligned with the full-review action while
        // using the same minimal snippet comparison as the detailed viewer.
        if let Some(preview) =
            crate::diff_model::from_tool_result(name, input, output).filter(|preview| {
                preview.files.len() == files.len()
                    && preview
                        .files
                        .iter()
                        .zip(&files)
                        .all(|(a, b)| a.path == b.path)
            })
        {
            use crate::diff_model::LineKind;
            for (file, rich) in files.iter_mut().zip(preview.files) {
                file.lines = rich
                    .hunks
                    .into_iter()
                    .flat_map(|hunk| {
                        let header = if hunk.header.starts_with("@@ -") {
                            hunk.header
                        } else {
                            let old = hunk
                                .lines
                                .iter()
                                .filter_map(|line| line.old_line)
                                .collect::<Vec<_>>();
                            let new = hunk
                                .lines
                                .iter()
                                .filter_map(|line| line.new_line)
                                .collect::<Vec<_>>();
                            format!(
                                "@@ -{},{} +{},{} @@{}",
                                old.first().copied().unwrap_or(0),
                                old.len(),
                                new.first().copied().unwrap_or(0),
                                new.len(),
                                if hunk.header.contains("snippet-relative") {
                                    " (snippet-relative lines)"
                                } else {
                                    ""
                                }
                            )
                        };
                        let header = Some(header);
                        header.into_iter().chain(hunk.lines.into_iter().map(|line| {
                            let prefix = match line.kind {
                                LineKind::Added => "+",
                                LineKind::Removed => "-",
                                LineKind::Context => " ",
                                LineKind::Meta => "\\ ",
                            };
                            format!("{prefix}{}", line.text)
                        }))
                    })
                    .collect();
            }
        }
        let files = Arc::new(files);
        // Keep at most eight small calls or one oversized call. Re-parsing a
        // large write on every countdown frame is much costlier than retaining
        // its already-resident transcript arguments until another tool arrives.
        while !cache.is_empty()
            && (cache.len() >= 8
                || cache
                    .iter()
                    .map(|(_, input, output, _)| input.len() + output.len())
                    .sum::<usize>()
                    + input.len()
                    + output.len()
                    > 1024 * 1024)
        {
            cache.pop_front();
        }
        cache.push_back((
            name.to_owned(),
            input.to_owned(),
            output.to_owned(),
            files.clone(),
        ));
        files
    })
}

impl Panel {
    pub(super) fn render_edit_metadata(
        &self,
        call_id: &str,
        name: &str,
        input: &str,
        output: &str,
        done: bool,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let files = inline_result_files(name, input, if error.is_none() { output } else { "" });
        if files.is_empty() {
            return None;
        }
        let failed = error.is_some();
        let intent = serde_json::from_str::<serde_json::Value>(input)
            .ok()
            .and_then(|value| value.get("intent")?.as_str().map(str::to_owned))
            .filter(|intent| !intent.trim().is_empty());
        let mut card = div()
            .debug_selector(|| "code-edit-preview".into())
            .my_2()
            .flex()
            .flex_col()
            .gap_2();
        for index in 0..files.len() {
            let header = edit_preview::PreviewHeader {
                intent: intent.clone(),
                review: crate::workspace::change_review::OpenChangeReview {
                    source: cx.entity_id(),
                    name: name.to_owned(),
                    input: input.to_owned(),
                    output: output.to_owned(),
                    selected: index,
                    done,
                    failed,
                },
                focus: self.focus_handle.clone(),
            };
            let preview = self.edit_previews.view(
                format!("{call_id}-{index}"),
                files.clone(),
                index,
                done,
                header,
                cx,
            );
            let footer_preview = preview.clone();
            card = card.child(
                div()
                    .debug_selector(move || format!("edit-preview-card-{index}").into())
                    .min_w_0()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(Theme::global().CODE_BG)
                    .child(preview)
                    .when_some(error, |el, message| {
                        el.child(
                            div()
                                .id(("edit-preview-footer", index))
                                .debug_selector(move || {
                                    format!("edit-preview-footer-{index}").into()
                                })
                                .cursor_pointer()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_click(move |_, _, cx| {
                                    footer_preview.update(cx, |view, cx| view.toggle_inline(cx));
                                    cx.stop_propagation();
                                })
                                .rounded_b_lg()
                                .bg(Theme::global().CODE_BG)
                                .px_3()
                                .py_1()
                                .text_size(px(10.))
                                .child(
                                    div()
                                        .debug_selector(|| "tool-error".into())
                                        .pt_1()
                                        .text_color(Theme::global().ERROR)
                                        .child(message.to_owned()),
                                ),
                        )
                    }),
            );
        }
        Some(card.into_any_element())
    }

    pub(crate) fn is_change_review(&self) -> bool {
        self.session_id.starts_with("review://")
    }

    pub(crate) fn new_change_review(
        session_id: String,
        working_dir: Option<String>,
        request: &crate::workspace::change_review::OpenChangeReview,
        bridge: Bridge,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(
            session_id,
            Some("Change review".into()),
            working_dir,
            bridge,
            cx,
        );
        panel.set_change_review(request, cx);
        panel
    }

    pub(crate) fn set_change_review(
        &mut self,
        request: &crate::workspace::change_review::OpenChangeReview,
        cx: &mut Context<Self>,
    ) {
        let output = if request.failed { "" } else { &request.output };
        let files = if output.is_empty() {
            Arc::new(tool_diffs(&request.name, &request.input))
        } else {
            inline_result_files(&request.name, &request.input, output)
        };
        let source = format!(
            "{} · {}",
            request.name,
            source_label(request.done, request.failed)
        );
        let mut review = DiffReview::new(files.clone(), request.selected, source);
        // Parse structured comparisons only on open and never pair mismatched files.
        if let Some(preview) =
            crate::diff_model::from_tool_result(&request.name, &request.input, output).filter(
                |preview| {
                    preview.files.len() == files.len()
                        && preview
                            .files
                            .iter()
                            .zip(files.iter())
                            .all(|(new, old)| new.path == old.path)
                },
            )
        {
            review.rich = Some(crate::diff_review_content::ReviewContent::new(
                preview,
                request.selected,
                request.done,
                request.failed,
                cx,
            ));
        }
        self.diff_review = Some(review);
        cx.notify();
    }

    pub(super) fn close_diff_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_change_review() {
            self.focus_handle.focus(window, cx);
            window.dispatch_action(
                Box::new(crate::workspace::change_review::CloseChangeReview {
                    panel: cx.entity_id(),
                }),
                cx,
            );
            return;
        }
        self.diff_review = None;
        self.focus_input(window, cx);
        cx.notify();
    }

    pub(super) fn render_diff_review(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let review = self.diff_review.as_ref()?;
        let file = &review.files[review.selected];
        let theme = Theme::global();
        let mut tree = div()
            .id("diff-tree-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_2();
        let mut entries: Vec<_> = review.files.iter().enumerate().collect();
        entries.sort_by(|a, b| a.1.path.cmp(&b.1.path));
        let mut seen = HashSet::new();
        for (index, entry) in entries {
            let parts: Vec<_> = entry
                .path
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            let mut hidden = false;
            for depth in 0..parts.len().saturating_sub(1) {
                let path = parts[..=depth].join("/");
                if seen.insert(path.clone()) {
                    let collapsed = review.collapsed.contains(&path);
                    let selector = format!("diff-directory-{path}");
                    tree = tree.child(
                        div()
                            .id(SharedString::from(format!("diff-dir-{path}")))
                            .debug_selector(move || selector.clone().into())
                            .pl(px(10. + depth as f32 * 12.))
                            .pr_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(theme.TEXT_DIM)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.QUOTE_BG))
                            .child(format!(
                                "{} {}",
                                if collapsed { "▸" } else { "▾" },
                                parts[depth]
                            ))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(review) = &mut this.diff_review {
                                        if !review.collapsed.remove(&path) {
                                            review.collapsed.insert(path.clone());
                                        }
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    );
                }
                if review.collapsed.contains(&parts[..=depth].join("/")) {
                    hidden = true;
                    break;
                }
            }
            if hidden {
                continue;
            }
            tree = tree.child(
                div()
                    .id(("diff-tree-file", index))
                    .debug_selector(move || format!("diff-tree-file-{index}").into())
                    .pl(px(12. + parts.len().saturating_sub(1) as f32 * 12.))
                    .pr_2()
                    .py_1p5()
                    .flex()
                    .gap_2()
                    .items_center()
                    .text_size(px(11.))
                    .cursor_pointer()
                    .when(index == review.selected, |s| s.bg(theme.ACCENT_DIM))
                    .hover(|s| s.bg(theme.QUOTE_BG))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(theme.TEXT)
                            .child(parts.last().copied().unwrap_or(&entry.path).to_owned()),
                    )
                    .child(
                        div()
                            .text_color(theme.TEXT_FAINT)
                            .child(entry.kind.chars().next().unwrap_or('M').to_string()),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if let Some(review) = &mut this.diff_review {
                                review.select(index, cx);
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            );
        }
        let panel = cx.entity().downgrade();
        let diff_list = list(review.scroll.clone(), move |index, _, cx| {
            let Some(panel) = panel.upgrade() else {
                return div().into_any_element();
            };
            let Some(review) = &panel.read(cx).diff_review else {
                return div().into_any_element();
            };
            let Some(row) = review.rows.get(index) else {
                return div().into_any_element();
            };
            let theme = Theme::global();
            div()
                .min_w_full()
                .flex()
                .font_family(theme.FONT_MONO)
                .text_size(px(12.))
                .line_height(px(22.))
                .when(row.text.starts_with('+'), |s| {
                    s.bg(gpui::Hsla::from(theme.OK).opacity(0.08))
                })
                .when(row.text.starts_with('-'), |s| {
                    s.bg(gpui::Hsla::from(theme.ERROR).opacity(0.08))
                })
                .when(row.text.starts_with("@@"), |s| s.bg(theme.CODE_HEADER_BG))
                .child(gutter(row.old.clone()))
                .child(gutter(row.new.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pr_3()
                        .text_color(line_color(&row.text))
                        .child(row.text.clone()),
                )
                .into_any_element()
        })
        .size_full();
        let copy = format!("{}\n{}", file.path, file.lines.join("\n"));
        Some(div().id("diff-review").debug_selector(|| "diff-review".into())
            .absolute().inset_0().size_full().flex().flex_col().bg(theme.PANEL_BG).occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(div().flex_none().flex().items_center().gap_3().p_3().border_b_1().border_color(theme.CODE_BORDER)
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.TEXT).child("Change review"))
                    .child(div().truncate().text_size(px(10.)).text_color(theme.TEXT_DIM).child(review.source.clone())))
                .child(div().id("diff-close").debug_selector(|| "diff-close".into()).px_2().py_1().rounded_md()
                    .text_size(px(11.)).text_color(theme.TEXT_DIM).cursor_pointer().hover(|s| s.bg(theme.QUOTE_BG))
                    .child(if self.is_change_review() { "Close review · Esc" } else { "Back to chat · Esc" })
                    .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, window, cx| {
                        this.close_diff_review(window, cx); cx.stop_propagation();
                    }))))
            .child(div().flex_1().min_h_0().flex()
                .child(div().debug_selector(|| "diff-file-tree".into()).w(relative(0.25)).min_w(px(110.)).max_w(px(240.))
                    .flex_none().flex().flex_col().border_r_1().border_color(theme.CODE_BORDER).bg(theme.HEADER_BG)
                    .child(div().p_2().text_size(px(10.)).text_color(theme.TEXT_DIM).child(format!("FILES IN THIS TOOL · {}", review.files.len())))
                    .child(tree))
                .child(div().min_w_0().flex_1().flex().flex_col()
                    .when(review.rich.is_none(), |el| el.child(div().flex_none().p_3().flex().flex_col().gap_2().border_b_1().border_color(theme.CODE_BORDER)
                        .child(div().debug_selector(|| "diff-selected-path".into()).text_size(px(12.)).font_family(theme.FONT_MONO).text_color(theme.TEXT).child(file.path.clone()))
                        .when_some(file.previous_path.clone(), |el, path| el.child(div().text_size(px(10.)).text_color(theme.TEXT_DIM).child(format!("Previously {path}"))))
                        .child(div().flex().items_center().gap_3().text_size(px(11.))
                            .child(div().text_color(theme.TEXT_DIM).child(file.kind.clone())).child(counts(file))
                            .child(div().id("diff-copy")
                                        .debug_selector(|| "diff-copy".into()).ml_auto().cursor_pointer().text_color(theme.ACCENT).child("Copy diff")
                                .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone())); cx.stop_propagation();
                                })))))
                    .child(div().flex_1().min_h_0().relative().bg(theme.CODE_BG)
                        .when(review.rich.is_none(), |el| el
                            .when(review.rows.is_empty(), |el| el.child(div().p_4().text_size(px(12.)).text_color(theme.TEXT_DIM)
                                .child("No line content supplied by this tool.")))
                            .child(diff_list).child(crate::scrollbar::vertical_list(&review.scroll, "diff-scrollbar")))
                        .when_some(review.rich.as_ref(), |el, rich| el.child(rich.render())))
                    .child(div().flex_none().px_3().py_2().text_size(px(10.)).text_color(theme.TEXT_FAINT)
                        .child("Recorded tool change, not a live working-tree diff."))))
            .into_any_element())
    }
}

fn gutter(text: String) -> gpui::Div {
    div()
        .w(px(42.))
        .flex_none()
        .pr_2()
        .text_align(gpui::TextAlign::Right)
        .text_color(Theme::global().CODE_GUTTER)
        .child(text)
}

fn line_color(line: &str) -> gpui::Rgba {
    if line.starts_with('+') {
        Theme::global().OK
    } else if line.starts_with('-') {
        Theme::global().ERROR
    } else if line.starts_with("@@") {
        Theme::global().ACCENT
    } else {
        Theme::global().CODE_TEXT
    }
}

pub(super) fn fixture_items() -> Vec<Item> {
    vec![
        Item::User("Make the navigation clearer and add a regression test.".into()),
        Item::Assistant("The update touches two files. Click a card for its inline diff, or the change counts for a full review.".into()),
        Item::Tool {
            call_id: "diff-fixture".into(), name: "apply_patch".into(),
            input: serde_json::json!({"intent": "Improve navigation labels", "patch_text": "*** Begin Patch\n*** Update File: src/navigation.rs\n@@\n fn label() -> &'static str {\n-    \"Go\"\n+    \"Continue\"\n }\n*** Add File: tests/navigation.rs\n+#[test]\n+fn navigation_label_is_clear() {\n+    assert_eq!(label(), \"Continue\");\n+}\n*** End Patch"}).to_string(),
            output: "Updated src/navigation.rs and added tests/navigation.rs\n\nFile diff:\n```diff\n--- a/src/navigation.rs\n+++ b/src/navigation.rs\n@@ -42,3 +42,3 @@\n fn label() -> &'static str {\n-    \"Go\"\n+    \"Continue\"\n }\n--- /dev/null\n+++ b/tests/navigation.rs\n@@ -0,0 +1,4 @@\n+#[test]\n+fn navigation_label_is_clear() {\n+    assert_eq!(label(), \"Continue\");\n+}\n```\n".into(), done: true, error: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_replaces_snippet_offsets_with_file_positions_and_refreshes_cache() {
        let input = serde_json::json!({"file_path":"src/demo.rs", "old_string":"old();\n", "new_string":"new();\n"}).to_string();
        let pending = inline_result_files("edit", &input, "");
        assert!(
            numbered_rows(&pending[0].lines)
                .iter()
                .all(|row| row.old.is_empty() && row.new.is_empty())
        );
        let output = "Done\n\nFile diff:\n```diff\n--- a/src/demo.rs\n+++ b/src/demo.rs\n@@ -142 +142 @@\n-old();\n+new();\n```\n";
        let done = inline_result_files("edit", &input, output);
        assert!(!Arc::ptr_eq(&pending, &done));
        assert!(Arc::ptr_eq(
            &done,
            &inline_result_files("edit", &input, output)
        ));
        let rows = numbered_rows(&done[0].lines);
        assert_eq!(rows[1].old, "142");
        assert_eq!(rows[2].new, "142");
        assert!(
            !done[0]
                .lines
                .iter()
                .any(|line| line.contains("snippet-relative"))
        );
    }

    #[test]
    fn snippet_headers_reset_unknown_positions_between_real_hunks() {
        let lines = [
            "@@ -42 +42 @@",
            "+known",
            "@@ -1 +1 @@ (snippet-relative lines)",
            "+unknown",
        ]
        .map(str::to_owned);
        let rows = numbered_rows(&lines);
        assert_eq!(rows[1].new, "42");
        assert!(rows[3].new.is_empty());
    }

    #[test]
    fn inline_preview_minimizes_unchanged_code_and_reuses_parsing() {
        let input = serde_json::json!({
            "file_path": "src/example.rs",
            "old_string": "fn label() {\n    let label = \"Go\";\n    show(label);\n}\n",
            "new_string": "fn label() {\n    let label = \"Continue\";\n    show(label);\n}\n"
        })
        .to_string();
        let files = inline_files("edit", &input);
        assert_eq!(files[0].added(), 1);
        assert_eq!(files[0].removed(), 1);
        assert!(files[0].lines.iter().any(|line| line == " fn label() {"));
        assert!(Arc::ptr_eq(&files, &inline_files("edit", &input)));
        let changed = input.replace("Continue", "Next");
        let newer = inline_files("edit", &changed);
        assert!(!Arc::ptr_eq(&files, &newer));
        assert!(newer[0].lines.iter().any(|line| line.contains("Next")));
        assert!(inline_files("read", &input).is_empty());
    }

    #[test]
    fn inline_preview_retains_multiedit_boundaries_and_noop_counts() {
        let input = serde_json::json!({"file_path": "same.rs", "edits": [
            {"old_string": "same\n", "new_string": "same\n"},
            {"old_string": "one\n", "new_string": "two\n"},
            {"old_string": "three\n", "new_string": "four\n"}
        ]})
        .to_string();
        let files = inline_files("multiedit", &input);
        assert_eq!((files[0].added(), files[0].removed()), (2, 2));
        assert!(
            files[0]
                .lines
                .iter()
                .filter(|line| line.starts_with("@@"))
                .count()
                >= 2
        );
        let noop =
            serde_json::json!({"file_path": "same.rs", "old_string": "same", "new_string": "same"})
                .to_string();
        let files = inline_files("edit", &noop);
        assert_eq!((files[0].added(), files[0].removed()), (0, 0));
    }

    #[gpui::test]
    fn inline_preview_numbers_full_diff_without_expanding_long_lines(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("balanced-preview", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Tool {
                call_id: "balanced".into(), name: "edit".into(),
                input: serde_json::json!({
                    "file_path": format!("src/{}/example.rs", "long-directory/".repeat(20)),
                    "old_string": (0..30).map(|n| format!("old {n} {}\n", "界".repeat(400))).collect::<String>(),
                    "new_string": (0..30).map(|n| format!("new {n} {}\n", "界".repeat(400))).collect::<String>(),
                }).to_string(),
                output: String::new(), done: true, error: None,
            }];
            cx.notify();
        });
        vcx.run_until_parked();
        let card = vcx.debug_bounds("edit-preview-card-0").unwrap();
        for selector in [
            "edit-preview-line-1",
            "edit-preview-line-2",
            "edit-preview-line-3",
        ] {
            let line = vcx.debug_bounds(selector).unwrap();
            assert!(line.size.height >= px(22.));
            assert!(line.size.width <= card.size.width);
        }
        assert!(vcx.debug_bounds("edit-countdown-bar").is_some());
        assert!(vcx.debug_bounds("edit-preview-number-1").is_some());
        assert!(
            card.size.width < px(1000.),
            "long paths and code must not widen the panel"
        );
        let header = vcx.debug_bounds("edit-change-counts-0").unwrap();
        vcx.simulate_click(header.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-review").is_some());
        assert!(panel.read_with(vcx, |panel, _| panel.expanded_tools.is_empty()));
    }

    #[test]
    fn diff_numbers_only_claim_locations_present_in_hunks() {
        let lines = [
            "@@ -12,2 +20,3 @@",
            " unchanged",
            "-before",
            "+after",
            "+extra",
            "\\ No newline at end of file",
            "@@",
            "+unknown",
        ]
        .map(str::to_owned);
        let rows = numbered_rows(&lines);
        assert_eq!((&rows[1].old[..], &rows[1].new[..]), ("12", "20"));
        assert_eq!((&rows[2].old[..], &rows[2].new[..]), ("13", ""));
        assert_eq!((&rows[3].old[..], &rows[3].new[..]), ("", "21"));
        assert_eq!(&rows[4].new, "22");
        assert!(rows[5].old.is_empty() && rows[5].new.is_empty());
        assert!(rows[7].new.is_empty());
    }

    #[test]
    fn diff_status_does_not_present_failed_or_running_input_as_applied() {
        assert!(source_label(false, false).contains("proposed"));
        assert!(source_label(true, true).starts_with("Failed"));
        assert!(source_label(true, false).contains("requested"));
    }

    #[gpui::test]
    fn diff_review_scrolls_full_changes_without_scrolling_chat(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("large-diff", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Tool {
                call_id: "large".into(), name: "write".into(),
                input: serde_json::json!({"file_path": "src/large.rs", "content": (0..500).map(|n| format!("line {n}\n")).collect::<String>()}).to_string(),
                output: String::new(), done: true, error: None,
            }];
            cx.notify();
        });
        vcx.run_until_parked();
        let header = vcx.debug_bounds("edit-change-counts-0").unwrap();
        vcx.simulate_click(header.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let review_panel =
            workspace.read_with(vcx, |workspace, _| workspace.test_panel(1).unwrap());
        assert!(panel.read_with(vcx, |panel, _| panel.diff_review.is_none()));
        assert_eq!(
            review_panel.read_with(vcx, |p, _| p.diff_review.as_ref().unwrap().rows.len()),
            500
        );
        let before = panel.read_with(vcx, |p, _| p.test_scroll_offset_y());
        let review = vcx.debug_bounds("diff-review").unwrap();
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: review.center() + gpui::point(review.size.width / 4., px(0.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-300.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        assert_ne!(
            review_panel.read_with(vcx, |p, _| p
                .diff_review
                .as_ref()
                .unwrap()
                .rich
                .as_ref()
                .unwrap()
                .scroll()
                .unwrap()
                .offset()
                .y),
            px(0.)
        );
        assert_eq!(
            panel.read_with(vcx, |p, _| p.test_scroll_offset_y()),
            before
        );
    }

    #[gpui::test]
    fn rich_review_retains_file_modes_and_escape_after_code_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::input::bind_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("rich-review", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items = fixture_items();
            cx.notify();
        });
        vcx.run_until_parked();
        let click_position = vcx.debug_bounds("edit-change-counts-0").unwrap().center();
        vcx.simulate_click(click_position, gpui::Modifiers::default());
        vcx.run_until_parked();
        let click_position = vcx.debug_bounds("diff-split").unwrap().center();
        vcx.simulate_click(click_position, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-line-0-0-1-old").is_some());
        let click_position = vcx.debug_bounds("diff-tree-file-1").unwrap().center();
        vcx.simulate_click(click_position, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-line-0-0-0-unified").is_some());
        let click_position = vcx.debug_bounds("diff-tree-file-0").unwrap().center();
        vcx.simulate_click(click_position, gpui::Modifiers::default());
        vcx.run_until_parked();
        let code = vcx
            .debug_bounds("diff-line-0-0-1-old")
            .expect("first file retains split mode");
        vcx.simulate_click(code.center(), gpui::Modifiers::default());
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.diff_review.is_none()));
    }

    #[gpui::test]
    fn diff_click_tree_copy_and_escape_preserve_chat(cx: &mut gpui::TestAppContext) {
        cx.update(crate::input::bind_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("diff-session", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items = fixture_items();
            panel.input.update(cx, |input, cx| {
                input.set_content("keep my draft".into(), cx)
            });
            cx.notify();
        });
        vcx.run_until_parked();
        let before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        let header = vcx
            .debug_bounds("edit-change-counts-0")
            .expect("clickable change counts");
        vcx.simulate_click(header.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-review").is_some());
        assert!(vcx.debug_bounds("diff-file-tree").is_some());
        let review_panel =
            workspace.read_with(vcx, |workspace, _| workspace.test_panel(1).unwrap());
        assert!(panel.read_with(vcx, |panel, _| panel.diff_review.is_none()));
        assert!(
            panel.read_with(vcx, |p, _| p.expanded_tools.is_empty()),
            "counts click must not toggle raw JSON"
        );
        let file = vcx.debug_bounds("diff-tree-file-1").unwrap();
        vcx.simulate_click(file.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            review_panel.read_with(vcx, |p, _| p.diff_review.as_ref().unwrap().selected),
            1
        );
        let copy = vcx.debug_bounds("diff-copy").unwrap();
        vcx.simulate_click(copy.center(), gpui::Modifiers::default());
        vcx.update(|_, cx| {
            let text = cx.read_from_clipboard().unwrap().text().unwrap();
            assert!(text.contains("tests/navigation.rs"));
            assert!(text.contains("+fn navigation_label_is_clear"));
        });
        let directory = vcx.debug_bounds("diff-directory-tests").unwrap();
        vcx.simulate_click(directory.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-tree-file-1").is_none());
        vcx.simulate_click(directory.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-tree-file-1").is_some());
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |p, cx| p.input.read(cx).snapshot().content),
            "keep my draft"
        );
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |p, _| p.diff_review.is_none()));
        assert_eq!(
            panel.read_with(vcx, |p, _| p.test_scroll_offset_y()),
            before
        );
        let header = vcx.debug_bounds("edit-change-counts-1").unwrap();
        vcx.simulate_click(header.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let review_panel =
            workspace.read_with(vcx, |workspace, _| workspace.test_panel(1).unwrap());
        assert_eq!(
            review_panel.read_with(vcx, |p, _| p.diff_review.as_ref().unwrap().selected),
            1
        );
        let close = vcx.debug_bounds("diff-close").unwrap();
        vcx.simulate_click(close.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |p, _| p.diff_review.is_none()));
    }
}
