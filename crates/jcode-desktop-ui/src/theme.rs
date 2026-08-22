//! Theme: the jcode TUI's hand-tuned palette adapted to a dark desktop
//! canvas. Colors mirror `jcode-tui-style`'s semantic roles (user blue,
//! ai green, purple accent, blue-gray user wash) so the desktop app reads
//! as the same product as the terminal UI.

use gpui::{Hsla, Rgba, rgb, rgba};

pub struct Theme;

#[allow(dead_code)]
impl Theme {
    pub const BG: Rgba = rgb_c(0x090909);
    pub const CANVAS_DOT: Rgba = rgba_c(0xffffff_0d);
    pub const PANEL_BG: Rgba = rgb_c(0x111111);
    /// TUI border role (100,100,110), dimmed for idle panel chrome.
    pub const PANEL_BORDER: Rgba = rgb_c(0x3a3a42);
    /// Focused panel ring uses the TUI accent (186,139,255).
    pub const PANEL_BORDER_FOCUS: Rgba = rgb_c(0xba8bff);
    /// niri `focus-ring { inactive-color "transparent" }`: only the focused
    /// panel is ringed.
    pub const PANEL_BORDER_IDLE: Rgba = rgba_c(0x00000000);
    pub const HEADER_BG: Rgba = rgb_c(0x171717);
    /// TUI `AiText` (220,220,215): the default reading color.
    pub const TEXT: Rgba = rgb_c(0xdcdcd7);
    /// TUI `Pending` (140,140,140).
    pub const TEXT_DIM: Rgba = rgb_c(0x8c8c8c);
    /// TUI `UserText` (245,245,255).
    pub const TEXT_USER: Rgba = rgb_c(0xf5f5ff);
    /// TUI `Accent` (186,139,255).
    pub const ACCENT: Rgba = rgb_c(0xba8bff);
    pub const ACCENT_DIM: Rgba = rgba_c(0xba8bff_2b);
    /// TUI user-message accent (138,180,248), for prompts and user chrome.
    pub const USER_ACCENT: Rgba = rgb_c(0x8ab4f8);
    /// TUI assistant accent (129,199,132).
    pub const AI_ACCENT: Rgba = rgb_c(0x81c784);
    /// TUI `UserBg` (35,40,50): the blue-gray user message wash.
    pub const USER_BG: Rgba = rgb_c(0x232832);
    pub const TOOL_BG: Rgba = rgb_c(0x181818);
    /// TUI `Tool` (120,120,120).
    pub const TOOL_TEXT: Rgba = rgb_c(0x787878);
    pub const REASONING: Rgba = rgb_c(0x888888);
    pub const REASONING_BG: Rgba = rgba_c(0xffffff_06);
    /// TUI `Dim` (80,80,80).
    pub const TEXT_FAINT: Rgba = rgb_c(0x6e6e6e);
    pub const TOOL_BORDER: Rgba = rgb_c(0x262626);
    pub const ERROR_BG: Rgba = rgba_c(0xff6464_14);
    pub const CODE_BG: Rgba = rgb_c(0x080808);
    pub const CODE_TEXT: Rgba = rgb_c(0xe7e7e7);
    pub const INLINE_CODE_BG: Rgba = rgb_c(0x242424);
    pub const CODE_BORDER: Rgba = rgb_c(0x232323);
    pub const CODE_HEADER_BG: Rgba = rgb_c(0x141414);
    pub const CODE_GUTTER: Rgba = rgb_c(0x4a4a4a);
    // Syntax tiers keyed to the TUI role hues: accent purple keywords,
    // ai-green strings, info-blue types, warning-amber numbers.
    pub const CODE_KEYWORD: Rgba = rgb_c(0xba8bff);
    pub const CODE_STRING: Rgba = rgb_c(0x81c784);
    pub const CODE_COMMENT: Rgba = rgb_c(0x6b6b6b);
    pub const CODE_NUMBER: Rgba = rgb_c(0xffc864);
    pub const CODE_TYPE: Rgba = rgb_c(0x8cb4ff);
    pub const CODE_PUNCT: Rgba = rgb_c(0x8f8f8f);
    pub const ACCENT_MUTED: Rgba = rgb_c(0xa98fd6);
    pub const QUOTE_BG: Rgba = rgba_c(0xffffff_07);
    pub const TABLE_STRIPE: Rgba = rgba_c(0xffffff_08);
    pub const INPUT_BG: Rgba = rgb_c(0x0d0d0d);
    pub const INPUT_BORDER: Rgba = rgb_c(0x444444);
    pub const CURSOR: Rgba = rgb_c(0xffffff);
    /// TUI `SelectionBg` (60,60,80).
    pub const SELECTION: Rgba = rgba_c(0x3c3c50_cc);
    /// TUI `Error` (255,100,100).
    pub const ERROR: Rgba = rgb_c(0xff6464);
    /// TUI `Success` (100,200,100).
    pub const OK: Rgba = rgb_c(0x64c864);
    /// TUI `Warning` (255,200,100).
    pub const WARN: Rgba = rgb_c(0xffc864);
    /// TUI `HeaderName` (190,210,235).
    pub const HEADING: Rgba = rgb_c(0xbed2eb);
    /// TUI `FileLink` (180,200,255).
    pub const LINK: Rgba = rgb_c(0xb4c8ff);

    // Minimap: monochrome washes so the map reads at a glance without
    // competing with the content.
    pub const MINIMAP_TRACK: Rgba = rgba_c(0xffffff_0f);
    pub const MINIMAP_TRACK_ACTIVE: Rgba = rgba_c(0xffffff_1c);
    pub const MINIMAP_VIEWPORT: Rgba = rgba_c(0xffffff_26);
    pub const MINIMAP_PANEL: Rgba = rgb_c(0x616161);
    pub const MINIMAP_PANEL_BUSY: Rgba = rgb_c(0xba8bff);
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
