//! A window-sized image lightbox. The transcript stays mounted behind it, so
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
                        source: jcode_sdk::RenderedImageSource::Other {
                            role: "assistant".into(),
                        },
                        anchor: None,
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
        self.image_preview_drag = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_image_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_preview = None;
        self.image_preview_drag = None;
        self.focus_input(window, cx);
        cx.notify();
    }

    fn zoom_image_preview(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let center = self.image_preview_scroll.bounds().center();
        self.zoom_image_preview_at(zoom, center, cx);
    }

    fn zoom_image_preview_at(
        &mut self,
        zoom: f32,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !zoom.is_finite() {
            return;
        }
        let previous_zoom = self.image_preview_zoom;
        self.image_preview_zoom = zoom.clamp(1.0, 4.0);
        let offset = if self.image_preview_zoom == 1.0 {
            gpui::point(px(0.0), px(0.0))
        } else {
            // Keep the image point under the gesture fixed. Toolbar zoom uses
            // the viewport center, while pinch and wheel zoom use the pointer.
            let ratio = self.image_preview_zoom / previous_zoom;
            let bounds = self.image_preview_scroll.bounds();
            let anchor = position - bounds.origin;
            let previous = self.image_preview_scroll.offset();
            gpui::point(
                previous.x * ratio + anchor.x * (1.0 - ratio),
                previous.y * ratio + anchor.y * (1.0 - ratio),
            )
        };
        self.image_preview_scroll.set_offset(offset);
        cx.notify();
    }

    pub(super) fn render_image_preview(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let image = self.image_preview.as_ref()?;
        let preview = image.preview.clone()?;
        let overlay = div()
                .id("image-preview")
                .debug_selector(|| "image-preview".into())
                .w(window.viewport_size().width)
                .h(window.viewport_size().height)
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .bg(Theme::global().PANEL_BG)
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_image_preview(window, cx);
                    cx.stop_propagation();
                }))
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .on_pinch(|_, _, cx| cx.stop_propagation())
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
                        .map(|mut view| {
                            view.style().allow_concurrent_scroll = Some(true);
                            view
                        })
                        .track_scroll(&self.image_preview_scroll)
                        .cursor(gpui::CursorStyle::OpenHand)
                        .when(self.image_preview_drag.is_some(), |view| {
                            view.cursor(gpui::CursorStyle::ClosedHand)
                        })
                        .on_pinch(cx.listener(|this, event: &gpui::PinchEvent, _, cx| {
                            this.zoom_image_preview_at(
                                this.image_preview_zoom * (1.0 + event.delta),
                                event.position,
                                cx,
                            );
                            cx.stop_propagation();
                        }))
                        .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, window, cx| {
                            if event.modifiers.control {
                                let delta = event.delta.pixel_delta(px(20.0));
                                let factor = (f32::from(delta.y) * 0.01).clamp(-1.0, 1.0).exp();
                                this.zoom_image_preview_at(this.image_preview_zoom * factor, event.position, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        }))
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.image_preview_drag = Some(event.position);
                            cx.notify();
                            cx.stop_propagation();
                        }))
                        .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                            if event.pressed_button == Some(gpui::MouseButton::Left) {
                                if let Some(previous) = this.image_preview_drag {
                                    this.image_preview_drag = Some(event.position);
                                    let offset = this.image_preview_scroll.offset() + event.position - previous;
                                    this.image_preview_scroll.set_offset(offset);
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            } else if this.image_preview_drag.take().is_some() {
                                cx.notify();
                            }
                        }))
                        .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.image_preview_drag = None;
                            cx.notify();
                        }))
                        .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.image_preview_drag = None;
                            cx.notify();
                        }))
                        .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                            this.image_preview_drag = None;
                            // GPUI also emits clicks after mouse drags. Keep pan
                            // gestures open, but let a regular click return to chat.
                            let dragged = matches!(event, gpui::ClickEvent::Mouse(event)
                                if (event.up.position - event.down.position).magnitude() > 4.0);
                            if !dragged {
                                this.close_image_preview(window, cx);
                            }
                            cx.notify();
                            cx.stop_propagation();
                        }))
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
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child("Pinch or Ctrl+scroll to zoom · Drag or scroll to pan · Click image to close"),
                )
                .into_any_element();
        // Defer beyond panel clipping and anchor in window coordinates so the
        // viewer covers the sidebar and neighboring panels as well.
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::point(px(0.0), px(0.0)))
                    .child(overlay),
            )
            .with_priority(100)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn inline_image_panel_hover_gestures_preserve_composer_and_route_scroll(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("inline-gestures", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        vcx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.items = (0..40)
                    .map(|i| Item::User(format!("earlier message {i}")))
                    .collect();
                panel.items.push(Item::Image(fixture_image()));
                panel
                    .input
                    .update(cx, |input, cx| input.set_content("draft".into(), cx));
                panel.focus_input(window, cx);
                cx.notify();
            })
        });
        vcx.run_until_parked();
        let viewport = vcx
            .debug_bounds("inline-image-viewport")
            .expect("inline viewport paints");
        let before = vcx.debug_bounds("inline-image-content").unwrap();
        assert_eq!(viewport.size.height, px(400.0));
        assert_eq!(before.size, viewport.size);
        let anchor = viewport.center();
        let transcript_before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        // No click/focus transfer precedes this hover pinch. Existing chat
        // momentum must be cancelled as the image takes ownership.
        panel.update(vcx, |panel, _| {
            panel.transcript_wheel_glide.remaining = 70.0
        });
        vcx.simulate_event(gpui::MouseMoveEvent {
            position: anchor,
            ..Default::default()
        });
        vcx.simulate_event(gpui::PinchEvent {
            position: anchor,
            delta: 1.0,
            phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        vcx.run_until_parked();
        let zoomed = vcx.debug_bounds("inline-image-content").unwrap();
        assert_eq!(zoomed.size.height, before.size.height * 2.0);
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.transcript_wheel_glide.remaining),
            0.0
        );
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
            "draftx"
        );
        for (control, x, y) in [(true, 0.0, 10.0), (false, -15.0, -20.0)] {
            let old = vcx.debug_bounds("inline-image-content").unwrap();
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: anchor,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(x), px(y))),
                modifiers: gpui::Modifiers {
                    control,
                    ..Default::default()
                },
                touch_phase: gpui::TouchPhase::Moved,
            });
            vcx.run_until_parked();
            let new = vcx.debug_bounds("inline-image-content").unwrap();
            let expected = if control {
                anchor + (old.origin - anchor) * 0.1_f32.exp()
            } else {
                old.origin + gpui::point(px(x), px(y))
            };
            assert!((new.origin.x - expected.x).abs() < px(1.0));
            assert!((new.origin.y - expected.y).abs() < px(1.0));
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
                transcript_before
            );
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.transcript_wheel_glide.remaining),
                0.0
            );
            assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
        }
        panel.update(vcx, |panel, _| {
            panel.transcript_wheel_glide.remaining = 25.0
        });
        vcx.simulate_event(gpui::MouseDownEvent {
            position: anchor,
            button: gpui::MouseButton::Left,
            ..Default::default()
        });
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.transcript_wheel_glide.remaining),
            0.0
        );
        vcx.run_until_parked();
        let moved = anchor + gpui::point(px(20.0), px(15.0));
        vcx.simulate_event(gpui::MouseMoveEvent {
            position: moved,
            pressed_button: Some(gpui::MouseButton::Left),
            ..Default::default()
        });
        vcx.run_until_parked();
        vcx.simulate_event(gpui::MouseUpEvent {
            position: moved,
            button: gpui::MouseButton::Left,
            ..Default::default()
        });
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
            transcript_before
        );
        let fit = vcx.debug_bounds("inline-image-fit").unwrap();
        vcx.simulate_click(fit.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(vcx.debug_bounds("inline-image-content").unwrap(), before);
        // Ctrl-wheel must also bypass parent capture at fit, not just zoomed.
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: anchor,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(10.0))),
            modifiers: gpui::Modifiers {
                control: true,
                ..Default::default()
            },
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("inline-image-content")
                .unwrap()
                .size
                .height
                > before.size.height
        );
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
            transcript_before
        );
        let fit = vcx.debug_bounds("inline-image-fit").unwrap();
        vcx.simulate_click(fit.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: anchor,
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, 3.0)),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        assert!(
            panel.read_with(vcx, |panel, _| panel.transcript_wheel_glide.remaining
                != 0.0
                || panel.test_scroll_offset_y() != transcript_before),
            "fit wheel goes through transcript scrolling"
        );
    }

    #[gpui::test]
    fn image_click_expands_inline_and_preserves_draft(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("image-session", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        vcx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.items = vec![Item::Image(fixture_image())];
                panel
                    .input
                    .update(cx, |input, cx| input.set_content("draft".into(), cx));
                panel.focus_input(window, cx);
                cx.notify();
            })
        });
        vcx.run_until_parked();
        let before = vcx.debug_bounds("inline-image-viewport").unwrap();
        vcx.simulate_click(before.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let expanded = vcx.debug_bounds("inline-image-viewport").unwrap();
        assert!(expanded.size.width > before.size.width);
        assert!(expanded.size.height > before.size.height);
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
        assert!(vcx.debug_bounds("image-preview").is_none());
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
            "draftx"
        );
        let fit = vcx.debug_bounds("inline-image-fit").unwrap();
        assert!(fit.top() >= expanded.bottom());
        vcx.simulate_click(fit.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            vcx.debug_bounds("inline-image-viewport").unwrap().size,
            before.size
        );
    }

    #[gpui::test]
    fn image_preview_pinch_wheel_drag_and_click_to_close(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("gesture-session", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Image(fixture_image())];
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_image_preview(fixture_image(), window, cx);
            })
        });
        vcx.run_until_parked();
        let viewport = vcx.debug_bounds("image-preview-viewport").unwrap();
        let before = vcx.debug_bounds("image-preview-full").unwrap();
        let transcript_scroll = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        let anchor =
            viewport.origin + gpui::point(viewport.size.width * 0.3, viewport.size.height * 0.4);
        for (phase, delta) in [
            (gpui::TouchPhase::Started, 0.0),
            (gpui::TouchPhase::Moved, 1.0),
            (gpui::TouchPhase::Ended, 0.0),
        ] {
            vcx.simulate_event(gpui::PinchEvent {
                position: anchor,
                delta,
                phase,
                ..Default::default()
            });
            vcx.run_until_parked();
        }
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
            2.0
        );
        let after = vcx.debug_bounds("image-preview-full").unwrap();
        let expected_origin = anchor + (before.origin - anchor) * 2.0;
        assert!((after.origin.x - expected_origin.x).abs() < px(1.0));
        assert!((after.origin.y - expected_origin.y).abs() < px(1.0));

        let offset = panel.read_with(vcx, |panel, _| panel.image_preview_scroll.offset());
        vcx.simulate_event(gpui::MouseDownEvent {
            position: anchor,
            button: gpui::MouseButton::Left,
            ..Default::default()
        });
        vcx.run_until_parked();
        let movement = gpui::point(px(30.0), px(20.0));
        vcx.simulate_event(gpui::MouseMoveEvent {
            position: anchor + movement,
            pressed_button: Some(gpui::MouseButton::Left),
            ..Default::default()
        });
        vcx.run_until_parked();
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.image_preview_scroll.offset()),
            offset + movement
        );
        vcx.simulate_event(gpui::MouseUpEvent {
            position: anchor + movement,
            button: gpui::MouseButton::Left,
            ..Default::default()
        });
        vcx.run_until_parked();
        assert!(
            panel.read_with(vcx, |panel, _| panel.image_preview_drag.is_none()
                && panel.image_preview.is_some())
        );

        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: anchor,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(10.0))),
            modifiers: gpui::Modifiers {
                control: true,
                ..Default::default()
            },
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        let zoom = panel.read_with(vcx, |panel, _| panel.image_preview_zoom);
        assert!((zoom - 2.0 * 0.1_f32.exp()).abs() < 0.001);
        let offset = panel.read_with(vcx, |panel, _| panel.image_preview_scroll.offset());
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: anchor,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-15.0), px(-20.0))),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
            zoom
        );
        let panned = panel.read_with(vcx, |panel, _| panel.image_preview_scroll.offset());
        assert!(panned.x < offset.x && panned.y < offset.y);

        for (selector, expected_zoom) in
            [("image-preview-fit", 1.0), ("image-preview-zoom-in", 1.5)]
        {
            let button = vcx.debug_bounds(selector).unwrap();
            vcx.simulate_click(button.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
                expected_zoom
            );
            assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_some()));
        }
        for (delta, expected) in [(100.0, 4.0), (f32::NAN, 4.0), (-0.99, 1.0)] {
            vcx.simulate_event(gpui::PinchEvent {
                position: anchor,
                delta,
                phase: gpui::TouchPhase::Moved,
                ..Default::default()
            });
            vcx.run_until_parked();
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
                expected
            );
        }
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.image_preview_scroll.offset()),
            gpui::point(px(0.0), px(0.0))
        );
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
            transcript_scroll
        );
        let zoom_in = vcx.debug_bounds("image-preview-zoom-in").unwrap();
        vcx.simulate_click(zoom_in.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_some()));
        vcx.simulate_click(anchor, gpui::Modifiers::default());
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
                    message_id: None,
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
            let overlay = vcx.debug_bounds("image-preview").unwrap();
            let viewport = vcx.update(|window, _| window.viewport_size());
            assert_eq!(overlay.origin, gpui::point(px(0.0), px(0.0)));
            assert_eq!(
                overlay.size, viewport,
                "preview must cover the whole window"
            );
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
            // GPUI rounds layout positions to device pixels.
            assert!((zoomed.center().x - enlarged.center().x).abs() < px(1.0));
            assert!((zoomed.center().y - enlarged.center().y).abs() < px(1.0));
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
            for _ in 0..8 {
                let zoom_in = vcx.debug_bounds("image-preview-zoom-in").unwrap();
                vcx.simulate_click(zoom_in.center(), gpui::Modifiers::default());
                vcx.run_until_parked();
            }
            assert_eq!(
                panel.read_with(vcx, |panel, _| panel.image_preview_zoom),
                4.0
            );
            let center = vcx.debug_bounds("image-preview-full").unwrap().center();
            assert!((center.x - enlarged.center().x).abs() < px(2.0));
            assert!((center.y - enlarged.center().y).abs() < px(2.0));
            let fit = vcx.debug_bounds("image-preview-fit").unwrap();
            vcx.simulate_click(fit.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            // Workspace navigation asks the panel for its focus target. An
            // open viewer must not send that focus to the hidden composer.
            vcx.update(|window, cx| {
                panel.update(cx, |panel, cx| panel.focus_input(window, cx));
            });
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
