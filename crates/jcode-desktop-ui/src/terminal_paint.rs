//! GPUI presentation only. All terminal state and protocol semantics live in Handterm.
use super::*;
use gpui::{
    App, ContentMask, FontStyle, FontWeight, Hsla, PaintQuad, ShapedLine, TextAlign, TextRun, fill,
    point, rgb, size,
};
use handterm_common::{
    grid::{
        ATTR_BOLD, ATTR_ITALIC, ATTR_STRIKETHROUGH, ATTR_UNDERLINE, FLAG_WIDE, FLAG_WIDE_CONT,
        UnderlineStyle,
    },
    terminal::CursorStyle,
    visual::{is_in_selection, resolve_cell_colors, resolve_underline_color},
};
use std::hash::{Hash, Hasher};

pub(super) struct Scene {
    backgrounds: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    images: Vec<(Bounds<Pixels>, Arc<RenderImage>)>,
    decorations: Vec<PaintQuad>,
}

impl Scene {
    pub(super) fn paint(self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in self.backgrounds {
                window.paint_quad(quad);
            }
            for (origin, line) in self.lines {
                let _ = line.paint(origin, px(LINE_HEIGHT), TextAlign::Left, None, window, cx);
            }
            for (image_bounds, image) in self.images {
                let _ =
                    window.paint_image(bounds, image_bounds, Default::default(), image, 0, false);
            }
            for quad in self.decorations {
                window.paint_quad(quad);
            }
        });
    }
}

fn packed(color: gpui::Rgba) -> u32 {
    ((color.r * 255.).round() as u32) << 16
        | ((color.g * 255.).round() as u32) << 8
        | (color.b * 255.).round() as u32
}

pub(super) fn bgra_image(
    image: &handterm_common::graphics::KittyImage,
) -> Option<image::RgbaImage> {
    let mut data = image.data.clone();
    // GPUI's atlas consumes BGRA pixels, despite image::Frame's RGBA container.
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    image::RgbaImage::from_raw(image.width, image.height, data)
}

impl TerminalPanel {
    fn sync_images(&mut self, window: &mut Window) {
        let generation = self.terminal.kitty_generation();
        if self.image_generation == Some(generation) {
            return;
        }
        self.image_generation = Some(generation);
        self.images.retain(|id, cached| {
            if self.terminal.kitty_image(*id).is_some() {
                true
            } else {
                let _ = window.drop_image(cached.image.clone());
                false
            }
        });
        for image in self.terminal.kitty_images() {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            image.width.hash(&mut hash);
            image.height.hash(&mut hash);
            image.data.hash(&mut hash);
            let fingerprint = hash.finish();
            if self
                .images
                .get(&image.id)
                .is_some_and(|cached| cached.fingerprint == fingerprint)
            {
                continue;
            }
            if let Some(pixels) = bgra_image(image) {
                let render_image = self.image_ids.render(vec![image::Frame::new(pixels)]);
                if let Some(old) = self.images.insert(
                    image.id,
                    CachedImage {
                        fingerprint,
                        image: render_image,
                    },
                ) {
                    let _ = window.drop_image(old.image);
                }
            }
        }
    }

