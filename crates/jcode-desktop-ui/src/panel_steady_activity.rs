//! Keeps the live activity row steady while the transcript tail glides.
//!
//! The activity row is the last virtual row, so its laid-out position follows
//! the transcript's end. When a streamed block is released, that end jumps
//! down by the block's height and the tail follower then eases the view back
//! up. Text rides this step unnoticed, but a lone spinner visibly bobs. While
//! the transcript follows its tail, this wrapper therefore pins the row at its
//! resting place just above the composer: the row is never painted below the
//! spot the follower is about to bring it back to. Layout itself is
//! untouched, and when the reader scrolls manually the row tracks the content
//! exactly. Other position changes (the composer resizing, short transcripts
//! growing) ease rather than jump.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, point, px,
};

/// Time constant of the screen-position ease.
const EASE: f32 = 0.09;
/// How far the painted row may sit above its laid-out position. The row's
/// own top padding is 10px, so this never overlaps the line above.
const MAX_RISE: f32 = 8.0;
/// How far it may trail below, into the gap above the composer.
const MAX_SINK: f32 = 12.0;
/// Larger jumps (restores, row changes off screen) snap.
const SNAP: f32 = 160.0;
/// Ignore pathological frame gaps.
const MAX_STEP: Duration = Duration::from_millis(100);
/// A row not painted for this long starts at its laid-out position.
const STALE: Duration = Duration::from_millis(250);

/// Smoothed position, relative to the transcript viewport top.
#[derive(Clone, Copy, Debug)]
pub(super) struct Steady {
    y: f32,
    at: Instant,
}

pub(super) type SteadyCell = Rc<Cell<Option<Steady>>>;

/// Advance the eased position one frame. Returns the new position.
fn ease(previous: f32, target: f32, dt: f32) -> f32 {
    if (target - previous).abs() > SNAP {
        return target;
    }
    let mut next = previous + (target - previous) * (1.0 - (-dt / EASE).exp());
    next = next.clamp(target - MAX_RISE, target + MAX_SINK);
    if (target - next).abs() < 0.25 {
        next = target;
    }
    next
}

pub(super) struct SteadyRow {
    child: AnyElement,
    cell: SteadyCell,
    enabled: bool,
    /// Space the list reserves below this row for the floating composer.
    end_pad: f32,
}

pub(super) fn steady_row(
    child: impl IntoElement,
    cell: SteadyCell,
    enabled: bool,
    end_pad: f32,
) -> SteadyRow {
    SteadyRow {
        child: child.into_any_element(),
        cell,
        enabled,
        end_pad,
    }
}

/// Where a following row should paint: never below its resting place at the
/// bottom of the viewport, which is where the tail follower is heading.
fn pinned(target: f32, rest: f32) -> f32 {
    target.min(rest)
}

impl IntoElement for SteadyRow {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SteadyRow {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = self.child.request_layout(window, cx);
        let style = gpui::Style {
            flex_shrink: 0.,
            ..Default::default()
        };
        (window.request_layout(style, [child], cx), ())
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
        // The list prepaints rows inside its viewport mask. Reading the list
        // state here would re-borrow it mid-layout.
        let mask = window.content_mask().bounds;
        let viewport = f32::from(mask.top());
        let laid_out = f32::from(bounds.top()) - viewport;
        let target = if self.enabled {
            let rest = f32::from(mask.size.height) - self.end_pad - f32::from(bounds.size.height);
            pinned(laid_out, rest)
        } else {
            laid_out
        };
        let now = Instant::now();
        // A stale sample (a previous turn, a hidden row) is not motion.
        let fresh = |previous: &Steady| now.saturating_duration_since(previous.at) < STALE;
        let y = match self
            .cell
            .get()
            .filter(|previous| self.enabled && fresh(previous))
        {
            Some(previous) => {
                let dt = now
                    .saturating_duration_since(previous.at)
                    .min(MAX_STEP)
                    .as_secs_f32();
                ease(previous.y, target, dt)
            }
            None => target,
        };
        self.cell.set(Some(Steady { y, at: now }));
        if y != target {
            window.request_animation_frame();
        }
        let scale = window.scale_factor().max(1.0);
        let shift = ((y - laid_out) * scale).round() / scale;
        window.with_element_offset(point(px(0.), px(shift)), |window| {
            self.child.prepaint(window, cx);
        });
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

    #[test]
    fn block_steps_do_not_move_a_following_row() {
        // A released block pushes the laid-out row 66px below its resting
        // place, then the tail follower brings it back. Pinned, the painted
        // row stays put the whole time.
        let rest = 400.0f32;
        let dt = 1.0 / 60.0;
        let mut y = rest;
        for frame in 0..90 {
            let offset = 66.0 * (-(frame % 30) as f32 / 4.0).exp();
            let target = pinned(rest + offset, rest);
            y = ease(y, target, dt);
            assert_eq!(y, rest, "frame {frame}");
        }
        // A short transcript sits above its resting place and follows layout.
        assert_eq!(pinned(120.0, rest), 120.0);
    }

    #[test]
    fn line_jumps_are_softened_and_bounded() {
        // A wrapped line pushes the row down 22px, then the tail glide eases
        // it back. The painted row moves far less than the layout does.
        let dt = 1.0 / 60.0;
        let mut y = 300.0f32;
        let mut layout = 300.0f32;
        let mut painted_travel = 0.0f32;
        let mut layout_travel = 0.0f32;
        for frame in 0..60 {
            let next_layout = if frame % 15 == 0 {
                layout + 22.0
            } else {
                layout + (300.0 - layout) * 0.2
            };
            layout_travel += (next_layout - layout).abs();
            layout = next_layout;
            let next = ease(y, layout, dt);
            assert!(next >= layout - MAX_RISE - 1e-3 && next <= layout + MAX_SINK + 1e-3);
            painted_travel += (next - y).abs();
            y = next;
        }
        assert!(
            painted_travel < layout_travel * 0.6,
            "{painted_travel} vs {layout_travel}"
        );
    }

    #[test]
    fn settles_and_snaps_large_jumps() {
        let mut y = 0.0;
        for _ in 0..120 {
            y = ease(y, 10.0, 1.0 / 60.0);
        }
        assert_eq!(y, 10.0);
        assert_eq!(ease(0.0, 500.0, 1.0 / 60.0), 500.0);
    }
}
