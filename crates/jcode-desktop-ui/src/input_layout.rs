//! Preserve every explicit line returned by GPUI shaping, including empty lines.
//! Byte offsets include newline separators, while visual rows include soft wraps.
use gpui::{App, Pixels, Point, Window, WrappedLine, point, px};

pub(super) struct PromptLayout {
    lines: Vec<WrappedLine>,
}

impl PromptLayout {
    pub(super) fn new(lines: impl IntoIterator<Item = WrappedLine>) -> Self {
        Self {
            lines: lines.into_iter().collect(),
        }
    }

    pub(super) fn visual_line_count(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.wrap_boundaries().len() + 1)
            .sum()
    }

    pub(super) fn position_for_index(
        &self,
        index: usize,
        line_height: Pixels,
    ) -> Option<Point<Pixels>> {
        let mut start = 0;
        let mut top = px(0.);
        for line in &self.lines {
            if index <= start + line.len() {
                return line
                    .position_for_index(index - start, line_height)
                    .map(|position| position + point(px(0.), top));
            }
            start += line.len() + 1;
            top += line_height * (line.wrap_boundaries().len() + 1);
        }
        None
    }

    pub(super) fn closest_index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        if position.y < px(0.) {
            return Err(0);
        }
        let mut start = 0;
        let mut top = px(0.);
        for line in &self.lines {
            let height = line_height * (line.wrap_boundaries().len() + 1);
            if position.y < top + height {
                return line
                    .closest_index_for_position(position - point(px(0.), top), line_height)
                    .map(|index| start + index)
                    .map_err(|index| start + index);
            }
            start += line.len() + 1;
            top += height;
        }
        Err(start.saturating_sub(1))
    }

    pub(super) fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<()> {
        let mut top = px(0.);
        for line in &self.lines {
            line.paint(
                origin + point(px(0.), top),
                line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )?;
            top += line_height * (line.wrap_boundaries().len() + 1);
        }
        Ok(())
    }
}
