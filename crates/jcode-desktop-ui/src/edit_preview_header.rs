//! Two-tone edit header: intent and counts above file details.
use super::*;
use crate::ui_actions::OpenChangeReview;

#[derive(Clone, PartialEq)]
pub(crate) struct PreviewHeader {
    pub intent: Option<String>,
    pub error: Option<String>,
    pub review: OpenChangeReview,
    pub focus: gpui::FocusHandle,
    /// Fixtures can omit the transcript model, but live cards share one
    /// selection and stable document key across every header leaf.
    pub selection: Option<(
        gpui::Entity<crate::text_selection::TextSelection>,
        gpui::SharedString,
    )>,
}

impl PreviewHeader {
    pub(super) fn leaf(
        &self,
        part: &str,
        text: String,
        window: &Window,
        cx: &gpui::App,
    ) -> gpui::AnyElement {
        match &self.selection {
            Some((selection, key)) => crate::text_selection::plain(
                selection.clone(),
                format!("{key}-{part}"),
                text,
                window,
                cx,
            ),
            None => text.into_any_element(),
        }
    }
}

impl TimedPreview {
    pub(super) fn render_header(
        &mut self,
        window: &Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Div {
        let running = !self.done && !self.header.review.failed;
        if !running {
            self.running_veil = None;
        }
        let veil = running.then(|| {
            gpui::AnyView::from(
                self.running_veil
                    .get_or_insert_with(|| {
                        cx.new(|_| {
                            crate::pulse_text::Ticker::new(crate::tool_icon::running_veil(
                                Theme::global().CODE_HEADER_BG,
                            ))
                        })
                    })
                    .clone(),
            )
        });
        let theme = Theme::global();
        let index = self.index;
        let countdown = self.mode == crate::config::DiffDisplay::Inline
            && self.timer.state() == EditPreviewState::Countdown;
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
            .child(self.header.leaf("path", file.path.clone(), window, cx));
        let counts = super::super::counts(file)
            .id("edit-change-counts")
            .debug_selector(move || format!("edit-change-counts-{index}").into())
            .text_size(px(11.))
            .rounded_full()
            .cursor_pointer()
            .hover(|style| style.bg(theme.ACCENT_DIM))
            // The counts own their click. Never toggle the inline preview or
            // let the workspace mouse-up handler refocus the source panel.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.preview_press = None;
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |event, window, cx| {
                if super::is_preview_click(event) {
                    focus.focus(window, cx);
                    window.dispatch_action(Box::new(request.clone()), cx);
                }
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
            .child(crate::tool_icon::animate_running(
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
                    .child(crate::tool_icon::render_badge_with_label(
                        &self.header.review.name,
                        self.done,
                        self.header.review.failed,
                        self.header
                            .leaf("name", self.header.review.name.to_string(), window, cx),
                    ))
                    .child(
                        div()
                            .debug_selector(move || format!("edit-preview-intent-{index}").into())
                            .flex_1()
                            .min_w_0()
                            .h(px(20.))
                            .truncate()
                            .child(
                                self.header.leaf(
                                    "intent",
                                    self.header
                                        .intent
                                        .clone()
                                        .unwrap_or_else(|| "Edit file".into()),
                                    window,
                                    cx,
                                ),
                            ),
                    )
                    .child(counts),
                self.done,
                self.header.review.failed,
                veil,
            ))
            .child(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn multi_file_headers_and_click_actions_share_tool_payload(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            crate::panel::Panel::new(
                "shared-preview-payload".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let input = serde_json::json!({
            "intent": "Update both files",
            "patch_text": "*** Begin Patch\n*** Add File: first.rs\n+first\n*** Add File: second.rs\n+second\n*** End Patch",
        }).to_string();
        let output = "Recorded tool output\n".repeat(32768);
        // Exercise the actual per-file construction on every status transition,
        // including failure, which must retain the original review payload.
        for (done, error) in [(false, None), (true, None), (true, Some("Edit failed"))] {
            panel.update(cx, |panel, cx| {
                assert!(
                    panel
                        .render_edit_metadata(
                            "shared",
                            "apply_patch",
                            &input,
                            &output,
                            done,
                            error,
                            cx,
                        )
                        .is_some()
                );
                let views = panel.edit_previews.views.borrow();
                assert_eq!(views.len(), 2);
                let first = &views["shared-0"].read(cx).header.review;
                let second = &views["shared-1"].read(cx).header.review;
                assert!(Arc::ptr_eq(&first.name, &second.name));
                assert!(Arc::ptr_eq(&first.input, &second.input));
                assert!(Arc::ptr_eq(&first.output, &second.output));
                assert_eq!(first.input.as_ref(), input);
                assert_eq!(first.output.as_ref(), output);
                assert_eq!((first.selected, second.selected), (0, 1));
                assert_eq!((first.done, first.failed), (done, error.is_some()));
                assert_eq!((second.done, second.failed), (done, error.is_some()));
                assert_eq!(first.source, cx.entity_id());
                // render_header and action dispatch both clone the request.
                // Those clones must retain the same backing allocations too.
                let click_action = second.clone();
                let dispatched = click_action.clone();
                assert!(Arc::ptr_eq(&first.input, &dispatched.input));
                assert!(Arc::ptr_eq(&first.output, &dispatched.output));
                assert_eq!(dispatched.selected, 1);
            });
        }
    }

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
                    output: "".into(),
                    selected: 0, done: true, failed: false,
                },
                focus: cx.focus_handle(),
                selection: None,
                error: None,
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
                let pill = vcx
                    .debug_bounds("tool-identity-pill")
                    .expect("edit tool pill paints");
                let name = vcx
                    .debug_bounds("tool-name")
                    .expect("edit tool name persists");
                assert!(pill.left() < icon.left() && name.right() < pill.right());
                assert!(icon.right() < name.left());
                assert!(pill.right() <= intent.left());
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

    #[gpui::test]
    fn selectable_header_clicks_still_toggle_inline_diff(cx: &mut gpui::TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            let mut header = super::super::tests::header(cx);
            header.selection = Some((
                cx.new(crate::text_selection::TextSelection::new),
                "click-test".into(),
            ));
            TimedPreview::new(super::super::tests::fixture(), 0, true, header)
        });
        for part in ["name", "intent", "path"] {
            let selector = match part {
                "name" => "selectable-text-click-test-name",
                "intent" => "selectable-text-click-test-intent",
                "path" => "selectable-text-click-test-path",
                _ => unreachable!(),
            };
            view.update(vcx, |view, cx| {
                view.timer.expand();
                cx.notify();
            });
            vcx.run_until_parked();
            let leaf = vcx.debug_bounds(selector).unwrap();
            vcx.simulate_click(leaf.center(), gpui::Modifiers::default());
            assert_eq!(
                view.read_with(vcx, |view, _| view.timer.state()),
                EditPreviewState::Collapsing,
                "{part}"
            );
            view.update(vcx, |view, cx| {
                view.timer.update(true, Duration::from_secs(1));
                cx.notify();
            });
            vcx.run_until_parked();
            let leaf = vcx.debug_bounds(selector).unwrap();
            vcx.simulate_click(leaf.center(), gpui::Modifiers::default());
            assert_eq!(
                view.read_with(vcx, |view, _| view.timer.state()),
                EditPreviewState::PinnedOpen,
                "{part}"
            );
        }
    }
}
