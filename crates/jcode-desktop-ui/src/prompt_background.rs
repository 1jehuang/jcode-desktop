//! A single, softly stepped fill behind native visual text lines.
//!
//! The parent reserves 8px horizontally and 4px vertically. This wrapper adds
//! no padding, text, hitboxes or clipping and leaves the original selectable,
//! link-bearing child intact. Geometry is read during paint, after prepaint has
//! populated the shared TextLayout with its final origin and native wrapping.
use gpui::{
    AnyElement, Bounds, PathBuilder, Pixels, Rgba, TextAlign, TextLayout, canvas, div, point,
    prelude::*, px, size,
};

const HORIZONTAL_PADDING: f32 = 8.;
const VERTICAL_PADDING: f32 = 4.;
const RADIUS: f32 = 12.;

/// Structured Markdown fallback. As with `wrap`, the parent reserves padding.
pub(crate) fn wrap_block(child: AnyElement, color: Rgba) -> AnyElement {
    div()
        .relative()
        .min_w_0()
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, window, _| {
                    let padded = Bounds::new(
                        point(
                            bounds.left() - px(HORIZONTAL_PADDING),
                            bounds.top() - px(VERTICAL_PADDING),
                        ),
                        size(
                            bounds.size.width + px(2. * HORIZONTAL_PADDING),
                            bounds.size.height + px(2. * VERTICAL_PADDING),
                        ),
                    );
                    let mut quad = gpui::fill(padded, color);
                    quad.corner_radii = px(RADIUS).into();
                    window.paint_quad(quad);
                },
            )
            .absolute()
            .size_full(),
        )
        .child(child)
        .into_any_element()
}

pub(crate) fn wrap(child: AnyElement, layout: TextLayout, color: Rgba) -> AnyElement {
    div()
        .relative()
        .min_w_0()
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let lines = visual_lines(&layout, window.text_style().text_align);
                    let vertices = contour(&lines);
                    if vertices.len() < 3 {
                        return;
                    }
                    let mut path = PathBuilder::fill();
                    for i in 0..vertices.len() {
                        let previous = vertices[(i + vertices.len() - 1) % vertices.len()];
                        let vertex = vertices[i];
                        let next = vertices[(i + 1) % vertices.len()];
                        let radius = RADIUS
                            .min(distance(previous, vertex) / 2.)
                            .min(distance(vertex, next) / 2.);
                        let enter = toward(vertex, previous, radius);
                        let exit = toward(vertex, next, radius);
                        if i == 0 {
                            path.move_to(point(px(enter.0), px(enter.1)));
                        } else {
                            path.line_to(point(px(enter.0), px(enter.1)));
                        }
                        path.curve_to(
                            point(px(exit.0), px(exit.1)),
                            point(px(vertex.0), px(vertex.1)),
                        );
                    }
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(child)
        .into_any_element()
}

fn visual_lines(layout: &TextLayout, align: TextAlign) -> Vec<Bounds<Pixels>> {
    let bounds = layout.bounds();
    let height = layout.line_height();
    if height <= px(0.) {
        return Vec::new();
    }
    let mut y = bounds.top();
    let mut result = Vec::new();
    for line in layout.line_layouts() {
        let unwrapped = &line.unwrapped_layout;
        let ends = line
            .wrap_boundaries
            .iter()
            .map(|boundary| unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
            .chain([line.len()]);
        let mut start = 0;
        for end in ends {
            // Indices belong to the shaped glyphs, never UTF-8 character counts.
            let width = unwrapped.x_for_index(end) - unwrapped.x_for_index(start);
            let offset = match align {
                TextAlign::Left => px(0.),
                TextAlign::Center => (bounds.size.width - width) / 2.,
                TextAlign::Right => bounds.size.width - width,
            };
            result.push(Bounds::new(
                point(
                    bounds.left() + offset - px(HORIZONTAL_PADDING),
                    y - px(VERTICAL_PADDING),
                ),
                size(
                    width + px(2. * HORIZONTAL_PADDING),
                    height + px(2. * VERTICAL_PADDING),
                ),
            ));
            start = end;
            y += height;
        }
    }
    result
}

type Vertex = (f32, f32);

// Union the overlapping padded lines into horizontal bands, then walk the
// outside exactly once. One tessellated fill avoids both antialiasing slits and
// double-alpha seams. Blank explicit lines retain their 16px padded footprint.
// Native left/center/right alignment guarantees overlapping horizontal spans.
fn contour(lines: &[Bounds<Pixels>]) -> Vec<Vertex> {
    let mut edges: Vec<f32> = lines
        .iter()
        .flat_map(|line| [f32::from(line.top()), f32::from(line.bottom())])
        .collect();
    edges.sort_by(f32::total_cmp);
    edges.dedup();
    let mut bands = Vec::new();
    let mut first_active = 0;
    for pair in edges.windows(2) {
        let middle = (pair[0] + pair[1]) / 2.;
        // Native lines have ordered tops and bottoms. Retire completed lines
        // once, and inspect only the small overlapping window for this band.
        while first_active < lines.len() && f32::from(lines[first_active].bottom()) <= middle {
            first_active += 1;
        }
        let mut left = f32::INFINITY;
        let mut right = f32::NEG_INFINITY;
        for line in &lines[first_active..] {
            if f32::from(line.top()) >= middle {
                break;
            }
            left = left.min(f32::from(line.left()));
            right = right.max(f32::from(line.right()));
        }
        if left < right {
            bands.push((left, right, pair[0], pair[1]));
        }
    }
    let mut vertices = Vec::new();
    for &(_, right, top, bottom) in &bands {
        vertices.push((right, top));
        vertices.push((right, bottom));
    }
    for &(left, _, top, bottom) in bands.iter().rev() {
        vertices.push((left, bottom));
        vertices.push((left, top));
    }
    vertices.dedup();
    // Remove straight-edge vertices before choosing radii, so equal-width
    // neighbors have a continuous edge, not repeated rounded corners.
    let original = vertices.clone();
    vertices = original
        .iter()
        .enumerate()
        .filter_map(|(i, &p)| {
            let previous = original[(i + original.len() - 1) % original.len()];
            let next = original[(i + 1) % original.len()];
            if (previous.0 == p.0 && p.0 == next.0) || (previous.1 == p.1 && p.1 == next.1) {
                None
            } else {
                Some(p)
            }
        })
        .collect();
    vertices
}

fn distance(a: Vertex, b: Vertex) -> f32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}

