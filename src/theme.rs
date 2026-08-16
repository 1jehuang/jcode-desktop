//! Theme: the website's mono, monochrome, minimal language adapted to a
//! dark desktop canvas. These mirror the site's ink/muted/faint/rule/wash
//! hierarchy rather than using a conventional editor accent palette.

use gpui::{Hsla, Rgba, rgb, rgba};

pub struct Theme;

#[allow(dead_code)]
impl Theme {
    pub const BG: Rgba = rgb_c(0x090909);
    pub const CANVAS_DOT: Rgba = rgba_c(0xffffff_0d);
    pub const PANEL_BG: Rgba = rgb_c(0x111111);
    pub const PANEL_BORDER: Rgba = rgb_c(0x333333);
    pub const PANEL_BORDER_FOCUS: Rgba = rgb_c(0xf4f4f4);
    /// niri `focus-ring { inactive-color "transparent" }`: only the focused
    /// panel is ringed.
    pub const PANEL_BORDER_IDLE: Rgba = rgba_c(0x00000000);
    pub const HEADER_BG: Rgba = rgb_c(0x171717);
    pub const TEXT: Rgba = rgb_c(0xf4f4f4);
    pub const TEXT_DIM: Rgba = rgb_c(0x999999);
    pub const TEXT_USER: Rgba = rgb_c(0xffffff);
    pub const ACCENT: Rgba = rgb_c(0xffffff);
    pub const ACCENT_DIM: Rgba = rgba_c(0xffffff_1f);
    pub const USER_BG: Rgba = rgb_c(0x1b1b1b);
    pub const USER_BAR: Rgba = rgb_c(0xffffff);
    pub const TOOL_BG: Rgba = rgb_c(0x181818);
    pub const TOOL_TEXT: Rgba = rgb_c(0xcccccc);
    pub const REASONING: Rgba = rgb_c(0x888888);
    pub const CODE_BG: Rgba = rgb_c(0x080808);
    pub const CODE_TEXT: Rgba = rgb_c(0xe7e7e7);
    pub const INLINE_CODE_BG: Rgba = rgb_c(0x242424);
    pub const INPUT_BG: Rgba = rgb_c(0x0d0d0d);
    pub const INPUT_BORDER: Rgba = rgb_c(0x444444);
    pub const CURSOR: Rgba = rgb_c(0xffffff);
    pub const SELECTION: Rgba = rgba_c(0xffffff_33);
    pub const ERROR: Rgba = rgb_c(0xffffff);
    pub const OK: Rgba = rgb_c(0xcccccc);
    pub const WARN: Rgba = rgb_c(0x999999);
    pub const HEADING: Rgba = rgb_c(0xffffff);
    pub const LINK: Rgba = rgb_c(0xcccccc);

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
