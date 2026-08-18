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
    pub const TOOL_BG: Rgba = rgb_c(0x181818);
    pub const TOOL_TEXT: Rgba = rgb_c(0xcccccc);
    pub const REASONING: Rgba = rgb_c(0x888888);
    pub const REASONING_BG: Rgba = rgba_c(0xffffff_06);
    pub const TEXT_FAINT: Rgba = rgb_c(0x6e6e6e);
    pub const TOOL_BORDER: Rgba = rgb_c(0x262626);
    pub const ERROR_BG: Rgba = rgba_c(0xffffff_0a);
    pub const CODE_BG: Rgba = rgb_c(0x080808);
    pub const CODE_TEXT: Rgba = rgb_c(0xe7e7e7);
    pub const INLINE_CODE_BG: Rgba = rgb_c(0x242424);
    pub const CODE_BORDER: Rgba = rgb_c(0x232323);
    pub const CODE_HEADER_BG: Rgba = rgb_c(0x141414);
    pub const CODE_GUTTER: Rgba = rgb_c(0x4a4a4a);
    // Monochrome syntax tiers: weight by luminance, not hue, so code stays
    // legible inside the site's ink/muted/faint hierarchy.
    pub const CODE_KEYWORD: Rgba = rgb_c(0xffffff);
    pub const CODE_STRING: Rgba = rgb_c(0xb9b9b9);
    pub const CODE_COMMENT: Rgba = rgb_c(0x6b6b6b);
    pub const CODE_NUMBER: Rgba = rgb_c(0xd6d6d6);
    pub const CODE_TYPE: Rgba = rgb_c(0xededed);
    pub const CODE_PUNCT: Rgba = rgb_c(0x8f8f8f);
    pub const ACCENT_MUTED: Rgba = rgb_c(0xbdbdbd);
    pub const QUOTE_BG: Rgba = rgba_c(0xffffff_07);
    pub const TABLE_STRIPE: Rgba = rgba_c(0xffffff_08);
    pub const INPUT_BG: Rgba = rgb_c(0x0d0d0d);
    pub const INPUT_BORDER: Rgba = rgb_c(0x444444);
    pub const CURSOR: Rgba = rgb_c(0xffffff);
    pub const SELECTION: Rgba = rgba_c(0xffffff_33);
    pub const ERROR: Rgba = rgb_c(0xffffff);
    pub const OK: Rgba = rgb_c(0xcccccc);
    pub const WARN: Rgba = rgb_c(0x999999);
    pub const HEADING: Rgba = rgb_c(0xffffff);
    pub const LINK: Rgba = rgb_c(0xcccccc);

    // Minimap: monochrome washes so the map reads at a glance without
    // competing with the content.
    pub const MINIMAP_TRACK: Rgba = rgba_c(0xffffff_0f);
    pub const MINIMAP_TRACK_ACTIVE: Rgba = rgba_c(0xffffff_1c);
    pub const MINIMAP_VIEWPORT: Rgba = rgba_c(0xffffff_26);
    pub const MINIMAP_PANEL: Rgba = rgb_c(0x616161);
    pub const MINIMAP_PANEL_BUSY: Rgba = rgb_c(0xaaaaaa);
    pub const MINIMAP_BG: Rgba = rgba_c(0x171717_e6);

    // macOS text rendering in this GPUI revision is gated behind the `font-kit`
    // feature of `gpui_platform` (it forwards to `gpui_macos/font-kit`). The
    // Cargo.toml files enable it for macOS, not Linux. Without it GPUI logs
    // "gpui_macos was compiled without the `font-kit` feature, so no text will
    // be rendered" and every glyph run stays empty while SVG icons still paint.
    // Separately, GPUI does not silently substitute a missing family on macOS,
    // so we also avoid JetBrainsMono Nerd Font (absent on a stock Mac) and use
    // Menlo, which ships with every supported macOS version and preserves the
    // application's monospaced visual language.
    #[cfg(target_os = "macos")]
    pub const FONT_UI: &'static str = "Menlo";
    #[cfg(target_os = "macos")]
    pub const FONT_MONO: &'static str = "Menlo";
    #[cfg(not(target_os = "macos"))]
    pub const FONT_UI: &'static str = "JetBrainsMono Nerd Font";
    #[cfg(not(target_os = "macos"))]
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
