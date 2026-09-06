//! A panel-sized image lightbox. The transcript stays mounted behind it, so
//! closing the preview preserves the user's scroll position and draft.
use super::*;

pub(super) fn fixture_image() -> TranscriptImage {
    TranscriptImage::new(
        "image/png".into(),
        base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("../../../assets/previews/image-preview.png")),
        Some("Image preview test chart".into()),
    )
}

impl Panel {
    pub(super) fn media_preview_handler(
        &self,
        cx: &Context<Self>,
    ) -> markdown::MediaPreviewHandler {
        let panel = cx.entity().downgrade();
        std::rc::Rc::new(move |preview, window, cx| {
            let _ = panel.update(cx, |panel, cx| {
                panel.open_image_preview(
                    TranscriptImage {
                        media_type: "image/svg+xml".into(),
                        data: String::new(),
                        label: Some("Mermaid diagram".into()),
                        preview: Some(preview),
                    },
                    window,
                    cx,
                );
            });
        })
    }

    pub(super) fn open_image_preview(
        &mut self,
        image: TranscriptImage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if image.preview.is_none() {
            return;
        }
        self.image_preview = Some(image);
        self.image_preview_zoom = 1.0;
        self.image_preview_scroll = gpui::ScrollHandle::new();
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_image_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_preview = None;
        self.focus_input(window, cx);
        cx.notify();
    }

    fn zoom_image_preview(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.image_preview_zoom = zoom.clamp(1.0, 4.0);
        if self.image_preview_zoom == 1.0 {
            self.image_preview_scroll
                .set_offset(gpui::point(px(0.0), px(0.0)));
        }
        cx.notify();
    }

