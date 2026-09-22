//! Small overlay scrollbars for GPUI's scrollable divs.

use gpui::{
    AnyElement, App, IntoElement, ListState, MouseButton, MouseMoveEvent, MouseUpEvent, RenderOnce,
    ScrollHandle, Window, canvas, div, point, prelude::*, px,
};
use std::{cell::Cell, rc::Rc};

use crate::theme::Theme;

const WIDTH: f32 = 4.0;
const INSET: f32 = 4.0;
/// Space compact content can reserve for the track plus a gap on either side.
/// Keep this stable even without overflow so rows do not shift as lists grow.
pub const GUTTER: f32 = WIDTH + INSET * 2.0;
const MIN_THUMB_HEIGHT: f32 = 28.0;
const HIT_WIDTH: f32 = 20.0;

#[derive(Clone, Copy, Debug)]
struct ListGeometry {
    viewport: f32,
    maximum: f32,
    offset: f32,
    track: f32,
    thumb: f32,
    top: f32,
}

impl ListGeometry {
    fn new(viewport: f32, maximum: f32, offset: f32) -> Option<Self> {
        if viewport <= INSET * 2.0 || maximum <= 0.5 {
            return None;
        }
        let track = viewport - INSET * 2.0;
        let thumb =
            (track * viewport / (viewport + maximum)).clamp(MIN_THUMB_HEIGHT.min(track), track);
        let offset = offset.clamp(0.0, maximum);
        let top = (track - thumb) * offset / maximum;
        Some(Self {
            viewport,
            maximum,
            offset,
            track,
            thumb,
            top,
        })
    }

    fn from_list(state: &ListState) -> Option<Self> {
        Self::new(
            state.viewport_bounds().size.height.into(),
            state.max_offset_for_scrollbar().y.into(),
            -f32::from(state.scroll_px_offset_for_scrollbar().y),
        )
    }

    fn drag_offset(self, delta: f32) -> f32 {
        let travel = self.track - self.thumb;
        if travel <= 0.0 {
            return self.offset;
        }
        (self.offset + delta * self.maximum / travel).clamp(0.0, self.maximum)
    }

    fn page_offset(self, above_thumb: bool) -> f32 {
        (self.offset
            + if above_thumb {
                -self.viewport
            } else {
                self.viewport
            })
        .clamp(0.0, self.maximum)
    }
}

struct ThumbDrag {
    state: ListState,
    start_y: f32,
    geometry: ListGeometry,
}

impl Drop for ThumbDrag {
    fn drop(&mut self) {
        // Also release GPUI's frozen measurement extent if the element disappears.
        self.state.scrollbar_drag_ended();
    }
}

