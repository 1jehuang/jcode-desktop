//! Transient screenshot panel. Never insert into the workspace strip or change
//! its camera: a window-anchored preview flies into a measured attachment slot.
use super::*;
use std::{cell::Cell, rc::Rc};

pub(super) type MeasuredBounds = Rc<Cell<Option<Bounds<Pixels>>>>;
const HOLD: Duration = Duration::from_millis(900);
const FLIGHT: Duration = Duration::from_millis(420);

pub(super) struct Preview {
    pub index: usize,
    started: Instant,
    origin: Option<Bounds<Pixels>>,
}

impl Preview {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            started: Instant::now(),
            origin: None,
        }
    }
}

pub(super) fn bounds_marker(bounds: MeasuredBounds) -> gpui::AnyElement {
    gpui::canvas(|_, _, _| (), move |rect, _, _, _| bounds.set(Some(rect)))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
}

/// Prefer a usable right-hand panel, then left. A narrow window gets a floating
/// preview above the composer rather than moving or resizing the active panel.
fn preview_bounds(
    panel: Bounds<Pixels>,
    target: Bounds<Pixels>,
    viewport: gpui::Size<Pixels>,
) -> Bounds<Pixels> {
    let margin = 12.0;
    let width = f32::from(viewport.width).max(1.0);
    let height = f32::from(viewport.height).max(1.0);
    let right = (width - f32::from(panel.right()) - 2.0 * margin).max(0.0);
    let left = (f32::from(panel.left()) - 2.0 * margin).max(0.0);
    let (x, y, w, h) = if right >= 240.0 || left >= 240.0 {
        let on_right = right >= 240.0;
        let w = if on_right { right } else { left }.min(420.0);
        let h = (f32::from(panel.size.height) - 2.0 * margin)
            .clamp(1.0, 460.0)
            .min((height - 2.0 * margin).max(1.0));
        let x = if on_right {
            f32::from(panel.right()) + margin
        } else {
            f32::from(panel.left()) - margin - w
        };
        (x, f32::from(panel.top()) + margin, w, h)
    } else {
        let w = 360.0_f32.min((width - 2.0 * margin).max(1.0));
        let h = 280.0_f32
            .min((f32::from(target.top()) - 2.0 * margin).max(80.0))
            .min((height - 2.0 * margin).max(1.0));
        (
            f32::from(panel.right()) - margin - w,
            f32::from(target.top()) - margin - h,
            w,
            h,
        )
    };
    Bounds::new(
        point(
            px(x.clamp(0.0, (width - w).max(0.0))),
            px(y.clamp(0.0, (height - h).max(0.0))),
        ),
        size(px(w), px(h)),
    )
}

fn progress(elapsed: Duration) -> Option<f32> {
    if elapsed >= HOLD + FLIGHT {
        return None;
    }
    let t = (elapsed.saturating_sub(HOLD).as_secs_f32() / FLIGHT.as_secs_f32()).clamp(0.0, 1.0);
    Some(t * t * (3.0 - 2.0 * t))
}

fn interpolate(from: Bounds<Pixels>, to: Bounds<Pixels>, progress: f32) -> Bounds<Pixels> {
    Bounds::new(
        from.origin + (to.origin - from.origin) * progress,
        size(
            from.size.width + (to.size.width - from.size.width) * progress,
            from.size.height + (to.size.height - from.size.height) * progress,
        ),
    )
}

impl PromptInput {
    pub(crate) fn paste_preview_panel_marker(&self) -> gpui::AnyElement {
        bounds_marker(self.preview_panel_bounds.clone())
    }