    pub(super) fn render_image_preview(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let image = self.image_preview.as_ref()?;
        let preview = image.preview.clone()?;
        Some(
            div()
                .id("image-preview")
                .debug_selector(|| "image-preview".into())
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .bg(Theme::global().PANEL_BG)
                .occlude()
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_image_preview(window, cx);
                    cx.stop_propagation();
                }))
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_3()
                        .text_size(px(12.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(
                                    image
                                        .label
                                        .clone()
                                        .unwrap_or_else(|| "Image preview".into()),
                                ),
                        )
                        .children(
                            [
                                ("image-preview-zoom-out", "−", -0.5),
                                ("image-preview-fit", "Fit", 0.0),
                                ("image-preview-zoom-in", "+", 0.5),
                            ]
                            .into_iter()
                            .map(|(id, label, delta)| {
                                div()
                                    .id(id)
                                    .debug_selector(move || id.into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(Theme::global().QUOTE_BG))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let zoom = if delta == 0.0 {
                                            1.0
                                        } else {
                                            this.image_preview_zoom + delta
                                        };
                                        this.zoom_image_preview(zoom, cx);
                                        cx.stop_propagation();
                                    }))
                            }),
                        )
                        .child(format!("{:.0}%", self.image_preview_zoom * 100.0))
                        .child("Esc to close")
                        .child(
                            div()
                                .id("image-preview-close")
                                .debug_selector(|| "image-preview-close".into())
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(Theme::global().QUOTE_BG))
                                .child("Close ×")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_image_preview(window, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                )
                .child(
                    div()
                        .id("image-preview-viewport")
                        .debug_selector(|| "image-preview-viewport".into())
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .overflow_scroll()
                        .track_scroll(&self.image_preview_scroll)
                        .child(
                            div()
                                .w(gpui::relative(self.image_preview_zoom))
                                .h(gpui::relative(self.image_preview_zoom))
                                .flex_none()
                                .child(
                                    img(crate::image_cache::source(preview))
                                        .debug_selector(|| "image-preview-full".into())
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Contain),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn image_preview_click_enlarges_and_escape_restores_draft(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("image-session", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Image(fixture_image())];
            panel.input.update(cx, |input, cx| {
                input.set_content("keep this draft".into(), cx)
            });
            cx.notify();
        });
        vcx.run_until_parked();
        let scroll_before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        let thumbnail = vcx
            .debug_bounds("transcript-image")
            .expect("thumbnail paints");
        vcx.simulate_click(thumbnail.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_some()));
        let enlarged = vcx
            .debug_bounds("image-preview-full")
            .expect("full image paints");
        assert!(
            enlarged.size.height > thumbnail.size.height,
            "preview must be larger than thumbnail"
        );
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
            "keep this draft"
        );
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
        assert!(vcx.debug_bounds("image-preview").is_none());
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
            scroll_before
        );
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
            "keep this draftx"
        );
        let thumbnail = vcx.debug_bounds("transcript-image").unwrap();
        vcx.simulate_click(thumbnail.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let overlay = vcx.debug_bounds("image-preview").unwrap();
        vcx.simulate_click(overlay.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
    }
    #[gpui::test]
    fn mermaid_preview_streamed_click_close_and_escape_preserve_transcript(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("diagram-session", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        panel.update(vcx, |panel, cx| {
            panel.input.update(cx, |input, cx| {
                input.set_content("diagram draft".into(), cx)
            });
            panel.apply(
                &ApiEvent::TextDelta {
                    session_id: "diagram-session".into(),
                    text: "```mermaid\nflowchart LR\nA[Start] --> B[Done]\n```".into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let scroll_before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        for close_with_escape in [false, true] {
            let thumbnail = vcx
                .debug_bounds("md-mermaid")
                .expect("streamed diagram paints");
            vcx.simulate_click(thumbnail.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert!(panel.read_with(vcx, |panel, _| {
                panel.image_preview.as_ref().is_some_and(|image| {
                    image.media_type == "image/svg+xml"
                        && image.label.as_deref() == Some("Mermaid diagram")
                })
            }));
            let enlarged = vcx
                .debug_bounds("image-preview-full")
                .expect("diagram overlay paints");
            assert!(enlarged.size.height > thumbnail.size.height);
            let zoom_in = vcx.debug_bounds("image-preview-zoom-in").unwrap();
            vcx.simulate_click(zoom_in.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
                1.5
            );
            let zoomed = vcx.debug_bounds("image-preview-full").unwrap();
            assert!(zoomed.size.width > enlarged.size.width * 1.4);
            assert!(zoomed.size.height > enlarged.size.height * 1.4);
            let viewport = vcx.debug_bounds("image-preview-viewport").unwrap();
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -3.0)),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            vcx.run_until_parked();
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Lines(gpui::point(-3.0, 0.0)),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            vcx.run_until_parked();
            let offset = panel.read_with(vcx, |panel, _| panel.image_preview_scroll.offset());
            assert!(
                offset.x < px(0.0) && offset.y < px(0.0),
                "zoomed media pans in both axes: {offset:?}"
            );
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
                scroll_before
            );
            let zoom_out = vcx.debug_bounds("image-preview-zoom-out").unwrap();
            vcx.simulate_click(zoom_out.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
                1.0
            );
            let zoom_in = vcx.debug_bounds("image-preview-zoom-in").unwrap();
            vcx.simulate_click(zoom_in.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            let fit = vcx.debug_bounds("image-preview-fit").unwrap();
            vcx.simulate_click(fit.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
                1.0
            );
            assert_eq!(
                vcx.debug_bounds("image-preview-full").unwrap().size,
                enlarged.size
            );
            vcx.simulate_keystrokes("x");
            assert_eq!(
                panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
                "diagram draft"
            );
            if close_with_escape {
                vcx.simulate_keystrokes("escape");
            } else {
                let close = vcx
                    .debug_bounds("image-preview-close")
                    .expect("close control paints");
                vcx.simulate_click(close.center(), gpui::Modifiers::default());
            }
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("image-preview").is_none());
            assert!(vcx.debug_bounds("md-mermaid").is_some());
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
                scroll_before
            );
        }
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
            "diagram draftx"
        );
    }
}