    pub(super) fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Scene {
        let theme = Theme::global();
        let mut style = window.text_style();
        style.font_family = theme.FONT_MONO.into();
        style.font_size = px(FONT_SIZE).into();
        let cell_width = window
            .text_system()
            .shape_line("M".into(), px(FONT_SIZE), &[style.to_run(1)], None)
            .width
            .max(px(1.));
        self.resize(bounds, cell_width, cx);
        self.sync_images(window);
        let mut scene = Scene {
            backgrounds: Vec::new(),
            lines: Vec::new(),
            images: Vec::new(),
            decorations: Vec::new(),
        };
        let grid = &self.terminal.grid;
        let base_fg = packed(theme.TEXT);
        let base_bg = packed(theme.BG);
        let cursor = grid.cursor_pos();
        let show_cursor = self.terminal.cursor_visible && grid.scroll_offset == 0 && !self.exited;
        let focused = self.focus.is_focused(window);
        for row in 0..grid.rows {
            let mut col = 0;
            while col < grid.cols {
                let cell = grid.cell_at_scroll(row, col);
                if cell.flags & FLAG_WIDE_CONT != 0 {
                    col += 1;
                    continue;
                }
                let start_col = col;
                let selected = is_in_selection(grid.selection, row, col);
                let block_cursor = show_cursor
                    && focused
                    && cursor == (col, row)
                    && self.terminal.cursor_style == CursorStyle::Block;
                let colors = resolve_cell_colors(cell, base_fg, base_bg, block_cursor, selected);
                let mut text = grid
                    .cell_grapheme_at_scroll(row, col)
                    .map(str::to_owned)
                    .unwrap_or_else(|| cell.char_display().to_string());
                let width = if cell.flags & FLAG_WIDE != 0 { 2 } else { 1 };
                col += width;
                // Shape consecutive ASCII cells as a run, not thousands of GPUI elements.
                // Unicode graphemes remain anchored to their cell to avoid fallback-font drift.
                let ascii = text.len() == 1 && text.is_ascii();
                if ascii {
                    while col < grid.cols {
                        let next = grid.cell_at_scroll(row, col);
                        if next.flags != 0
                            || next.attrs != cell.attrs
                            || next.fg != cell.fg
                            || next.bg != cell.bg
                            || next.underline_style != cell.underline_style
                            || next.underline_color != cell.underline_color
                            || next.ch > 127
                            || grid.cell_grapheme_at_scroll(row, col).is_some()
                            || is_in_selection(grid.selection, row, col) != selected
                            || block_cursor
                            || (show_cursor && cursor == (col, row))
                        {
                            break;
                        }
                        text.push(next.char_display());
                        col += 1;
                    }
                }
                let origin = bounds.origin + point(cell_width * start_col, px(LINE_HEIGHT) * row);
                let run_bounds = Bounds::new(
                    origin,
                    size(cell_width * (col - start_col), px(LINE_HEIGHT)),
                );
                if colors.bg != base_bg {
                    scene.backgrounds.push(fill(run_bounds, rgb(colors.bg)));
                }
                if text.trim().is_empty() && cell.attrs & (ATTR_UNDERLINE | ATTR_STRIKETHROUGH) == 0
                {
                    continue;
                }
                let mut font = style.font();
                if cell.attrs & ATTR_BOLD != 0 {
                    font.weight = FontWeight::BOLD;
                }
                if cell.attrs & ATTR_ITALIC != 0 {
                    font.style = FontStyle::Italic;
                }
                let color: Hsla = rgb(colors.fg).into();
                let run = TextRun {
                    len: text.len(),
                    font,
                    color,
                    background_color: None,
                    underline: (cell.attrs & ATTR_UNDERLINE != 0).then(|| gpui::UnderlineStyle {
                        thickness: px(1.),
                        color: Some(rgb(resolve_underline_color(cell, colors.fg)).into()),
                        wavy: cell.underline_style == UnderlineStyle::Curly,
                    }),
                    strikethrough: (cell.attrs & ATTR_STRIKETHROUGH != 0).then_some(
                        gpui::StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(color),
                        },
                    ),
                };
                let shaped = window.text_system().shape_line(
                    text.into(),
                    px(FONT_SIZE),
                    &[run],
                    ascii.then_some(cell_width),
                );
                scene.lines.push((origin, shaped));
            }
        }
        for placement in self.terminal.kitty_placements() {
            if let Some(image) = self.images.get(&placement.image_id) {
                scene.images.push((
                    Bounds::new(
                        bounds.origin
                            + point(cell_width * placement.col, px(LINE_HEIGHT) * placement.row),
                        size(
                            cell_width * placement.cols.max(1),
                            px(LINE_HEIGHT) * placement.rows.max(1),
                        ),
                    ),
                    image.image.clone(),
                ));
            }
        }
        if show_cursor {
            let origin = bounds.origin + point(cell_width * cursor.0, px(LINE_HEIGHT) * cursor.1);
            let cursor_color = theme.CURSOR;
            if !focused {
                // A hollow cursor preserves position without pretending this pane has focus.
                for rect in [
                    Bounds::new(origin, size(cell_width, px(1.))),
                    Bounds::new(
                        origin + point(px(0.), px(LINE_HEIGHT - 1.)),
                        size(cell_width, px(1.)),
                    ),
                    Bounds::new(origin, size(px(1.), px(LINE_HEIGHT))),
                    Bounds::new(
                        origin + point(cell_width - px(1.), px(0.)),
                        size(px(1.), px(LINE_HEIGHT)),
                    ),
                ] {
                    scene.decorations.push(fill(rect, cursor_color));
                }
            } else {
                match self.terminal.cursor_style {
                    CursorStyle::Bar => scene.decorations.push(fill(
                        Bounds::new(origin, size(px(2.), px(LINE_HEIGHT))),
                        cursor_color,
                    )),
                    CursorStyle::Underline => scene.decorations.push(fill(
                        Bounds::new(
                            origin + point(px(0.), px(LINE_HEIGHT - 2.)),
                            size(cell_width, px(2.)),
                        ),
                        cursor_color,
                    )),
                    CursorStyle::Block => {}
                }
            }
        }
        if !self.status.is_empty() || self.exited {
            let text = if self.exited {
                "[process exited]".to_string()
            } else {
                self.status.clone()
            };
            let mut style = style.clone();
            style.color = theme.TEXT_DIM.into();
            let shaped = window.text_system().shape_line(
                text.clone().into(),
                px(FONT_SIZE),
                &[style.to_run(text.len())],
                None,
            );
            let origin = if self.exited {
                point(bounds.origin.x, bounds.bottom() - px(LINE_HEIGHT))
            } else {
                bounds.origin
            };
            scene.backgrounds.push(fill(
                Bounds::new(origin, size(bounds.size.width, px(LINE_HEIGHT))),
                theme.BG,
            ));
            scene.lines.push((origin, shaped));
        }
        self.terminal.grid.clear_dirty();
        scene
    }
}
