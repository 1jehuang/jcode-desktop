//! Independent session and sidebar sheets. The live tabs own the top edge,
//! without a wide shoulder or a connector forcing unrelated surfaces together.
use std::{cell::RefCell, rc::Rc};

use crate::theme::Theme;
use gpui::{Bounds, Pixels, canvas, div, point, prelude::*, px};

#[derive(Default)]
pub(super) struct Frame {
    pub canvas: Option<Bounds<Pixels>>,
    pub selected_tab: Option<Bounds<Pixels>>,
    pub active_panel: Option<Bounds<Pixels>>,
    pub panel_right: Pixels,
}
pub(super) type SharedFrame = Rc<RefCell<Frame>>;

pub(super) enum Region {
    Canvas,
    SelectedTab,
    Panel { active: bool },
}

pub(super) fn measure(frame: SharedFrame, region: Region) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let bounds = bounds.intersect(&window.content_mask().bounds);
            if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
                return;
            }
            let mut frame = frame.borrow_mut();
            match region {
                Region::Canvas => frame.canvas = Some(bounds),
                Region::SelectedTab => frame.selected_tab = Some(bounds),
                Region::Panel { active } => {
                    frame.panel_right = frame.panel_right.max(bounds.right());
                    if active {
                        frame.active_panel = Some(bounds);
                    }
                }
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub(super) fn background(frame: SharedFrame) -> impl IntoElement {
    let reset = frame.clone();
    div()
        .debug_selector(|| "native-folder-surface".into())
        .absolute()
        .size_full()
        .child(
            canvas(
                move |_, _, _| *reset.borrow_mut() = Frame::default(),
                move |_, _, window, _| {
                    let frame = frame.borrow();
                    if frame.canvas.is_none() {
                        return;
                    }
                    let mut regions = vec![];
                    // The recessed panel backing is HEADER_BG, which the workspace
                    // root already fills the window with, so only the raised
                    // selected tab is a sheet. Repainting the backing added a
                    // near full-window layer to every frame for no visible
                    // change, and software renderers paid for it on each
                    // click-driven repaint (#5).
                    if let Some(tab) = frame.selected_tab {
                        regions.push((Rect::from_bounds(tab), Theme::global().PANEL_BG));
                    }
                    // Paint separately: disconnected navigation should never
                    // require a bridge across the header or another panel.
                    // The panel strip is recessed backing, not active paper.
                    // Otherwise the active pane's rounded cutouts disappear
                    // into it and the inactive panes look like the top layer.
                    // The selected sidebar tab remains its own raised sheet.
                    //
                    // Each sheet is one rectangle, so it is a rounded quad,
                    // not a path. The panel backing covers most of the window,
                    // and as a path it went through a full-window 4x MSAA
                    // pass on every frame. On Windows software rendering that
                    // made each click-driven repaint stall the UI (#5).
                    for (region, color) in regions {
                        let Some((bounds, radius)) = sheet(region) else {
                            continue;
                        };
                        window.paint_quad(gpui::fill(bounds, color).corner_radii(radius));
                    }
                },
            )
            .size_full(),
        )
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}
impl Rect {
    fn from_bounds(bounds: Bounds<Pixels>) -> Self {
        Self {
            left: f32::from(bounds.left()),
            top: f32::from(bounds.top()),
            right: f32::from(bounds.right()),
            bottom: f32::from(bounds.bottom()),
        }
    }
}
/// Sheet corners stay soft without masking their neighbors.
const SHEET_RADIUS: f32 = 8.;

/// A sheet's bounds and corner radius (at most half its shorter side), or
/// `None` for an empty region.
fn sheet(region: Rect) -> Option<(Bounds<Pixels>, Pixels)> {
    let width = region.right - region.left;
    let height = region.bottom - region.top;
    if width <= 0. || height <= 0. {
        return None;
    }
    Some((
        Bounds::new(
            point(px(region.left), px(region.top)),
            gpui::size(px(width), px(height)),
        ),
        px(SHEET_RADIUS.min(width / 2.).min(height / 2.)),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn sheets_are_rounded_quads_with_capped_radius() {
        let (bounds, radius) = sheet(rect(264., 48., 1400., 984.)).unwrap();
        assert_eq!(bounds.origin, point(px(264.), px(48.)));
        assert_eq!(bounds.size, gpui::size(px(1136.), px(936.)));
        assert_eq!(radius, px(8.));
        // A short selected tab keeps a pill-like end instead of overlapping corners.
        assert_eq!(sheet(rect(0., 0., 120., 10.)).unwrap().1, px(5.));
        assert!(sheet(rect(10., 10., 10., 40.)).is_none());
        assert!(sheet(rect(10., 40., 50., 30.)).is_none());
    }
}
