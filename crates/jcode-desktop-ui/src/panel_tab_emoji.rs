//! Two cached native-emoji poses, with a paint-driven clock instead of a
//! display-rate animation. GPUI can rotate SVGs, but not color text glyphs.
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use gpui::{
    Context, FontRun, PlatformTextSystem, Render, RenderGlyphParams, RenderImage, Task, Window,
    canvas, div, img, prelude::*, px,
};

const TICK: Duration = Duration::from_millis(280);
const ANGLE: f32 = 12.0;
const RASTER_SIZE: u32 = 80;
type Frames = [Arc<RenderImage>; 2];

// A headless platform gives us the public native font rasterizer without
// opening a window. Reuse it and the two textures across all matching tabs.
static TEXT: LazyLock<Arc<dyn PlatformTextSystem>> =
    LazyLock::new(|| gpui_platform::current_platform(true).text_system());
static FRAMES: LazyLock<Mutex<HashMap<&'static str, Option<Frames>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) struct TabEmoji {
    emoji: &'static str,
    step: usize,
    tick: Option<Task<()>>,
}

impl TabEmoji {
    pub(super) fn new(emoji: &'static str, _: &mut Context<Self>) -> Self {
        Self {
            emoji,
            step: 0,
            tick: None,
        }
    }

    fn arm(&mut self, reduce_motion: bool, cx: &mut Context<Self>) {
        if reduce_motion || self.tick.is_some() {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TICK).await;
            let _ = this.update(cx, |this, cx| {
                this.tick = None;
                this.step ^= 1;
                cx.notify();
            });
        }));
    }
}

fn pose_angle(step: usize) -> f32 {
    if step % 2 == 0 { -ANGLE } else { ANGLE }
}

fn rotate(source: &image::RgbaImage, degrees: f32) -> image::RgbaImage {
    let (sin, cos) = degrees.to_radians().sin_cos();
    image::RgbaImage::from_fn(RASTER_SIZE, RASTER_SIZE, |x, y| {
        let x = x as f32 + 0.5 - RASTER_SIZE as f32 / 2.0;
        let y = y as f32 + 0.5 - RASTER_SIZE as f32 / 2.0;
        let sx = (cos * x + sin * y + source.width() as f32 / 2.0).floor();
        let sy = (-sin * x + cos * y + source.height() as f32 / 2.0).floor();
        if sx >= 0.0 && sy >= 0.0 && sx < source.width() as f32 && sy < source.height() as f32 {
            *source.get_pixel(sx as u32, sy as u32)
        } else {
            image::Rgba([0; 4])
        }
    })
}

fn raster_frames(emoji: &'static str) -> Option<Frames> {
    // The platform's default family may not be installed (e.g. IBM Plex on
    // Linux). Shape using an installed base font and let native font fallback
    // select the emoji face, just as normal text does. Do not load emoji-only
    // fonts as a base font: Cosmic Text expects base fonts to contain 'm'.
    let font_id = TEXT
        .font_id(&gpui::font(".SystemUIFont"))
        .ok()
        .or_else(|| {
            TEXT.all_font_names()
                .into_iter()
                .filter(|family| !family.to_ascii_lowercase().contains("emoji"))
                .find_map(|family| TEXT.font_id(&gpui::font(family)).ok())
        })?;
    let line = TEXT.layout_line(
        emoji,
        px(14.0),
        &[FontRun {
            len: emoji.len(),
            font_id,
        }],
    );
    let run = line.runs.first()?;
    let glyph = run.glyphs.first()?;
    // Session icons are single-codepoint emoji. Fall back to ordinary text if
    // the native font cannot supply a color glyph (including test platforms).
    if !glyph.is_emoji || line.runs.len() != 1 || run.glyphs.len() != 1 {
        return None;
    }
    let params = RenderGlyphParams {
        font_id: run.font_id,
        glyph_id: glyph.id,
        font_size: px(14.0),
        subpixel_variant: Default::default(),
        scale_factor: 4.0,
        is_emoji: true,
        subpixel_rendering: false,
        dilation: 0,
    };
    let bounds = TEXT.glyph_raster_bounds(&params).ok()?;
    let (size, bytes) = TEXT.rasterize_glyph(&params, bounds).ok()?;
    // Native color glyphs and RenderImage both use BGRA, no channel swap.
    let source = image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)?;
    Some(std::array::from_fn(|step| {
        Arc::new(RenderImage::new(vec![image::Frame::new(rotate(
            &source,
            pose_angle(step),
        ))]))
    }))
}

impl Render for TabEmoji {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reduced = crate::config::get().appearance.reduce_motion;
        let frames = if reduced {
            None
        } else {
            FRAMES
                .lock()
                .unwrap()
                .entry(self.emoji)
                .or_insert_with(|| raster_frames(self.emoji))
                .clone()
        };
        let this = cx.entity().downgrade();
        let mut icon = div()
            .debug_selector(|| "tab-activity-emoji".into())
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center();
        if let Some(frames) = frames {
            icon = icon
                .child(img(frames[self.step].clone()).size_full())
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, cx| {
                            if bounds.intersects(&window.content_mask().bounds) {
                                let _ = this.update(cx, |this, cx| {
                                    this.arm(crate::config::get().appearance.reduce_motion, cx)
                                });
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                );
        } else {
            icon = icon.child(self.emoji);
        }
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a native color emoji font"]
    fn native_emoji_produces_both_cached_poses() {
        for emoji in ["💫", "🦊", "🐱", "🐜", "🐟"] {
            let frames = raster_frames(emoji).expect("native emoji rasterization");
            assert_ne!(frames[0].id, frames[1].id);
        }
    }

    #[test]
    fn exactly_two_opposite_poses() {
        for step in 0..8 {
            assert_eq!(pose_angle(step), -pose_angle(step + 1));
            assert_eq!(pose_angle(step), pose_angle(step + 2));
        }
        let source =
            image::RgbaImage::from_fn(56, 56, |x, y| image::Rgba([x as u8, y as u8, 42, 255]));
        let left = rotate(&source, pose_angle(0));
        let right = rotate(&source, pose_angle(1));
        assert_ne!(left, right);
        for frame in [left, right] {
            assert_eq!(frame.dimensions(), (RASTER_SIZE, RASTER_SIZE));
            assert!(
                frame
                    .enumerate_pixels()
                    .filter(|(x, y, _)| *x == 0
                        || *y == 0
                        || *x == RASTER_SIZE - 1
                        || *y == RASTER_SIZE - 1)
                    .all(|(_, _, pixel)| pixel[3] == 0),
                "rotated glyph must not clip"
            );
        }
    }

    #[gpui::test]
    fn clock_alternates_only_when_painted_and_respects_reduced_motion(
        cx: &mut gpui::TestAppContext,
    ) {
        let emoji = cx.new(|cx| TabEmoji::new("🦊", cx));
        emoji.update(cx, |emoji, cx| {
            emoji.arm(true, cx);
            assert!(emoji.tick.is_none());
            emoji.arm(false, cx);
            emoji.arm(false, cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        emoji.read_with(cx, |emoji, _| {
            assert_eq!(emoji.step, 1);
            assert!(emoji.tick.is_none());
        });
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        emoji.read_with(cx, |emoji, _| assert_eq!(emoji.step, 1));
        emoji.update(cx, |emoji, cx| emoji.arm(false, cx));
        cx.run_until_parked();
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        emoji.read_with(cx, |emoji, _| assert_eq!(emoji.step, 0));
    }
}
