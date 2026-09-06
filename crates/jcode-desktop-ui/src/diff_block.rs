//! Lazy, stateful diff blocks shared by transcript code fences and tool review.
//!
//! Keep parsing and viewer state at the element boundary instead of rebuilding
//! it every time the surrounding transcript receives another streaming token.
use gpui::{App, IntoElement, RenderOnce, SharedString, Window};

use crate::{diff_model, diff_view::DiffView};

#[derive(IntoElement)]
pub(crate) struct DiffBlock {
    key: SharedString,
    source: String,
}

impl DiffBlock {
    pub(crate) fn new(source: &str, identity: impl AsRef<str>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hash);
        Self {
            key: format!("diff-block-{}-{:x}", identity.as_ref(), hash.finish()).into(),
            source: source.to_owned(),
        }
    }
}

impl RenderOnce for DiffBlock {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        window.use_keyed_state(self.key, cx, |_, cx| {
            let preview = diff_model::from_patch(&self.source).unwrap_or_else(|| {
                // Incomplete/unknown patch syntax must remain visible, not
                // disappear or acquire invented additions/deletions.
                diff_model::DiffPreview {
                    files: vec![diff_model::DiffFile {
                        path: "Patch".into(),
                        previous_path: None,
                        kind: "Preview".into(),
                        note: Some("Unrecognized patch format. Original text is shown.".into()),
                        hunks: vec![diff_model::DiffHunk {
                            header: String::new(),
                            lines: self
                                .source
                                .lines()
                                .map(|text| diff_model::DiffLine {
                                    kind: diff_model::LineKind::Meta,
                                    text: text.to_owned(),
                                    old_line: None,
                                    new_line: None,
                                    emphasis: None,
                                })
                                .collect(),
                        }],
                    }],
                }
            });
            DiffView::new(preview, cx)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{markdown, text_selection::TextSelection};
    use gpui::{Context, Entity, Render, div, prelude::*, px};

    struct MarkdownHost {
        source: String,
        selection: Entity<TextSelection>,
    }
    impl Render for MarkdownHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(800.)).child(markdown::render(
                &self.source,
                0,
                &self.selection,
                window,
                cx,
            ))
        }
    }

    #[gpui::test]
    fn markdown_diff_fences_keep_controls_and_state_on_repaint(cx: &mut gpui::TestAppContext) {
        let (host, vcx) = cx.add_window_view(|_, cx| MarkdownHost {
            source: format!(
                "```diff\n{}```",
                include_str!("../../../assets/previews/change-review.diff")
            ),
            selection: cx.new(TextSelection::new),
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-view").is_some());
        assert!(vcx.debug_bounds("diff-file-1").is_some());
        let split = vcx.debug_bounds("diff-split").unwrap();
        vcx.simulate_click(split.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-line-0-0-2-old").is_some());
        host.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("diff-line-0-0-2-old").is_some());
    }

    #[gpui::test]
    fn malformed_patch_fence_retains_original_text_and_copy(cx: &mut gpui::TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, cx| MarkdownHost {
            source: "```patch\nnot a patch yet\n```".into(),
            selection: cx.new(TextSelection::new),
        });
        vcx.run_until_parked();
        let copy = vcx.debug_bounds("diff-copy").unwrap();
        vcx.simulate_click(copy.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(
            vcx.read(|cx| cx.read_from_clipboard().unwrap().text().unwrap())
                .contains("not a patch yet")
        );
    }
}