fn toward(from: Vertex, to: Vertex, amount: f32) -> Vertex {
    let ratio = amount / distance(from, to);
    (
        from.0 + (to.0 - from.0) * ratio,
        from.1 + (to.1 - from.1) * ratio,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, StyledText};
    use std::{cell::RefCell, rc::Rc};

    fn line(width: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(-8.), px(y - 4.)), size(px(width + 16.), px(30.)))
    }

    #[test]
    fn equal_width_lines_have_only_four_outer_corners() {
        let outline = contour(&[line(100., 0.), line(100., 22.), line(100., 44.)]);
        assert_eq!(
            outline,
            vec![(108., -4.), (108., 70.), (-8., 70.), (-8., -4.)]
        );
        assert!(contour(&[]).is_empty());
    }

    #[test]
    fn unequal_lines_form_a_connected_stepped_union() {
        let outline = contour(&[line(100., 0.), line(40., 22.), line(80., 44.)]);
        assert_eq!(
            outline,
            vec![
                (108., -4.),
                (108., 26.),
                (48., 26.),
                (48., 40.),
                (88., 40.),
                (88., 70.),
                (-8., 70.),
                (-8., -4.),
            ]
        );
        for i in 0..outline.len() {
            let a = outline[i];
            let b = outline[(i + 1) % outline.len()];
            assert!(a.0 == b.0 || a.1 == b.1);
            assert!(distance(a, b) > 0.);
        }
    }

    struct Fixture {
        text: &'static str,
        layout: TextLayout,
        output: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    }

    impl Render for Fixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let text = StyledText::new(self.text);
            self.layout = text.layout().clone();
            let layout = self.layout.clone();
            let output = self.output.clone();
            div()
                .w(px(160.))
                .font_family("monospace")
                .text_size(px(14.))
                .line_height(px(22.))
                .child(wrap(
                    text.into_any_element(),
                    layout.clone(),
                    gpui::rgba(0x77777780),
                ))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, _, _| {
                            *output.borrow_mut() = visual_lines(&layout, TextAlign::Left);
                        },
                    )
                    .absolute(),
                )
        }
    }

    #[gpui::test]
    fn native_wrapping_and_unicode_widths(cx: &mut gpui::TestAppContext) {
        let output = Rc::new(RefCell::new(Vec::new()));
        let (view, vcx) = cx.add_window_view(|_, _| Fixture {
            text: "café🙂 long native wrapped text with unequal visual lines",
            layout: TextLayout::default(),
            output: output.clone(),
        });
        vcx.run_until_parked();
        let lines = output.borrow();
        assert!(lines.len() >= 3);
        view.read_with(vcx, |view, _| {
            let shaped = view.layout.line_layouts();
            assert_eq!(
                lines.len(),
                shaped
                    .iter()
                    .map(|line| line.wrap_boundaries.len() + 1)
                    .sum::<usize>()
            );
            let first = &shaped[0];
            let boundary = first.wrap_boundaries[0];
            let end = first.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
            assert_eq!(
                lines[0].size.width,
                first.unwrapped_layout.x_for_index(end) + px(16.)
            );
            for (i, line) in lines.iter().enumerate() {
                assert_eq!(
                    line.top(),
                    view.layout.bounds().top() + px(i as f32 * 22. - 4.)
                );
                assert!(line.right() <= view.layout.bounds().right() + px(9.));
            }
        });
    }

    #[gpui::test]
    fn explicit_and_blank_newlines_keep_native_line_geometry(cx: &mut gpui::TestAppContext) {
        let output = Rc::new(RefCell::new(Vec::new()));
        let (view, vcx) = cx.add_window_view(|_, _| Fixture {
            text: "longer line\nx\n\ncafé🙂",
            layout: TextLayout::default(),
            output: output.clone(),
        });
        vcx.run_until_parked();
        let lines = output.borrow();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].size.width > lines[1].size.width);
        assert_eq!(lines[2].size.width, px(16.));
        view.read_with(vcx, |view, _| {
            for (rect, shaped) in lines.iter().zip(view.layout.line_layouts().iter()) {
                assert_eq!(
                    rect.size.width,
                    shaped.unwrapped_layout.x_for_index(shaped.len()) + px(16.)
                );
            }
            assert_eq!(
                lines.last().unwrap().bottom(),
                view.layout.bounds().top() + px(4. * 22. + 4.)
            );
        });
        assert!(!contour(&lines).is_empty());
    }
}
