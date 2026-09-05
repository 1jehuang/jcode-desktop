//! Independent session and sidebar sheets. The live tabs own the top edge,
//! without a wide shoulder or a connector forcing unrelated surfaces together.
use std::{cell::RefCell, rc::Rc};

use super::{FOLDER_CONTENT_INSET, STRIP_PADDING_Y};
use crate::theme::Theme;
use gpui::{Bounds, PathBuilder, Pixels, canvas, div, point, prelude::*, px};

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
                    let Some(canvas) = frame.canvas else { return };
                    let mut regions = vec![];
                    if frame.panel_right > canvas.left() {
                        regions.push(Rect {
                            left: f32::from(canvas.left()),
                            top: f32::from(canvas.top()) + STRIP_PADDING_Y + FOLDER_CONTENT_INSET,
                            right: f32::from(frame.panel_right.min(canvas.right())),
                            bottom: f32::from(canvas.bottom()) - STRIP_PADDING_Y,
                        });
                    }
                    if let Some(tab) = frame.selected_tab {
                        regions.push(Rect::from_bounds(tab));
                    }
                    // Paint separately: disconnected navigation should never
                    // require a bridge across the header or another panel.
                    for region in regions {
                        let contour = outline(&[region]);
                        if contour.len() < 3 {
                            return;
                        }
                        let mut builder = PathBuilder::fill();
                        // Keep each sheet's corners soft without masking its neighbors.
                        for i in 0..contour.len() {
                            let previous = contour[(i + contour.len() - 1) % contour.len()];
                            let vertex = contour[i];
                            let next = contour[(i + 1) % contour.len()];
                            let radius = 8f32
                                .min(distance(previous, vertex) / 2.)
                                .min(distance(vertex, next) / 2.);
                            let enter = toward(vertex, previous, radius);
                            let exit = toward(vertex, next, radius);
                            if i == 0 {
                                builder.move_to(point(px(enter.0), px(enter.1)));
                            } else {
                                builder.line_to(point(px(enter.0), px(enter.1)));
                            }
                            builder.curve_to(
                                point(px(exit.0), px(exit.1)),
                                point(px(vertex.0), px(vertex.1)),
                            );
                        }
                        builder.close();
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, Theme::global().PANEL_BG);
                        }
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
type Vertex = (f32, f32);
fn distance(a: Vertex, b: Vertex) -> f32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}
fn toward(a: Vertex, b: Vertex, amount: f32) -> Vertex {
    let ratio = amount / distance(a, b);
    (a.0 + (b.0 - a.0) * ratio, a.1 + (b.1 - a.1) * ratio)
}

/// Extract the external contour of a small connected orthogonal union. Grid
/// coordinates come from actual layout bounds, so resizing and scrolling do
/// not require a second, approximate layout model. At most four rectangles.
fn outline(regions: &[Rect]) -> Vec<Vertex> {
    let regions: Vec<_> = regions
        .iter()
        .filter(|r| r.right > r.left && r.bottom > r.top)
        .collect();
    let mut xs: Vec<_> = regions.iter().flat_map(|r| [r.left, r.right]).collect();
    let mut ys: Vec<_> = regions.iter().flat_map(|r| [r.top, r.bottom]).collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    if xs.len() < 2 || ys.len() < 2 {
        return vec![];
    }
    let filled = |x: usize, y: usize| {
        if x + 1 >= xs.len() || y + 1 >= ys.len() {
            return false;
        }
        let (x, y) = ((xs[x] + xs[x + 1]) / 2., (ys[y] + ys[y + 1]) / 2.);
        regions
            .iter()
            .any(|r| x > r.left && x < r.right && y > r.top && y < r.bottom)
    };
    let mut edges = vec![];
    for y in 0..ys.len() - 1 {
        for x in 0..xs.len() - 1 {
            if !filled(x, y) {
                continue;
            }
            if y == 0 || !filled(x, y - 1) {
                edges.push(((x, y), (x + 1, y)));
            }
            if !filled(x + 1, y) {
                edges.push(((x + 1, y), (x + 1, y + 1)));
            }
            if !filled(x, y + 1) {
                edges.push(((x + 1, y + 1), (x, y + 1)));
            }
            if x == 0 || !filled(x - 1, y) {
                edges.push(((x, y + 1), (x, y)));
            }
        }
    }
    let Some((start, mut end)) = edges.pop() else {
        return vec![];
    };
    let mut vertices = vec![(xs[start.0], ys[start.1])];
    while end != start {
        vertices.push((xs[end.0], ys[end.1]));
        let Some(index) = edges.iter().position(|(from, _)| *from == end) else {
            return vec![];
        };
        end = edges.swap_remove(index).1;
    }
    let n = vertices.len();
    (0..n)
        .filter_map(|i| {
            let a = vertices[(i + n - 1) % n];
            let b = vertices[i];
            let c = vertices[(i + 1) % n];
            ((a.0 != b.0 || b.0 != c.0) && (a.1 != b.1 || b.1 != c.1)).then_some(b)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidebar_bridge_and_focused_panel_have_one_seamless_contour() {
        let regions = [
            Rect {
                left: 264.,
                top: 16.,
                right: 1400.,
                bottom: 48.,
            },
            Rect {
                left: 264.,
                top: 16.,
                right: 276.,
                bottom: 184.,
            },
            Rect {
                left: 8.,
                top: 140.,
                right: 276.,
                bottom: 184.,
            },
            Rect {
                left: 650.,
                top: 48.,
                right: 1000.,
                bottom: 984.,
            },
        ];
        let shape = outline(&regions);
        assert!(shape.contains(&(8., 140.)));
        assert!(
            !shape.contains(&(650., 16.)),
            "panel must not add a top tab"
        );
        assert!(shape.contains(&(264., 16.)));
        assert!(shape.contains(&(1400., 16.)));
        assert!(shape.contains(&(1000., 984.)));
        let area: f32 = shape
            .iter()
            .zip(shape.iter().cycle().skip(1))
            .take(shape.len())
            .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
            .sum::<f32>()
            .abs()
            / 2.;
        let expected = 1136. * 32. + 12. * (184. - 48.) + 256. * 44. + 350. * (984. - 48.);
        assert_eq!(
            area, expected,
            "one contour contains the entire union without internal seams"
        );
    }
}