    pub(super) fn render_paste_preview(&mut self, window: &mut Window) -> Option<gpui::AnyElement> {
        let preview = self.attachment_preview.as_mut()?;
        // Navigation, submission, removal and restore must not leave floating
        // screenshots behind. Do not steal focus from continued typing.
        let elapsed = preview.started.elapsed();
        let Some(t) = progress(elapsed).filter(|_| self.focus_handle.is_focused(window)) else {
            self.attachment_preview = None;
            return None;
        };
        window.request_animation_frame();
        let attachment = self.attachments.get(preview.index)?;
        let target = attachment.bounds.get()?;
        let panel = self.preview_panel_bounds.get().or(self.last_bounds)?;
        let origin = *preview
            .origin
            .get_or_insert_with(|| preview_bounds(panel, target, window.viewport_size()));
        let rect = interpolate(origin, target, t);
        let chrome = 1.0 - t;
        let overlay = div()
            .id("paste-image-preview")
            .debug_selector(|| "paste-image-preview".into())
            .w(rect.size.width)
            .h(rect.size.height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_lg()
            .border(px(chrome))
            .border_color(Theme::global().PANEL_BORDER_FOCUS)
            .bg(Theme::global().PANEL_BG)
            .p(px(12.0 * chrome))
            .child(
                div()
                    .h(px(28.0 * chrome))
                    .flex_none()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .opacity(chrome)
                    .child("Screenshot attached"),
            )
            .child(
                img(crate::image_cache::source(attachment.preview.clone()))
                    .debug_selector(|| "paste-image-preview-content".into())
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .object_fit(gpui::ObjectFit::Contain),
            );
        Some(
            gpui::deferred(gpui::anchored().position(rect.origin).child(overlay))
                .with_priority(90)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> crate::clipboard_image::ClipboardImage {
        crate::clipboard_image::ClipboardImage {
            media_type: "image/png".into(),
            bytes: include_bytes!("../../../assets/previews/image-preview.png").to_vec(),
            width: 640,
            height: 480,
        }
    }

    #[gpui::test]
    fn preview_lifecycle_preserves_draft_and_attachment_payload(cx: &mut gpui::TestAppContext) {
        let (input, vcx) =
            cx.add_window_view(|_, cx| PromptInput::new(cx, "test", |_, _, _, _| {}));
        vcx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.focus_handle.focus(window, cx);
                input.set_content("keep my draft".into(), cx);
                input.attach_image(image(), cx);
                input.attachments[0]
                    .bounds
                    .set(Some(rect(30., 600., 64., 52.)));
                input
                    .preview_panel_bounds
                    .set(Some(rect(10., 30., 500., 700.)));
                assert!(input.attachment_preview.is_some());
                let snapshot = input.snapshot();
                input.attachment_preview.as_mut().unwrap().started = Instant::now() - HOLD - FLIGHT;
                assert!(input.render_paste_preview(window).is_none());
                assert!(input.attachment_preview.is_none());
                assert_eq!(input.snapshot(), snapshot);

                // Repeated pastes supersede only the transient preview, not images.
                input.attach_image(image(), cx);
                assert_eq!(input.attachment_preview.as_ref().unwrap().index, 1);
                assert_eq!(input.attachments.len(), 2);
                input.remove_attachment(0, cx);
                assert!(input.attachment_preview.is_none());
                assert_eq!(input.attachments.len(), 1);
                input.restore(snapshot.clone(), cx);
                assert!(input.attachment_preview.is_none());
                assert_eq!(input.snapshot(), snapshot);

                input.attach_image(image(), cx);
                window.blur();
                assert!(input.render_paste_preview(window).is_none());
                assert!(input.attachment_preview.is_none());
            })
        });
    }

    #[gpui::test]
    fn flight_does_not_resize_composer_and_submit_cancels_it(cx: &mut gpui::TestAppContext) {
        let (input, vcx) =
            cx.add_window_view(|_, cx| PromptInput::new(cx, "test", |_, _, _, _| {}));
        vcx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.focus_handle.focus(window, cx);
                input.attach_image(image(), cx);
            })
        });
        vcx.run_until_parked();
        let initial = vcx.debug_bounds("prompt-input").unwrap();
        input.update(vcx, |input, cx| {
            input.attachment_preview.as_mut().unwrap().started = Instant::now() - HOLD - FLIGHT / 2;
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(vcx.debug_bounds("prompt-input").unwrap(), initial);
        assert!(vcx.debug_bounds("paste-image-preview").is_some());
        vcx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.submit(&Submit, window, cx);
                assert!(input.attachment_preview.is_none());
                assert!(input.attachments.is_empty());
            })
        });
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn prefers_right_even_when_left_has_more_space() {
        let panel = rect(600., 40., 500., 900.);
        let preview = preview_bounds(
            panel,
            rect(620., 840., 64., 52.),
            size(px(1440.), px(1000.)),
        );
        assert!(preview.left() > panel.right());
        assert!(preview.right() <= px(1440.));
    }

    #[test]
    fn uses_left_when_right_would_require_moving_active_panel() {
        let panel = rect(850., 40., 570., 940.);
        let preview = preview_bounds(
            panel,
            rect(880., 870., 64., 52.),
            size(px(1440.), px(1000.)),
        );
        assert!(preview.right() < panel.left());
        assert_eq!(preview.size.width, px(420.));
    }

    #[test]
    fn compact_fallback_stays_in_view_above_attachment() {
        for (w, h) in [(390., 700.), (240., 300.), (100., 80.)] {
            let panel = rect(0., 30., w, h - 30.);
            let target = rect(20., h - 60., 64., 52.);
            let preview = preview_bounds(panel, target, size(px(w), px(h)));
            assert!(preview.left() >= px(0.) && preview.right() <= px(w));
            assert!(preview.top() >= px(0.) && preview.bottom() <= px(h));
            if h > 100. {
                assert!(preview.bottom() < target.top());
            }
        }
    }

    #[test]
    fn holds_then_flies_exactly_to_attachment_and_stops() {
        assert_eq!(progress(Duration::ZERO), Some(0.));
        assert_eq!(progress(HOLD), Some(0.));
        assert!((progress(HOLD + FLIGHT / 2).unwrap() - 0.5).abs() < 0.001);
        assert_eq!(progress(HOLD + FLIGHT), None);
        let from = rect(900., 50., 420., 460.);
        let to = rect(290., 860., 64., 52.);
        assert_eq!(interpolate(from, to, 0.), from);
        assert_eq!(interpolate(from, to, 1.), to);
    }
}
