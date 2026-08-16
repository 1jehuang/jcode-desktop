//! Theme: one dark, quiet palette for the whole app.

use gpui::{Hsla, Rgba, rgb, rgba};

pub struct Theme;

#[allow(dead_code)]
impl Theme {
    pub const BG: Rgba = rgb_c(0x16161e);
    pub const CANVAS_DOT: Rgba = rgba_c(0x3b3b52_60);
    pub const PANEL_BG: Rgba = rgb_c(0x1e1e2a);
    pub const PANEL_BORDER: Rgba = rgb_c(0x2c2c3d);
    pub const PANEL_BORDER_FOCUS: Rgba = rgb_c(0x7aa2f7);
    pub const HEADER_BG: Rgba = rgb_c(0x232333);
    pub const TEXT: Rgba = rgb_c(0xc8ccd4);
    pub const TEXT_DIM: Rgba = rgb_c(0x6b7089);
    pub const TEXT_USER: Rgba = rgb_c(0xe0e6f0);
    pub const ACCENT: Rgba = rgb_c(0x7aa2f7);
    pub const ACCENT_DIM: Rgba = rgba_c(0x7aa2f7_28);
    pub const USER_BG: Rgba = rgba_c(0x7aa2f7_14);
    pub const USER_BAR: Rgba = rgb_c(0x7aa2f7);
    pub const TOOL_BG: Rgba = rgba_c(0x9ece6a_10);
    pub const TOOL_TEXT: Rgba = rgb_c(0x9ece6a);
    pub const REASONING: Rgba = rgb_c(0x565f89);
    pub const CODE_BG: Rgba = rgb_c(0x14141c);
    pub const CODE_TEXT: Rgba = rgb_c(0xa9b1d6);
    pub const INLINE_CODE_BG: Rgba = rgba_c(0x414868_60);
    pub const INPUT_BG: Rgba = rgb_c(0x1a1a26);
    pub const INPUT_BORDER: Rgba = rgb_c(0x33334a);
    pub const CURSOR: Rgba = rgb_c(0x7aa2f7);
    pub const SELECTION: Rgba = rgba_c(0x7aa2f7_40);
    pub const ERROR: Rgba = rgb_c(0xf7768e);
    pub const OK: Rgba = rgb_c(0x9ece6a);
    pub const WARN: Rgba = rgb_c(0xe0af68);
    pub const HEADING: Rgba = rgb_c(0xbb9af7);
    pub const LINK: Rgba = rgb_c(0x7dcfff);

    pub const FONT_UI: &'static str = "JetBrainsMono Nerd Font";
    pub const FONT_MONO: &'static str = "JetBrainsMono Nerd Font";
}

pub fn to_hsla(color: Rgba) -> Hsla {
    color.into()
}

const fn rgb_c(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

const fn rgba_c(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 24) & 0xff) as f32 / 255.0,
        g: ((hex >> 16) & 0xff) as f32 / 255.0,
        b: ((hex >> 8) & 0xff) as f32 / 255.0,
        a: (hex & 0xff) as f32 / 255.0,
    }
}

// Keep rgb/rgba imports alive for callers that construct ad-hoc colors.
#[allow(dead_code)]
pub fn _unused() {
    let _ = rgb(0);
    let _ = rgba(0);
}