type ScrollStart = Box<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
struct InteractiveListScrollbar {
    state: ListState,
    selector: &'static str,
    on_scroll_start: ScrollStart,
    on_scroll_finished: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// Interactive alternative to [`vertical_list`]. Render after the list, inside
/// the same positioned viewport. `selector` must be stable and unique there.
///
/// The callback runs synchronously before every left-button thumb grab or track
/// page click. Use it to cancel queued wheel motion and release tail-follow.
/// It can capture a weak panel entity and update it with the supplied `App`.
/// Callers that intercept wheel events in capture phase should ignore them
/// while `state.is_scrollbar_dragging()` so touchpad momentum cannot fight a grab.
/// The selector still identifies only the 4px painted thumb, not the invisible
/// 20px hit target. Dragging is direct pixel positioning, without momentum.
/// The finished callback runs after a track page or after a drag releases its
/// frozen extent, including recovery from a missed mouse-up.
pub fn interactive_vertical_list(
    state: &ListState,
    selector: &'static str,
    on_scroll_start: impl Fn(&mut Window, &mut App) + 'static,
    on_scroll_finished: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    InteractiveListScrollbar {
        state: state.clone(),
        selector,
        on_scroll_start: Box::new(on_scroll_start),
        on_scroll_finished: Rc::new(on_scroll_finished),
    }
    .into_any_element()
}

impl RenderOnce for InteractiveListScrollbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let drag = window.use_keyed_state((self.selector, 0usize), cx, |_, _| None::<ThumbDrag>);
        let Some(geometry) = ListGeometry::from_list(&self.state) else {
            drag.update(cx, |drag, _| {
                drag.take();
            });
            return div().into_any_element();
        };
        let active = drag.read(cx).is_some();
        let drag_down = drag.clone();
        let track_origin = Rc::new(Cell::new(0.0));
        let painted_origin = track_origin.clone();
        let selector = self.selector;
        let page_finished = self.on_scroll_finished.clone();
        let drag_finished = self.on_scroll_finished.clone();
        div()
            .id((selector, 1usize))
            .group(selector)
            .absolute()
            .top(px(INSET))
            .right_0()
            .w(px(HIT_WIDTH))
            .h(px(geometry.track))
            .cursor_default()
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                (self.on_scroll_start)(window, cx);
                let Some(current_geometry) = ListGeometry::from_list(&self.state) else {
                    return;
                };
                let y = f32::from(event.position.y);
                // Hit-test the thumb the user actually saw. The list may have
                // measured new rows since this overlay was constructed.
                let track_y = y - track_origin.get();
                if track_y >= geometry.top && track_y <= geometry.top + geometry.thumb {
                    drag_down.update(cx, |drag, _| {
                        drag.take();
                        self.state.scrollbar_drag_started();
                        *drag = Some(ThumbDrag {
                            state: self.state.clone(),
                            start_y: y,
                            geometry: current_geometry,
                        });
                    });
                } else {
                    self.state.set_offset_from_scrollbar(point(
                        px(0.),
                        px(-current_geometry.page_offset(track_y < geometry.top)),
                    ));
                    page_finished(window, cx);
                }
                cx.stop_propagation();
                window.refresh();
            })
            .child(
                div()
                    .debug_selector(move || selector.into())
                    .absolute()
                    .top(px(geometry.top))
                    .right(px(INSET))
                    .w(px(WIDTH))
                    .h(px(geometry.thumb))
                    .rounded_full()
                    .bg(if active {
                        Theme::global().TEXT_DIM
                    } else {
                        Theme::global().TEXT_FAINT
                    })
                    .group_hover(selector, |style| style.bg(Theme::global().TEXT_DIM)),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        painted_origin.set(f32::from(bounds.top()));
                        let drag_move = drag.clone();
                        let move_finished = drag_finished.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if !phase.capture() || drag_move.read(cx).is_none() {
                                return;
                            }
                            let finished = drag_move.update(cx, |drag, _| {
                                if !event.dragging() {
                                    drag.take();
                                    return true;
                                } else if let Some(drag) = drag {
                                    let offset = drag
                                        .geometry
                                        .drag_offset(f32::from(event.position.y) - drag.start_y);
                                    drag.state
                                        .set_offset_from_scrollbar(point(px(0.), px(-offset)));
                                }
                                false
                            });
                            if finished {
                                move_finished(window, cx);
                            }
                            cx.stop_propagation();
                            window.refresh();
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase.capture()
                                && event.button == MouseButton::Left
                                && drag.read(cx).is_some()
                            {
                                drag.update(cx, |drag, _| {
                                    drag.take();
                                });
                                drag_finished(window, cx);
                                cx.stop_propagation();
                                window.refresh();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }
}

fn vertical_parts(handle: &ScrollHandle, selector: &'static str, show_track: bool) -> AnyElement {
    let viewport_height = f32::from(handle.bounds().size.height);
    let max_offset = f32::from(handle.max_offset().y).max(0.0);
    if viewport_height <= 0.0 || max_offset <= 0.5 {
        return div().into_any_element();
    }

    let track_height = (viewport_height - INSET * 2.0).max(0.0);
    let content_height = viewport_height + max_offset;
    let thumb_height = (track_height * viewport_height / content_height)
        .clamp(MIN_THUMB_HEIGHT.min(track_height), track_height);
    let progress = (-f32::from(handle.offset().y) / max_offset).clamp(0.0, 1.0);
    let thumb_top = (track_height - thumb_height) * progress;

    div()
        .debug_selector(move || selector.into())
        .absolute()
        .top(px(INSET))
        .right(px(INSET))
        .w(px(WIDTH))
        .h(px(track_height))
        .rounded_full()
        .when(show_track, |track| track.bg(Theme::global().MINIMAP_TRACK))
        .child(
            div()
                .absolute()
                .top(px(thumb_top))
                .w_full()
                .h(px(thumb_height))
                .rounded_full()
                .bg(Theme::global().TEXT_FAINT),
        )
        .into_any_element()
}

