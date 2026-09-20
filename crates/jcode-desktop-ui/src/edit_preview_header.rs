//! Two-tone edit header: intent and counts above file details.
use super::*;
use crate::workspace::change_review::OpenChangeReview;

#[derive(Clone, PartialEq)]
pub(crate) struct PreviewHeader {
    pub intent: Option<String>,
    pub review: OpenChangeReview,
    pub focus: gpui::FocusHandle,
}

impl TimedPreview {
    pub(super) fn render_header(&self) -> gpui::Div {
        let theme = Theme::global();
        let index = self.index;
        let countdown = self.timer.state() == EditPreviewState::Countdown;
        let file = &self.files[index];
        let request = self.header.review.clone();
        let focus = self.header.focus.clone();
        let path = div()
            .debug_selector(move || format!("diff-file-{index}").into())
            .min_w_0()
            .flex_1()
            .truncate()
            .font_family(theme.FONT_MONO)
            .text_color(theme.TEXT_DIM)
            .child(file.path.clone());
        let counts = super::super::counts(file)
            .id("edit-change-counts")
            .debug_selector(move || format!("edit-change-counts-{index}").into())
            .text_size(px(11.))
            .rounded_md()
            .cursor_pointer()
            .hover(|style| style.bg(theme.ACCENT_DIM))
            // The counts own their click. Never toggle the inline preview or
            // let the workspace mouse-up handler refocus the source panel.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                focus.focus(window, cx);
                window.dispatch_action(Box::new(request.clone()), cx);
                cx.stop_propagation();
            });
        let status = if self.header.review.failed {
            "Failed".to_owned()
        } else if !self.done {
            "Editing…".to_owned()
        } else if countdown {
            format!("{}s", (self.timer.remaining_fraction() * 5.).ceil() as u32)
        } else {
            String::new()
        };
        let metadata = div()
            .debug_selector(move || format!("edit-preview-metadata-{index}").into())
            .flex()
            .items_center()
            .gap_1()
            .h(px(24.))
            .px_3()
            .bg(theme.CODE_BG)
            .when(
                self.timer.expansion_fraction() == 0. && !self.header.review.failed,
                |el| el.rounded_b_lg(),
            )
            .min_w_0()
            .text_size(px(11.))
            .line_height(px(16.))
            .child(path)
            .when(!status.is_empty(), |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_color(if self.header.review.failed {
                            theme.ERROR
                        } else {
                            theme.TEXT_DIM
                        })
                        .child(status),
                )
            });
        div()
            .debug_selector(move || format!("edit-preview-header-{index}").into())
            .min_w_0()
            .child(
                div()
                    .debug_selector(move || format!("edit-preview-intent-row-{index}").into())
                    // Overflow clipping is rectangular in GPUI. Round the
                    // painted top row itself, not just its outer container.
                    .rounded_t_lg()
                    .bg(theme.CODE_HEADER_BG)
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .line_height(px(20.))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.TEXT)
                    .child(crate::tool_icon::render_status(
                        &self.header.review.name,
                        self.done,
                        self.header.review.failed,
                    ))
                    .child(
                        div()
                            .debug_selector(move || format!("edit-preview-intent-{index}").into())
                            .flex_1()
                            .min_w_0()
                            .h(px(20.))
                            .truncate()
                            .child(
                                self.header
                                    .intent
                                    .clone()
                                    .unwrap_or_else(|| "Edit file".into()),
                            ),
                    )
                    .child(counts),
            )
            .child(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn compact_header_keeps_intent_above_one_metadata_row(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            let files = super::super::super::inline_files("edit", &serde_json::json!({
                "file_path": "src/a/very/long/directory/with/a/long_filename.rs",
                "old_string": "old", "new_string": "new",
            }).to_string());
            TimedPreview::new(files, 0, true, PreviewHeader {
                intent: Some("A long intent should stay on one compact line instead of enlarging the header".into()),
                review: OpenChangeReview {
                    source: cx.entity_id(), name: "edit".into(), input: "{}".into(),
                    output: String::new(),
                    selected: 0, done: true, failed: false,
                },
                focus: cx.focus_handle(),
            })
        });
        // Exercise the real component at narrow and wide panel sizes, in every
        // state that changes the metadata row's controls or status.
        for width in [360., 420., 1120.] {
            vcx.simulate_resize(gpui::size(px(width), px(600.)));
            for state in 0..4 {
                view.update(vcx, |view, cx| {
                    view.done = state != 0;
                    view.header.review.failed = state == 3;
                    view.timer = EditPreviewTimer::new();
                    view.timer.update(view.done, Duration::ZERO);
                    if state == 2 {
                        view.timer.collapse();
                        view.timer.update(true, Duration::from_secs(1));
                    }
                    cx.notify();
                });
                vcx.run_until_parked();
                let header = vcx.debug_bounds("edit-preview-header-0").unwrap();
                let intent = vcx.debug_bounds("edit-preview-intent-0").unwrap();
                let icon = vcx
                    .debug_bounds("tool-type-icon")
                    .expect("edit icon paints");
                assert_eq!(icon.size, gpui::size(px(14.0), px(14.0)));
                assert!(icon.right() <= intent.left());
                let intent_row = vcx.debug_bounds("edit-preview-intent-row-0").unwrap();
                let counts = vcx.debug_bounds("edit-change-counts-0").unwrap();
                let metadata = vcx.debug_bounds("edit-preview-metadata-0").unwrap();
                assert!(
                    header.size.height <= px(52.),
                    "header must stay two compact rows"
                );
                assert_eq!(intent.size.height, px(20.));
                assert_eq!(intent_row.bottom(), metadata.origin.y);
                assert!(counts.origin.x >= intent.right());
                assert!(counts.right() <= intent_row.right());
                assert!(counts.origin.y >= intent_row.origin.y);
                assert!(counts.bottom() <= intent_row.bottom());
                assert_eq!(metadata.size.height, px(24.));
                if state == 2 {
                    assert!(vcx.debug_bounds("edit-countdown-bar").is_none());
                }
                for selector in ["edit-review", "edit-toggle", "edit-keep-open"] {
                    assert!(
                        vcx.debug_bounds(selector).is_none(),
                        "no separate {selector} button"
                    );
                }
                for selector in ["diff-file-0"] {
                    if let Some(control) = vcx.debug_bounds(selector) {
                        assert!(control.origin.x >= metadata.origin.x, "{selector}");
                        assert!(control.right() <= metadata.right(), "{selector}");
                        assert!(control.origin.y >= metadata.origin.y, "{selector}");
                        assert!(control.bottom() <= metadata.bottom(), "{selector}");
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn counts_click_never_toggles_inline_diff(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            TimedPreview::new(
                super::super::tests::fixture(),
                0,
                true,
                super::super::tests::header(cx),
            )
        });
        for state in 0..3 {
            view.update(vcx, |view, cx| {
                view.timer = EditPreviewTimer::new();
                view.timer.update(true, Duration::ZERO);
                if state == 1 {
                    view.timer.update(true, Duration::from_secs(10));
                } else if state == 2 {
                    view.timer.expand();
                }
                cx.notify();
            });
            vcx.run_until_parked();
            let before = view.read_with(vcx, |view, _| view.timer.state());
            let counts = vcx.debug_bounds("edit-change-counts-0").unwrap();
            vcx.simulate_click(counts.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(view.read_with(vcx, |view, _| view.timer.state()), before);
        }
    }
}
