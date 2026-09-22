//! Same-frame positioning for the input's out-of-flow suggestions.
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Point, Position, Size, Style, Window, point, prelude::*,
    px, relative, size,
};

const EDGE: f32 = 8.;
// One-pixel borders and p_1 (four pixels) on both vertical sides.
const CHROME: f32 = 10.;

pub(super) fn scroll_height(viewport: Pixels, model: bool) -> Pixels {
    let height = f32::from(viewport);
    px((height * if model { 0.6 } else { 0.35 })
        .min(if model { 480. } else { 280. })
        .min((height - 2. * EDGE - CHROME).max(0.)))
}

fn origin(
    anchor: Point<Pixels>,
    popup: Size<Pixels>,
    viewport: Size<Pixels>,
    gap: Pixels,
) -> Point<Pixels> {
    let fit = |preferred: Pixels, extent: Pixels, limit: Pixels| {
        let margin = px(EDGE).min(((limit - extent) / 2.).max(px(0.)));
        preferred
            .max(margin)
            .min((limit - extent - margin).max(margin))
    };
    point(
        fit(anchor.x, popup.width, viewport.width),
        fit(anchor.y - gap - popup.height, popup.height, viewport.height),
    )
}

fn available_side(composer: Bounds<Pixels>, viewport: Size<Pixels>, gap: Pixels) -> (bool, Pixels) {
    let above = (composer.top() - gap - px(EDGE)).max(px(0.));
    let below = (viewport.height - composer.bottom() - gap - px(EDGE)).max(px(0.));
    // Prefer above whenever there is enough room for the full model popup.
    // Otherwise choose the larger side, keeping the search editor accessible.
    if above >= scroll_height(viewport.height, true) + px(CHROME) || above >= below {
        (true, above)
    } else {
        (false, below)
    }
}

pub(super) struct Popup {
    child: AnyElement,
    gap: Pixels,
}

impl Popup {
    pub(super) fn new(child: impl IntoElement, gap: Pixels) -> Self {
        Self {
            child: child.into_any_element(),
            gap,
        }
    }
}

impl IntoElement for Popup {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Popup {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // This out-of-flow marker follows the full composer's current layout,
        // not cached bounds from the previous frame (stale during resizing).
        let mut style = Style::default();
        style.position = Position::Absolute;
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let viewport = window.viewport_size();
        let width = bounds
            .size
            .width
            .min((viewport.width - px(2. * EDGE)).max(px(0.)));
        let (above, available) = available_side(bounds, viewport, self.gap);
        // max_h_full on the popup resolves against this definite space. Its
        // flex/min_h_0 scroll subtree shrinks without hiding borders or rows.
        let measured = self.child.layout_as_root(
            size(
                AvailableSpace::Definite(width),
                AvailableSpace::Definite(available),
            ),
            window,
            cx,
        );
        let anchor = if above {
            bounds.origin
        } else {
            point(
                bounds.left(),
                bounds.bottom() + self.gap + measured.height + self.gap,
            )
        };
        self.child
            .prepaint_at(origin(anchor, measured, viewport, self.gap), window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Entity, TestAppContext, div};
    struct Fixture {
        input: Entity<super::super::PromptInput>,
        top: f32,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .pt(px(self.top))
                .px(px(12.))
                .child(self.input.clone())
        }
    }

    #[gpui::test]
    fn rendered_model_suggestions_stay_inside_resized_viewport(cx: &mut TestAppContext) {
        check_rendered_model_suggestions(cx);
    }

    fn check_rendered_model_suggestions(cx: &mut TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, cx| {
            let input = cx.new(|cx| super::super::PromptInput::new(cx, "test", |_, _, _, _| {}));
            input.update(cx, |input, cx| {
                input.command_models = (0..60).map(|i| format!("provider:model-{i}")).collect();
                input.command_completion = true;
                input.set_content("/model ".into(), cx);
            });
            Fixture { input, top: 20. }
        });
        let handle = vcx.update(|window, _| window.window_handle());
        for (width, height) in [(800., 600.), (320., 240.), (240., 100.), (1000., 800.)] {
            vcx.simulate_window_resize(handle, size(px(width), px(height)));
            for top in [0., height / 2., height - 40.] {
                view.update(vcx, |view, cx| {
                    view.top = top;
                    cx.notify();
                });
                vcx.run_until_parked();
                let popup = vcx
                    .debug_bounds("slash-command-overlay")
                    .expect("visible suggestions");
                assert!(popup.top() >= px(0.) && popup.left() >= px(0.), "{popup:?}");
                assert!(
                    popup.bottom() <= px(height) && popup.right() <= px(width),
                    "{popup:?} in {width}x{height}"
                );
                let scroll = vcx.debug_bounds("slash-command-scroll").unwrap();
                assert!(scroll.size.height > px(0.));
                let composer = vcx.debug_bounds("prompt-input").unwrap();
                assert!(
                    popup.bottom() <= composer.top() || popup.top() >= composer.bottom(),
                    "popup {popup:?} overlaps composer {composer:?}"
                );
            }
        }
    }

    #[test]
    fn suggestions_fit_at_top_center_bottom_and_after_resize() {
        for (width, height) in [(240., 100.), (320., 240.), (800., 600.), (1600., 1000.)] {
            for model in [false, true] {
                let viewport = size(px(width), px(height));
                let popup = size(
                    px(width - 16.),
                    scroll_height(viewport.height, model) + px(CHROME),
                );
                for y in [0., 20., height / 2., height, height + 500.] {
                    for x in [-100., 0., width / 2., width + 100.] {
                        for gap in [4., 10.] {
                            let p = origin(point(px(x), px(y)), popup, viewport, px(gap));
                            assert!(p.x >= px(0.) && p.y >= px(0.));
                            assert!(p.x + popup.width <= viewport.width);
                            assert!(p.y + popup.height <= viewport.height);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn suggestions_remain_above_composer_when_space_allows() {
        let popup = size(px(400.), px(200.));
        assert_eq!(
            origin(
                point(px(50.), px(500.)),
                popup,
                size(px(800.), px(600.)),
                px(4.)
            ),
            point(px(50.), px(296.))
        );
    }
}