/// Paint a thin, rounded vertical thumb for `handle` when its content overflows.
///
/// The scrollable element remains responsible for wheel and touchpad input. This
/// is deliberately an overlay, so adding it never changes transcript wrapping.
pub fn vertical(handle: &ScrollHandle, selector: &'static str) -> AnyElement {
    vertical_parts(handle, selector, false)
}

/// Paint a persistent track as well as the thumb, for compact regions where an
/// isolated thumb can be mistaken for decoration.
pub fn vertical_with_track(handle: &ScrollHandle, selector: &'static str) -> AnyElement {
    vertical_parts(handle, selector, true)
}

/// Paint the same overlay thumb for GPUI's variable-height virtual list.
pub fn vertical_list(state: &ListState, selector: &'static str) -> AnyElement {
    let viewport_height = f32::from(state.viewport_bounds().size.height);
    let max_offset = f32::from(state.max_offset_for_scrollbar().y).max(0.0);
    if viewport_height <= 0.0 || max_offset <= 0.5 {
        return div().into_any_element();
    }

    let track_height = (viewport_height - INSET * 2.0).max(0.0);
    let content_height = viewport_height + max_offset;
    let thumb_height = (track_height * viewport_height / content_height)
        .clamp(MIN_THUMB_HEIGHT.min(track_height), track_height);
    let progress =
        (-f32::from(state.scroll_px_offset_for_scrollbar().y) / max_offset).clamp(0.0, 1.0);
    let thumb_top = INSET + (track_height - thumb_height) * progress;

    div()
        .debug_selector(move || selector.into())
        .absolute()
        .top(px(thumb_top))
        .right(px(INSET))
        .w(px(WIDTH))
        .h(px(thumb_height))
        .rounded_full()
        .bg(Theme::global().TEXT_FAINT)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn scrollbar_dimensions_stay_thin_and_touch_friendly() {
        assert!(WIDTH <= 4.0);
        assert!(MIN_THUMB_HEIGHT >= 24.0);
        assert!(HIT_WIDTH >= 20.0);
    }

    #[test]
    fn direct_drag_preserves_grab_offset_and_clamps() {
        let geometry = ListGeometry::new(200.0, 800.0, 300.0).unwrap();
        assert_eq!(geometry.drag_offset(0.0), 300.0);
        assert!((geometry.drag_offset(1.0) - (300.0 + 800.0 / 153.6)).abs() < 0.001);
        assert_eq!(geometry.drag_offset(-1000.0), 0.0);
        assert_eq!(geometry.drag_offset(1000.0), 800.0);
        assert_eq!(geometry.page_offset(true), 100.0);
        assert_eq!(geometry.page_offset(false), 500.0);
        assert!(ListGeometry::new(0.0, 800.0, 0.0).is_none());
        assert!(ListGeometry::new(200.0, 0.5, 0.0).is_none());
        let tiny = ListGeometry::new(10.0, 800.0, 400.0).unwrap();
        assert_eq!(tiny.drag_offset(100.0), 400.0);
    }

    struct TestScrollbar {
        list: ListState,
        starts: Rc<Cell<usize>>,
        visible: bool,
    }

    impl gpui::Render for TestScrollbar {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let starts = self.starts.clone();
            div()
                .relative()
                .w(px(200.))
                .h(px(200.))
                .child(
                    gpui::list(self.list.clone(), |_, _, _| {
                        div().h(px(50.)).w_full().into_any_element()
                    })
                    .size_full(),
                )
                .when(self.visible, |view| {
                    view.child(interactive_vertical_list(
                        &self.list,
                        "test-list-thumb",
                        move |_, _| starts.set(starts.get() + 1),
                        |_, _| {},
                    ))
                })
        }
    }

    #[gpui::test]
    fn thumb_drag_survives_redraw_and_leaving_hit_target(cx: &mut gpui::TestAppContext) {
        let list = ListState::new(20, gpui::ListAlignment::Top, px(0.)).measure_all();
        let starts = Rc::new(Cell::new(0));
        let (view, vcx) = cx.add_window_view(|_, _| TestScrollbar {
            list: list.clone(),
            starts: starts.clone(),
            visible: true,
        });
        vcx.run_until_parked();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let thumb = vcx.debug_bounds("test-list-thumb").unwrap();
        assert_eq!(thumb.size.width, px(WIDTH));
        assert_eq!(thumb.right(), px(200. - INSET));
        // Grab outside the painted thumb, but within the invisible hit region.
        let grab = point(thumb.left() - px(8.), thumb.center().y);
        vcx.simulate_mouse_down(grab, MouseButton::Left, Default::default());
        assert_eq!(starts.get(), 1);
        assert!(list.is_scrollbar_dragging());
        assert_eq!(list.scroll_px_offset_for_scrollbar().y, px(0.));
        vcx.run_until_parked();
        let geometry = ListGeometry::from_list(&list).unwrap();
        let outside = point(px(10.), grab.y + px(30.));
        vcx.simulate_mouse_move(outside, MouseButton::Left, Default::default());
        vcx.run_until_parked();
        let expected = geometry.drag_offset(30.);
        assert!((-f32::from(list.scroll_px_offset_for_scrollbar().y) - expected).abs() < 0.1);
        vcx.simulate_mouse_up(outside, MouseButton::Left, Default::default());
        assert!(!list.is_scrollbar_dragging());
        let released = list.scroll_px_offset_for_scrollbar();
        vcx.simulate_mouse_move(point(px(10.), px(190.)), None, Default::default());
        vcx.run_until_parked();
        assert_eq!(list.scroll_px_offset_for_scrollbar(), released);
        assert_eq!(starts.get(), 1);
    }

    #[gpui::test]
    fn track_pages_once_and_missing_button_releases_drag(cx: &mut gpui::TestAppContext) {
        let list = ListState::new(20, gpui::ListAlignment::Top, px(0.)).measure_all();
        let starts = Rc::new(Cell::new(0));
        let (view, vcx) = cx.add_window_view(|_, _| TestScrollbar {
            list: list.clone(),
            starts: starts.clone(),
            visible: true,
        });
        vcx.run_until_parked();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let below = point(px(185.), px(185.));
        vcx.simulate_click(below, Default::default());
        vcx.run_until_parked();
        assert_eq!(starts.get(), 1);
        assert!(!list.is_scrollbar_dragging());
        assert_eq!(list.scroll_px_offset_for_scrollbar().y, px(-200.));
        vcx.simulate_click(point(px(185.), px(6.)), Default::default());
        vcx.run_until_parked();
        assert_eq!(starts.get(), 2);
        assert_eq!(list.scroll_px_offset_for_scrollbar().y, px(0.));
        let thumb = vcx.debug_bounds("test-list-thumb").unwrap();
        vcx.simulate_mouse_down(thumb.center(), MouseButton::Right, Default::default());
        assert_eq!(starts.get(), 2);
        assert!(!list.is_scrollbar_dragging());
        vcx.simulate_mouse_down(thumb.center(), MouseButton::Left, Default::default());
        assert!(list.is_scrollbar_dragging());
        vcx.run_until_parked();
        vcx.simulate_mouse_move(point(px(10.), px(100.)), None, Default::default());
        assert!(!list.is_scrollbar_dragging());
        assert_eq!(list.scroll_px_offset_for_scrollbar().y, px(0.));
        vcx.simulate_mouse_down(thumb.center(), MouseButton::Left, Default::default());
        assert!(list.is_scrollbar_dragging());
        view.update(vcx, |view, cx| {
            view.visible = false;
            cx.notify();
        });
        vcx.run_until_parked();
        // GPUI retains the previous frame's keyed state and event listeners
        // until that frame buffer is reused. Its final references drop here.
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        assert!(!list.is_scrollbar_dragging());
    }
}
