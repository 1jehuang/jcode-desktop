//! IME queries can run synchronously after an edit, before the next paint.
//! Keep the painted typography, but always shape the current document rather
//! than indexing the previous frame (or its placeholder) with new offsets.
use gpui::{Bounds, Pixels, Point, SharedString, TextRun, Window, point, px, size};
use std::ops::Range;

#[derive(Clone)]
pub struct ImeGeometry {
    font_size: Pixels,
    line_height: Pixels,
    run: TextRun,
    wrap_width: Option<Pixels>,
}

impl ImeGeometry {
    pub fn new(
        font_size: Pixels,
        line_height: Pixels,
        mut run: TextRun,
        wrap_width: Option<Pixels>,
    ) -> Self {
        run.underline = None;
        Self {
            font_size,
            line_height,
            run,
            wrap_width,
        }
    }

    /// `range` is in UTF-8 bytes of `text`; `origin` is window-local.
    /// Return only the first visual line, as required by NSTextInputClient.
    /// Callers with secrets must pass their masked display string, never secrets.
    pub fn bounds_for_range(
        &self,
        text: SharedString,
        range: Range<usize>,
        origin: Point<Pixels>,
        window: &Window,
    ) -> Option<Bounds<Pixels>> {
        let mut run = self.run.clone();
        run.len = text.len();
        let lines = window
            .text_system()
            .shape_text(text, self.font_size, &[run], self.wrap_width, None)
            .ok()?;
        let mut offset = 0;
        let mut top = px(0.);
        for line in lines {
            if range.start <= offset + line.len() {
                let start_index = range.start - offset;
                let start = line.position_for_index(start_index, self.line_height)?;
                // A multi-line range must not yield a rectangle stretching
                // across rows (or a negative width at a soft wrap).
                let visual_end = line
                    .wrap_boundaries()
                    .iter()
                    .map(|boundary| line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index)
                    .find(|&index| index >= start_index)
                    .unwrap_or(line.len());
                let end_index = range
                    .end
                    .saturating_sub(offset)
                    .max(start_index)
                    .min(visual_end);
                let end = line.position_for_index(end_index, self.line_height)?;
                return Some(Bounds::new(
                    origin + start + point(px(0.), top),
                    size((end.x - start.x).max(px(0.)), self.line_height),
                ));
            }
            offset += line.len() + 1;
            top += self.line_height * (line.wrap_boundaries().len() + 1);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, div};

    struct View;
    impl Render for View {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn geometry_keeps_window_origin_and_first_visual_line(cx: &mut gpui::TestAppContext) {
        let handle = cx.add_window(|_, _| View);
        handle
            .update(cx, |_, window, _| {
                let style = window.text_style();
                let run = TextRun {
                    len: 0,
                    font: style.font(),
                    color: style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let geometry = ImeGeometry::new(px(16.), px(24.), run, Some(px(80.)));
                let origin = point(px(123.), px(456.));
                let empty = geometry
                    .bounds_for_range("".into(), 0..0, origin, window)
                    .unwrap();
                assert_eq!(empty, Bounds::new(origin, size(px(0.), px(24.))));
                let text: SharedString = "a\nbc".into();
                let second = geometry
                    .bounds_for_range(text.clone(), 2..4, origin, window)
                    .unwrap();
                assert_eq!(second.origin, origin + point(px(0.), px(24.)));
                let first = geometry
                    .bounds_for_range(text, 0..4, origin, window)
                    .unwrap();
                assert_eq!(first.origin, origin);
                assert_eq!(first.size.height, px(24.));
                assert!(first.size.width >= px(0.));
                let wrapped: SharedString = "abcdefghijklmnopqrstuvwxyz".into();
                let rect = geometry
                    .bounds_for_range(wrapped.clone(), 0..wrapped.len(), origin, window)
                    .unwrap();
                assert_eq!(rect.origin, origin);
                assert_eq!(rect.size.height, px(24.));
                assert!(rect.size.width <= px(80.));
            })
            .unwrap();
    }
}
