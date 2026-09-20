//! Rounded inline-code fills, painted behind the original selectable text.
use std::ops::Range;

use gpui::{Bounds, Pixels, TextAlign, TextLayout, canvas, div, point, prelude::*, px, size};

use crate::theme::Theme;

// A small visual inset leaves breathing room without inserting characters into
// selectable/copyable code or changing the paragraph's native wrap positions.
const HORIZONTAL_PADDING: f32 = 3.0;

pub(super) fn wrap(
    child: gpui::AnyElement,
    layout: TextLayout,
    ranges: Vec<Range<usize>>,
) -> gpui::AnyElement {
    if ranges.is_empty() {
        return child;
    }
    div()
        .relative()
        .min_w_0()
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    for bounds in backgrounds(&layout, &ranges, window.text_style().text_align) {
                        let mut quad = gpui::fill(bounds, Theme::global().INLINE_CODE_BG);
                        quad.corner_radii = px(3.).into();
                        window.paint_quad(quad);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(child)
        .into_any_element()
}

// Work from shaped wrap boundaries, not character widths: this keeps Unicode,
// ligatures, mixed fonts and exact wrap endpoints aligned with the text. The
// background layer neither changes layout nor intercepts links or selection.
fn backgrounds(
    layout: &TextLayout,
    ranges: &[Range<usize>],
    align: TextAlign,
) -> Vec<Bounds<Pixels>> {
    let bounds = layout.bounds();
    let height = layout.line_height();
    let mut origin = bounds.origin;
    let mut offset = 0;
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
            let start_x = unwrapped.x_for_index(start);
            let width = unwrapped.x_for_index(end) - start_x;
            let alignment = match align {
                TextAlign::Left => px(0.),
                TextAlign::Center => (bounds.size.width - width) / 2.,
                TextAlign::Right => bounds.size.width - width,
            };
            for range in ranges {
                let lo = range.start.max(offset + start);
                let hi = range.end.min(offset + end);
                if lo >= hi {
                    continue;
                }
                let left = unwrapped.x_for_index(lo - offset) - start_x;
                let right = unwrapped.x_for_index(hi - offset) - start_x;
                if right > left {
                    result.push(Bounds::new(
                        point(
                            origin.x + alignment + left - px(HORIZONTAL_PADDING),
                            origin.y,
                        ),
                        size(right - left + px(2.0 * HORIZONTAL_PADDING), height),
                    ));
                }
            }
            start = end;
            origin.y += height;
        }
        offset += line.len() + 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, StyledText};
    use std::{cell::RefCell, rc::Rc};

    struct Fixture {
        layout: TextLayout,
        output: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    }
    impl Render for Fixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let text = StyledText::new("prefix diff_tool_result diff_model café🙂 tail");
            self.layout = text.layout().clone();
            let layout = self.layout.clone();
            let output = self.output.clone();
            let ranges = vec![7..33, 34..43];
            div()
                .w(px(160.))
                .font_family("monospace")
                .text_size(px(14.))
                .line_height(px(22.))
                .child(wrap(
                    text.into_any_element(),
                    layout.clone(),
                    ranges.clone(),
                ))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, _, _| {
                            *output.borrow_mut() = backgrounds(&layout, &ranges, TextAlign::Left);
                        },
                    )
                    .absolute(),
                )
        }
    }

    #[gpui::test]
    fn rounded_code_backgrounds_follow_wrapping_and_unicode(cx: &mut gpui::TestAppContext) {
        let output = Rc::new(RefCell::new(Vec::new()));
        let (view, vcx) = cx.add_window_view(|_, _| Fixture {
            layout: TextLayout::default(),
            output: output.clone(),
        });
        vcx.run_until_parked();
        let painted = output.borrow();
        assert!(
            painted.len() >= 3,
            "long code wraps into separate rounded fills: {painted:?}"
        );
        view.read_with(vcx, |view, _| {
            let text = view.layout.bounds();
            for rect in painted.iter() {
                assert_eq!(rect.size.height, px(22.));
                assert!(rect.size.width > px(0.));
                assert!(rect.left() >= text.left() - px(HORIZONTAL_PADDING));
                assert!(rect.right() <= text.right() + px(HORIZONTAL_PADDING + 1.));
                assert!(rect.top() >= text.top() && rect.bottom() <= text.bottom() + px(1.));
            }
            // Verify the fill leaves exactly three pixels on either side of
            // the shaped glyphs, while keeping the existing vertical geometry.
            let padded = backgrounds(&view.layout, &[0..6], TextAlign::Left);
            assert_eq!(padded.len(), 1);
            let lines = view.layout.line_layouts();
            let line = &lines[0].unwrapped_layout;
            assert_eq!(padded[0].left(), text.left() + line.x_for_index(0) - px(3.));
            assert_eq!(
                padded[0].right(),
                text.left() + line.x_for_index(6) + px(3.)
            );
            assert_eq!(padded[0].top(), text.top());
            assert!(backgrounds(&view.layout, &[], TextAlign::Left).is_empty());
            assert!(backgrounds(&view.layout, &[7..7], TextAlign::Left).is_empty());
        });
    }
}
