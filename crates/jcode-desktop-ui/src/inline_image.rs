//! Stateful, bounded image gestures shared by inline transcript attachments.
//! State is scoped to the mounted row and image identity, not the panel ABI.
use crate::theme::Theme;
use gpui::{
    App, ClickEvent, Context, CursorStyle, Image, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, PinchEvent, Pixels, Point, Render, RenderOnce,
    ScrollHandle, ScrollWheelEvent, SharedString, Window, canvas, div, img, point, prelude::*, px,
    relative,
};
use std::{rc::Rc, sync::Arc};

type OpenImage = Rc<dyn Fn(&mut Window, &mut App)>;
type FitScroll = Rc<dyn Fn(&ScrollWheelEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(crate) struct InlineImage {
    key: SharedString,
    image: Arc<Image>,
    open: OpenImage,
    fit_scroll: Option<FitScroll>,
    on_gesture: Option<OpenImage>,
}

impl InlineImage {
    pub(crate) fn new(
        row: usize,
        image: Arc<Image>,
        open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            key: format!("inline-image-{row}-{}", image.id).into(),
            image,
            open: Rc::new(open),
            fit_scroll: None,
            on_gesture: None,
        }
    }
}

impl InlineImage {
    /// A capturing parent scroller can delegate fit scrolling explicitly. In
    /// this mode the image occludes its parent only inside the viewport.
    pub(crate) fn on_fit_scroll(
        mut self,
        callback: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.fit_scroll = Some(Rc::new(callback));
        self
    }

    pub(crate) fn on_gesture(mut self, callback: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_gesture = Some(Rc::new(callback));
        self
    }
}

impl RenderOnce for InlineImage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.key, cx, |_, _| ImageView {
            image: self.image,
            open: self.open.clone(),
            fit_scroll: self.fit_scroll.clone(),
            on_gesture: self.on_gesture.clone(),
            zoom: 1.0,
            pinch_active: false,
            scroll: ScrollHandle::new(),
            drag: None,
            dragged: false,
        });
        // A retained row can receive a fresh callback without losing its gestures.
        state.update(cx, |state, _| {
            state.open = self.open;
            state.fit_scroll = self.fit_scroll;
            state.on_gesture = self.on_gesture;
        });
        state
    }
}

struct ImageView {
    image: Arc<Image>,
    open: OpenImage,
    fit_scroll: Option<FitScroll>,
    on_gesture: Option<OpenImage>,
    zoom: f32,
    pinch_active: bool,
    scroll: ScrollHandle,
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    dragged: bool,
}

impl ImageView {
    fn pan_to(&self, offset: Point<Pixels>) {
        let size = self.scroll.bounds().size;
        self.scroll.set_offset(point(
            offset.x.clamp(-size.width * (self.zoom - 1.0), px(0.0)),
            offset.y.clamp(-size.height * (self.zoom - 1.0), px(0.0)),
        ));
    }

    fn zoom_at(&mut self, zoom: f32, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !zoom.is_finite() {
            return;
        }
        let zoom = zoom.clamp(1.0, 4.0);
        let ratio = zoom / self.zoom;
        let anchor = position - self.scroll.bounds().origin;
        let offset = self.scroll.offset() * ratio + anchor * (1.0 - ratio);
        self.zoom = zoom;
        self.pan_to(offset);
        cx.notify();
    }
}

