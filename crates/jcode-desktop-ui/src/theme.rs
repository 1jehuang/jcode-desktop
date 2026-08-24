//! Runtime-configurable semantic theme for Jcode Desktop.

use gpui::{Hsla, Rgba, rgb, rgba};
use std::sync::OnceLock;

#[allow(non_snake_case)]
pub struct Theme {
    pub BG: Rgba,
    pub CANVAS_DOT: Rgba,
    pub PANEL_BG: Rgba,
    pub PANEL_BORDER: Rgba,
    pub PANEL_BORDER_FOCUS: Rgba,
    pub PANEL_BORDER_IDLE: Rgba,
    pub HEADER_BG: Rgba,
    pub TEXT: Rgba,
    pub TEXT_DIM: Rgba,
    pub TEXT_USER: Rgba,
    pub ACCENT: Rgba,
    pub ACCENT_DIM: Rgba,
    pub USER_ACCENT: Rgba,
    pub AI_ACCENT: Rgba,
    pub USER_BG: Rgba,
    pub TOOL_BG: Rgba,
    pub TOOL_TEXT: Rgba,
    pub REASONING: Rgba,
    pub REASONING_BG: Rgba,
    pub TEXT_FAINT: Rgba,
    pub TOOL_BORDER: Rgba,
    pub ERROR_BG: Rgba,
    pub CODE_BG: Rgba,
    pub CODE_TEXT: Rgba,
    pub INLINE_CODE_BG: Rgba,
    pub CODE_BORDER: Rgba,
    pub CODE_HEADER_BG: Rgba,
    pub CODE_GUTTER: Rgba,
    pub CODE_KEYWORD: Rgba,
    pub CODE_STRING: Rgba,
    pub CODE_COMMENT: Rgba,
    pub CODE_NUMBER: Rgba,
    pub CODE_TYPE: Rgba,
    pub CODE_PUNCT: Rgba,
    pub ACCENT_MUTED: Rgba,
    pub QUOTE_BG: Rgba,
    pub TABLE_STRIPE: Rgba,
    pub INPUT_BG: Rgba,
    pub INPUT_BORDER: Rgba,
    pub CURSOR: Rgba,
    pub SELECTION: Rgba,
    pub ERROR: Rgba,
    pub OK: Rgba,
    pub WARN: Rgba,
    pub HEADING: Rgba,
    pub LINK: Rgba,
    pub MINIMAP_TRACK: Rgba,
    pub MINIMAP_TRACK_ACTIVE: Rgba,
    pub MINIMAP_VIEWPORT: Rgba,
    pub MINIMAP_PANEL: Rgba,
    pub MINIMAP_PANEL_BUSY: Rgba,
    pub MINIMAP_BG: Rgba,
    pub FONT_UI: &'static str,
    pub FONT_MONO: &'static str,
}