impl Render for ImageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id("inline-image-viewport")
                    .debug_selector(|| "inline-image-viewport".into())
                    .relative()
                    .w_full()
                    .h(px(320.0))
                    .flex_none()
                    .overflow_hidden()
                    .track_scroll(&self.scroll)
                    .when(self.fit_scroll.is_some(), |view| view.occlude())
                    .cursor(if self.drag.is_some() {
                        CursorStyle::ClosedHand
                    } else if self.zoom > 1.0 {
                        CursorStyle::OpenHand
                    } else {
                        CursorStyle::PointingHand
                    })
                    .on_pinch(cx.listener(|this, event: &PinchEvent, window, cx| {
                        match event.phase {
                            gpui::TouchPhase::Started => this.pinch_active = true,
                            gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled => {
                                this.pinch_active = false;
                            }
                            gpui::TouchPhase::Moved => {}
                        }
                        if let Some(begin) = &this.on_gesture {
                            begin(window, cx);
                        }
                        this.zoom_at(this.zoom * (1.0 + event.delta), event.position, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }))
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        if this.pinch_active || event.modifiers.control || this.zoom > 1.0 {
                            if let Some(begin) = &this.on_gesture {
                                begin(window, cx);
                            }
                        }
                        let delta = event.delta.pixel_delta(px(20.0));
                        if this.pinch_active {
                            // Linux forwards native pinch translation as precise
                            // scroll after the scale event. It remains a pan even
                            // with Ctrl held or when zoom has reached Fit.
                            this.pan_to(this.scroll.offset() + delta);
                            cx.notify();
                        } else if event.modifiers.control {
                            let factor = (f32::from(delta.y) * 0.01).clamp(-1.0, 1.0).exp();
                            this.zoom_at(this.zoom * factor, event.position, cx);
                        } else if this.zoom > 1.0 {
                            this.pan_to(this.scroll.offset() + delta);
                            cx.notify();
                        } else {
                            // At fit, preserve the parent's exact scroll policy.
                            if let Some(scroll) = &this.fit_scroll {
                                scroll(event, window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                            return;
                        }
                        // Consume even at an edge, preventing scroll chaining to chat.
                        window.prevent_default();
                        cx.stop_propagation();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if this.zoom > 1.0 {
                                if let Some(begin) = &this.on_gesture {
                                    begin(window, cx);
                                }
                            }
                            this.dragged = false;
                            this.drag = Some((event.position, event.position));
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        let displaced = matches!(event, ClickEvent::Mouse(event)
                            if (event.up.position - event.down.position).magnitude() > 4.0);
                        if !this.dragged && !displaced {
                            (this.open)(window, cx);
                        }
                        this.drag = None;
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .w(relative(self.zoom))
                            .h(px(320.0 * self.zoom))
                            .relative()
                            .flex_none()
                            .child(
                                img(crate::image_cache::source(self.image.clone()))
                                    .debug_selector(|| "inline-image-content".into())
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .child({
                        let entity = cx.entity();
                        // Capture while dragging, even after leaving the viewport.
                        canvas(
                            |_, _, _| (),
                            move |_, _, window, _| {
                                let moved = entity.clone();
                                window.on_mouse_event(
                                    move |event: &MouseMoveEvent, phase, _, cx| {
                                        if !phase.capture() || moved.read(cx).drag.is_none() {
                                            return;
                                        }
                                        moved.update(cx, |this, cx| {
                                            if event.pressed_button != Some(MouseButton::Left) {
                                                this.drag = None;
                                            } else if let Some((start, previous)) = this.drag {
                                                this.dragged |=
                                                    (event.position - start).magnitude() > 4.0;
                                                if this.zoom > 1.0 && this.dragged {
                                                    this.pan_to(
                                                        this.scroll.offset() + event.position
                                                            - previous,
                                                    );
                                                }
                                                this.drag = Some((start, event.position));
                                            }
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    },
                                );
                                let released = entity.clone();
                                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                                    if phase.capture()
                                        && event.button == MouseButton::Left
                                        && released.read(cx).drag.is_some()
                                    {
                                        released.update(cx, |this, cx| {
                                            this.drag = None;
                                            cx.notify();
                                        });
                                        // Let GPUI synthesize click. `dragged` survives release.
                                    }
                                });
                            },
                        )
                        .absolute()
                        .size_full()
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .text_size(px(11.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .child(
                        div()
                            .id("inline-image-fit")
                            .debug_selector(|| "inline-image-fit".into())
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(Theme::global().QUOTE_BG))
                            .child("Fit")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.drag = None;
                                this.dragged = false;
                                this.zoom_at(1.0, this.scroll.bounds().center(), cx);
                                cx.stop_propagation();
                            })),
                    )
                    .child(format!("{:.0}%", self.zoom * 100.0))
                    .child("Pinch or Ctrl+scroll to zoom · Drag to pan")
                    .child(
                        div()
                            .id("inline-image-open")
                            .debug_selector(|| "inline-image-open".into())
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(Theme::global().QUOTE_BG))
                            .child("Open")
                            .on_click(cx.listener(|this, _, window, cx| {
                                (this.open)(window, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Bounds, Entity, ImageFormat, Modifiers, ScrollDelta, TestAppContext, TouchPhase,
        VisualTestContext,
    };
    use std::cell::Cell;

    struct Transcript {
        image: Arc<Image>,
        scroll: ScrollHandle,
        opens: Rc<Cell<usize>>,
        revision: usize,
    }

    impl Render for Transcript {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opens = self.opens.clone();
            div()
                .id("test-transcript")
                .w(px(600.0))
                .h(px(500.0))
                .overflow_scroll()
                .track_scroll(&self.scroll)
                .child(InlineImage::new(0, self.image.clone(), move |_, _| {
                    opens.set(opens.get() + 1);
                }))
                .child(div().h(px(1000.0)).child(format!("tail {}", self.revision)))
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<Transcript>, &mut VisualTestContext) {
        cx.add_window_view(|_, _| Transcript {
            image: crate::image_cache::encoded(
                ImageFormat::Png,
                include_bytes!("../../../assets/previews/image-preview.png").to_vec(),
            ),
            scroll: ScrollHandle::new(),
            opens: Rc::new(Cell::new(0)),
            revision: 0,
        })
    }

    fn wheel(cx: &mut VisualTestContext, position: Point<Pixels>, x: f32, y: f32, control: bool) {
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(x), px(y))),
            modifiers: Modifiers {
                control,
                ..Default::default()
            },
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
    }

    fn pinch(cx: &mut VisualTestContext, position: Point<Pixels>, delta: f32) {
        cx.simulate_event(PinchEvent {
            position,
            delta,
            phase: TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();
    }

    fn bounds(cx: &mut VisualTestContext) -> Bounds<Pixels> {
        cx.debug_bounds("inline-image-content")
            .expect("image paints")
    }

    fn assert_near(a: Point<Pixels>, b: Point<Pixels>) {
        assert!((a.x - b.x).abs() < px(1.0), "x: {a:?} != {b:?}");
        assert!((a.y - b.y).abs() < px(1.0), "y: {a:?} != {b:?}");
    }

    #[gpui::test]
    fn inline_image_pans_during_continuous_pinch(cx: &mut TestAppContext) {
        let (transcript, cx) = setup(cx);
        cx.run_until_parked();
        let anchor = cx.debug_bounds("inline-image-viewport").unwrap().center();
        cx.simulate_event(PinchEvent {
            position: anchor,
            phase: TouchPhase::Started,
            ..Default::default()
        });
        // Native Linux delivers scale then translation for each update, with
        // no intervening frame or finger lift. Zero-scale updates still pan.
        for (delta, x, y, control) in [
            (1.0, 15.0, -12.0, false),
            (0.0, -8.0, 9.0, true),
            (0.5, 12.0, -5.0, false),
            (0.0, -7.0, 8.0, false),
        ] {
            let before = bounds(cx);
            cx.simulate_event(PinchEvent {
                position: anchor,
                delta,
                phase: TouchPhase::Moved,
                ..Default::default()
            });
            wheel(cx, anchor, x, y, control);
            let after = bounds(cx);
            assert!((after.size.width - before.size.width * (1.0 + delta)).abs() < px(1.0));
            assert_near(
                after.origin,
                anchor + (before.origin - anchor) * (1.0 + delta) + point(px(x), px(y)),
            );
            assert_eq!(
                transcript.read_with(cx, |t, _| t.scroll.offset()),
                Point::default()
            );
            assert_eq!(transcript.read_with(cx, |t, _| t.opens.get()), 0);
        }
        cx.simulate_event(PinchEvent {
            position: anchor,
            phase: TouchPhase::Ended,
            ..Default::default()
        });
        let before = bounds(cx);
        wheel(cx, anchor, 0.0, 10.0, true);
        assert!(
            bounds(cx).size.width > before.size.width,
            "Ctrl-wheel zoom resumes after pinch ends"
        );
    }

    #[gpui::test]
    fn inline_image_pinch_translation_at_fit_never_scrolls_chat(cx: &mut TestAppContext) {
        let (transcript, cx) = setup(cx);
        cx.run_until_parked();
        let before = bounds(cx);
        let anchor = before.center();
        cx.simulate_event(PinchEvent {
            position: anchor,
            phase: TouchPhase::Started,
            ..Default::default()
        });
        wheel(cx, anchor, -20.0, -40.0, false);
        assert_eq!(bounds(cx), before);
        assert_eq!(
            transcript.read_with(cx, |t, _| t.scroll.offset()),
            Point::default()
        );
        cx.simulate_event(PinchEvent {
            position: anchor,
            phase: TouchPhase::Ended,
            ..Default::default()
        });
        wheel(cx, anchor, 0.0, -40.0, false);
        assert!(transcript.read_with(cx, |t, _| t.scroll.offset().y) < px(0.0));
    }

    #[gpui::test]
    fn inline_image_pointer_zoom_limits_and_fit(cx: &mut TestAppContext) {
        let (transcript, cx) = setup(cx);
        cx.run_until_parked();
        let viewport = cx.debug_bounds("inline-image-viewport").unwrap();
        assert_eq!(viewport.size.height, px(320.0));
        let before = bounds(cx);
        assert_eq!(before.size, viewport.size);
        let anchor = viewport.origin + point(viewport.size.width * 0.3, px(120.0));
        pinch(cx, anchor, 1.0);
        let after = bounds(cx);
        assert_eq!(after.size.width, before.size.width * 2.0);
        assert_eq!(after.size.height, before.size.height * 2.0);
        assert_near(after.origin, anchor + (before.origin - anchor) * 2.0);
        wheel(cx, anchor, 0.0, 10.0, true);
        let factor = 0.1_f32.exp();
        assert_near(bounds(cx).origin, anchor + (after.origin - anchor) * factor);
        assert_eq!(
            cx.debug_bounds("inline-image-viewport")
                .unwrap()
                .size
                .height,
            px(320.0)
        );
        for (delta, scale) in [(100.0, 4.0), (f32::NAN, 4.0), (-0.99, 1.0)] {
            pinch(cx, anchor, delta);
            assert_eq!(bounds(cx).size.width, before.size.width * scale);
            assert_eq!(bounds(cx).size.height, before.size.height * scale);
        }
        assert_near(bounds(cx).origin, before.origin);
        pinch(cx, anchor, 1.0);
        let fit = cx.debug_bounds("inline-image-fit").unwrap();
        cx.simulate_click(fit.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(bounds(cx), before);
        assert_eq!(transcript.read_with(cx, |t, _| t.opens.get()), 0);
        assert_eq!(
            transcript.read_with(cx, |t, _| t.scroll.offset()),
            Point::default()
        );
    }

    #[gpui::test]
    fn inline_image_scroll_routing_and_edges(cx: &mut TestAppContext) {
        let (transcript, cx) = setup(cx);
        cx.run_until_parked();
        let anchor = cx.debug_bounds("inline-image-viewport").unwrap().center();
        pinch(cx, anchor, 1.0);
        let before = bounds(cx);
        wheel(cx, anchor, -15.0, -20.0, false);
        assert_near(
            bounds(cx).origin,
            before.origin + point(px(-15.0), px(-20.0)),
        );
        assert_eq!(bounds(cx).size, before.size);
        for delta in [-10000.0, -10000.0, 10000.0, 10000.0] {
            wheel(cx, anchor, delta, delta, false);
            assert_eq!(
                transcript.read_with(cx, |t, _| t.scroll.offset()),
                Point::default()
            );
        }
        let fit = cx.debug_bounds("inline-image-fit").unwrap();
        cx.simulate_click(fit.center(), Modifiers::default());
        cx.run_until_parked();
        wheel(cx, anchor, 0.0, -80.0, false);
        assert!(
            transcript.read_with(cx, |t, _| t.scroll.offset().y) < px(0.0),
            "ordinary fit scroll must reach transcript"
        );
    }

    #[gpui::test]
    fn inline_image_drag_round_trip_outside_release_and_open(cx: &mut TestAppContext) {
        let (transcript, cx) = setup(cx);
        cx.run_until_parked();
        let viewport = cx.debug_bounds("inline-image-viewport").unwrap();
        let anchor = viewport.center();
        pinch(cx, anchor, 1.0);
        let before = bounds(cx);
        cx.simulate_event(MouseDownEvent {
            position: anchor,
            button: MouseButton::Left,
            ..Default::default()
        });
        cx.run_until_parked();
        let moved = anchor + point(px(30.0), px(20.0));
        cx.simulate_event(MouseMoveEvent {
            position: moved,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        });
        cx.run_until_parked();
        assert_near(bounds(cx).origin, before.origin + point(px(30.0), px(20.0)));
        cx.simulate_event(MouseMoveEvent {
            position: anchor,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.simulate_event(MouseUpEvent {
            position: anchor,
            button: MouseButton::Left,
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(
            transcript.read_with(cx, |t, _| t.opens.get()),
            0,
            "round-trip drag is not a click"
        );
        assert_near(bounds(cx).origin, before.origin);
        cx.simulate_event(MouseDownEvent {
            position: anchor,
            button: MouseButton::Left,
            ..Default::default()
        });
        cx.run_until_parked();
        let outside = viewport.bottom_right() + point(px(15.0), px(15.0));
        cx.simulate_event(MouseMoveEvent {
            position: outside,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.simulate_event(MouseUpEvent {
            position: outside,
            button: MouseButton::Left,
            ..Default::default()
        });
        cx.run_until_parked();
        let released = bounds(cx);
        cx.simulate_event(MouseMoveEvent {
            position: anchor,
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(bounds(cx), released, "release outside ends capture");
        assert_eq!(transcript.read_with(cx, |t, _| t.opens.get()), 0);
        let fit = cx.debug_bounds("inline-image-fit").unwrap();
        cx.simulate_click(fit.center(), Modifiers::default());
        cx.run_until_parked();
        cx.simulate_click(anchor, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(transcript.read_with(cx, |t, _| t.opens.get()), 1);
        let open = cx.debug_bounds("inline-image-open").unwrap();
        cx.simulate_click(open.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(transcript.read_with(cx, |t, _| t.opens.get()), 2);
    }

    #[gpui::test]
    fn inline_image_state_survives_render_but_not_image_replacement(cx: &mut TestAppContext) {
        let (transcript, cx) = setup(cx);
        cx.run_until_parked();
        let before = bounds(cx);
        pinch(cx, before.center(), 1.0);
        let zoomed = bounds(cx);
        transcript.update(cx, |t, cx| {
            t.revision += 1;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(bounds(cx), zoomed);
        transcript.update(cx, |t, cx| {
            t.image = crate::image_cache::encoded(ImageFormat::Svg,
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#.to_vec());
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(bounds(cx), before, "replacement must start at fit");
    }
}