impl Theme {
    pub fn global() -> &'static Self {
        static THEME: OnceLock<Theme> = OnceLock::new();
        THEME.get_or_init(|| {
            let config = crate::config::get();
            let mut theme = Self::defaults();
            if let Some(font) = config.appearance.ui_font.as_deref() {
                theme.FONT_UI = Box::leak(font.to_owned().into_boxed_str());
            }
            if let Some(font) = config.appearance.mono_font.as_deref() {
                theme.FONT_MONO = Box::leak(font.to_owned().into_boxed_str());
            }
            for (role, value) in &config.appearance.colors {
                match parse_color(value) {
                    Some(color) => theme.set_color(role, color),
                    None => eprintln!("ignoring invalid desktop color {role}={value:?}"),
                }
            }
            theme
        })
    }

    fn defaults() -> Self {
        Self {
            BG: rgb_c(0x090909),
            CANVAS_DOT: rgba_c(0xffffff0d),
            PANEL_BG: rgb_c(0x111111),
            PANEL_BORDER: rgb_c(0x3a3a42),
            PANEL_BORDER_FOCUS: rgb_c(0xba8bff),
            PANEL_BORDER_IDLE: rgba_c(0x00000000),
            HEADER_BG: rgb_c(0x171717),
            TEXT: rgb_c(0xdcdcd7),
            TEXT_DIM: rgb_c(0x8c8c8c),
            TEXT_USER: rgb_c(0xf5f5ff),
            ACCENT: rgb_c(0xba8bff),
            ACCENT_DIM: rgba_c(0xba8bff2b),
            USER_ACCENT: rgb_c(0x8ab4f8),
            AI_ACCENT: rgb_c(0x81c784),
            USER_BG: rgb_c(0x232832),
            TOOL_BG: rgb_c(0x181818),
            TOOL_TEXT: rgb_c(0x787878),
            REASONING: rgb_c(0x888888),
            REASONING_BG: rgba_c(0xffffff06),
            TEXT_FAINT: rgb_c(0x6e6e6e),
            TOOL_BORDER: rgb_c(0x262626),
            ERROR_BG: rgba_c(0xff646414),
            CODE_BG: rgb_c(0x080808),
            CODE_TEXT: rgb_c(0xe7e7e7),
            INLINE_CODE_BG: rgb_c(0x242424),
            CODE_BORDER: rgb_c(0x232323),
            CODE_HEADER_BG: rgb_c(0x141414),
            CODE_GUTTER: rgb_c(0x4a4a4a),
            CODE_KEYWORD: rgb_c(0xba8bff),
            CODE_STRING: rgb_c(0x81c784),
            CODE_COMMENT: rgb_c(0x6b6b6b),
            CODE_NUMBER: rgb_c(0xffc864),
            CODE_TYPE: rgb_c(0x8cb4ff),
            CODE_PUNCT: rgb_c(0x8f8f8f),
            ACCENT_MUTED: rgb_c(0xa98fd6),
            QUOTE_BG: rgba_c(0xffffff07),
            TABLE_STRIPE: rgba_c(0xffffff08),
            INPUT_BG: rgb_c(0x0d0d0d),
            INPUT_BORDER: rgb_c(0x444444),
            CURSOR: rgb_c(0xffffff),
            SELECTION: rgba_c(0x3c3c50cc),
            ERROR: rgb_c(0xff6464),
            OK: rgb_c(0x64c864),
            WARN: rgb_c(0xffc864),
            HEADING: rgb_c(0xbed2eb),
            LINK: rgb_c(0xb4c8ff),
            MINIMAP_TRACK: rgba_c(0xffffff0f),
            MINIMAP_TRACK_ACTIVE: rgba_c(0xffffff1c),
            MINIMAP_VIEWPORT: rgba_c(0xffffff26),
            MINIMAP_PANEL: rgb_c(0x616161),
            MINIMAP_PANEL_BUSY: rgb_c(0xba8bff),
            MINIMAP_BG: rgba_c(0x171717e6),
            FONT_UI: platform_font(),
            FONT_MONO: platform_font(),
        }
    }

    fn set_color(&mut self, role: &str, color: Rgba) {
        match role.to_ascii_lowercase().replace('-', "_").as_str() {
            "bg" => self.BG = color,
            "canvas_dot" => self.CANVAS_DOT = color,
            "panel_bg" => self.PANEL_BG = color,
            "panel_border" => self.PANEL_BORDER = color,
            "panel_border_focus" => self.PANEL_BORDER_FOCUS = color,
            "panel_border_idle" => self.PANEL_BORDER_IDLE = color,
            "header_bg" => self.HEADER_BG = color,
            "text" => self.TEXT = color,
            "text_dim" => self.TEXT_DIM = color,
            "text_user" => self.TEXT_USER = color,
            "accent" => self.ACCENT = color,
            "accent_dim" => self.ACCENT_DIM = color,
            "user_accent" => self.USER_ACCENT = color,
            "ai_accent" => self.AI_ACCENT = color,
            "user_bg" => self.USER_BG = color,
            "tool_bg" => self.TOOL_BG = color,
            "tool_text" => self.TOOL_TEXT = color,
            "reasoning" => self.REASONING = color,
            "reasoning_bg" => self.REASONING_BG = color,
            "text_faint" => self.TEXT_FAINT = color,
            "tool_border" => self.TOOL_BORDER = color,
            "error_bg" => self.ERROR_BG = color,
            "code_bg" => self.CODE_BG = color,
            "code_text" => self.CODE_TEXT = color,
            "inline_code_bg" => self.INLINE_CODE_BG = color,
            "code_border" => self.CODE_BORDER = color,
            "code_header_bg" => self.CODE_HEADER_BG = color,
            "code_gutter" => self.CODE_GUTTER = color,
            "code_keyword" => self.CODE_KEYWORD = color,
            "code_string" => self.CODE_STRING = color,
            "code_comment" => self.CODE_COMMENT = color,
            "code_number" => self.CODE_NUMBER = color,
            "code_type" => self.CODE_TYPE = color,
            "code_punct" => self.CODE_PUNCT = color,
            "accent_muted" => self.ACCENT_MUTED = color,
            "quote_bg" => self.QUOTE_BG = color,
            "table_stripe" => self.TABLE_STRIPE = color,
            "input_bg" => self.INPUT_BG = color,
            "input_border" => self.INPUT_BORDER = color,
            "cursor" => self.CURSOR = color,
            "selection" => self.SELECTION = color,
            "error" => self.ERROR = color,
            "ok" => self.OK = color,
            "warn" => self.WARN = color,
            "heading" => self.HEADING = color,
            "link" => self.LINK = color,
            "minimap_track" => self.MINIMAP_TRACK = color,
            "minimap_track_active" => self.MINIMAP_TRACK_ACTIVE = color,
            "minimap_viewport" => self.MINIMAP_VIEWPORT = color,
            "minimap_panel" => self.MINIMAP_PANEL = color,
            "minimap_panel_busy" => self.MINIMAP_PANEL_BUSY = color,
            "minimap_bg" => self.MINIMAP_BG = color,
            _ => eprintln!("ignoring unknown desktop color role {role:?}"),
        }
    }
}

#[cfg(target_os = "macos")]
const fn platform_font() -> &'static str {
    "Menlo"
}
#[cfg(not(target_os = "macos"))]
const fn platform_font() -> &'static str {
    "JetBrainsMono Nerd Font"
}

fn parse_color(value: &str) -> Option<Rgba> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    let raw = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some(rgb_c(raw)),
        8 => Some(rgba_c(raw)),
        _ => None,
    }
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

#[allow(dead_code)]
pub fn _unused() {
    let _ = rgb(0);
    let _ = rgba(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colors_accept_rgb_and_rgba_hex() {
        assert_eq!(parse_color("#ff0080").unwrap().a, 1.0);
        assert!((parse_color("10203040").unwrap().a - 64.0 / 255.0).abs() < 0.001);
        assert!(parse_color("purple").is_none());
    }
}
