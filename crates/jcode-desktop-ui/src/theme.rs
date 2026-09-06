//! Runtime-configurable semantic theme for Jcode Desktop.

mod palettes;

use gpui::{Hsla, Rgba, rgb, rgba};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
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
    /// Raised paper for the active pane, recessed backing for its neighbors.
    /// Use palette roles rather than dimming the entire pane so transcript text,
    /// code, images, and controls retain their original contrast and opacity.
    pub fn panel_background(&self, focused: bool) -> Rgba {
        if focused {
            self.PANEL_BG
        } else {
            self.HEADER_BG
        }
    }

    pub fn global() -> &'static Self {
        let target = ACTIVE_THEME.load(Ordering::Relaxed);
        let mut transition = transition_state().lock().unwrap();
        if let Some((from, started)) = *transition {
            let progress = started.elapsed().as_secs_f32() / 0.18;
            if progress < 1.0 {
                let step = (progress * 16.0).round().clamp(0.0, 16.0) as usize;
                return &transition_frames()[from][target][step];
            }
            *transition = None;
        }
        &themes()[target]
    }

    pub fn active_preset() -> ThemePreset {
        let _ = themes();
        ThemePreset::ALL[ACTIVE_THEME.load(Ordering::Relaxed)]
    }

    pub fn select(preset: ThemePreset) {
        let _ = themes();
        let from = ACTIVE_THEME.load(Ordering::Relaxed);
        ACTIVE_THEME.store(preset.index(), Ordering::Relaxed);
        *transition_state().lock().unwrap() = if crate::config::get().appearance.reduce_motion {
            None
        } else {
            Some((from, std::time::Instant::now()))
        };
    }

    pub fn is_transitioning() -> bool {
        transition_state().lock().unwrap().is_some()
    }

    fn configured(mut theme: Self) -> Self {
        let config = crate::config::get();
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
    }

    // Warm neutral: charcoal and stone surfaces, ivory type, restrained sandstone focus.
    fn defaults() -> Self {
        Self {
            BG: rgb_c(0x1c1a18),
            CANVAS_DOT: rgba_c(0xe4ddd30a),
            PANEL_BG: rgb_c(0x25221f),
            PANEL_BORDER: rgb_c(0x403a34),
            PANEL_BORDER_FOCUS: rgb_c(0x87796b),
            PANEL_BORDER_IDLE: rgba_c(0x00000000),
            HEADER_BG: rgb_c(0x302b27),
            TEXT: rgb_c(0xe4ddd3),
            TEXT_DIM: rgb_c(0xa79d91),
            TEXT_USER: rgb_c(0xeee7de),
            ACCENT: rgb_c(0xb6a08a),
            ACCENT_DIM: rgba_c(0xb6a08a20),
            USER_ACCENT: rgb_c(0xc2b09d),
            AI_ACCENT: rgb_c(0xa9b09b),
            USER_BG: rgb_c(0x302b27),
            TOOL_BG: rgb_c(0x292521),
            TOOL_TEXT: rgb_c(0xa79d91),
            REASONING: rgb_c(0xa79d91),
            REASONING_BG: rgba_c(0xe4ddd306),
            TEXT_FAINT: rgb_c(0x9b9084),
            TOOL_BORDER: rgb_c(0x3b3530),
            ERROR_BG: rgba_c(0xff646414),
            CODE_BG: rgb_c(0x1c1a18),
            CODE_TEXT: rgb_c(0xe4ddd3),
            INLINE_CODE_BG: rgb_c(0x36302b),
            CODE_BORDER: rgb_c(0x3b3530),
            CODE_HEADER_BG: rgb_c(0x292521),
            CODE_GUTTER: rgb_c(0x92877c),
            CODE_KEYWORD: rgb_c(0xc6b49f),
            CODE_STRING: rgb_c(0xb0b69e),
            CODE_COMMENT: rgb_c(0x9e9589),
            CODE_NUMBER: rgb_c(0xc4b38e),
            CODE_TYPE: rgb_c(0xb9adb9),
            CODE_PUNCT: rgb_c(0xb2a79b),
            ACCENT_MUTED: rgb_c(0xa99a89),
            QUOTE_BG: rgba_c(0xe4ddd307),
            TABLE_STRIPE: rgba_c(0xe4ddd306),
            INPUT_BG: rgb_c(0x211e1b),
            INPUT_BORDER: rgb_c(0x51483f),
            CURSOR: rgb_c(0xeee7de),
            SELECTION: rgba_c(0x75665780),
            ERROR: rgb_c(0xe18c85),
            OK: rgb_c(0xa6b68e),
            WARN: rgb_c(0xd0b17d),
            HEADING: rgb_c(0xe4ddd3),
            LINK: rgb_c(0xc6b5a2),
            MINIMAP_TRACK: rgba_c(0xe4ddd306),
            MINIMAP_TRACK_ACTIVE: rgba_c(0xe4ddd30d),
            MINIMAP_VIEWPORT: rgba_c(0xb6a08a66),
            MINIMAP_PANEL: rgb_c(0x554c43),
            MINIMAP_PANEL_BUSY: rgb_c(0x786a5d),
            MINIMAP_BG: rgba_c(0x302b27e6),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
    WarmNeutral,
    WarmStudio,
    NeutralDark,
    NeutralLight,
    Midnight,
    Ocean,
    Forest,
    Plum,
    RoseDawn,
    Parchment,
}

impl ThemePreset {
    pub const ALL: [Self; 10] = [
        Self::WarmNeutral,
        Self::WarmStudio,
        Self::NeutralDark,
        Self::NeutralLight,
        Self::Midnight,
        Self::Ocean,
        Self::Forest,
        Self::Plum,
        Self::RoseDawn,
        Self::Parchment,
    ];
    pub const fn id(self) -> &'static str {
        match self {
            Self::WarmNeutral => "warm-neutral",
            Self::WarmStudio => "warm-studio",
            Self::NeutralDark => "neutral-dark",
            Self::NeutralLight => "neutral-light",
            Self::Midnight => "midnight",
            Self::Ocean => "ocean",
            Self::Forest => "forest",
            Self::Plum => "plum",
            Self::RoseDawn => "rose-dawn",
            Self::Parchment => "parchment",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::WarmNeutral => "Warm neutral",
            Self::WarmStudio => "Warm studio",
            Self::NeutralDark => "Neutral dark",
            Self::NeutralLight => "Neutral light",
            Self::Midnight => "Midnight",
            Self::Ocean => "Ocean",
            Self::Forest => "Forest",
            Self::Plum => "Plum",
            Self::RoseDawn => "Rose dawn",
            Self::Parchment => "Parchment",
        }
    }
    const fn index(self) -> usize {
        match self {
            Self::WarmNeutral => 0,
            Self::WarmStudio => 1,
            Self::NeutralDark => 2,
            Self::NeutralLight => 3,
            Self::Midnight => 4,
            Self::Ocean => 5,
            Self::Forest => 6,
            Self::Plum => 7,
            Self::RoseDawn => 8,
            Self::Parchment => 9,
        }
    }
    pub fn from_id(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|preset| preset.id() == value)
            .unwrap_or(Self::WarmNeutral)
    }
    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }
}

static ACTIVE_THEME: AtomicUsize = AtomicUsize::new(0);

fn transition_state() -> &'static Mutex<Option<(usize, std::time::Instant)>> {
    static STATE: OnceLock<Mutex<Option<(usize, std::time::Instant)>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn transition_frames() -> &'static Vec<Vec<Vec<Theme>>> {
    static FRAMES: OnceLock<Vec<Vec<Vec<Theme>>>> = OnceLock::new();
    FRAMES.get_or_init(|| {
        themes()
            .iter()
            .map(|from| {
                themes()
                    .iter()
                    .map(|to| {
                        (0..=16)
                            .map(|step| interpolate(from, to, step as f32 / 16.0))
                            .collect()
                    })
                    .collect()
            })
            .collect()
    })
}

fn interpolate(from: &Theme, to: &Theme, amount: f32) -> Theme {
    if amount <= 0.0 {
        return from.clone();
    }
    if amount >= 1.0 {
        return to.clone();
    }
    let mut result = from.clone();
    let mix = |a: Rgba, b: Rgba| Rgba {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: a.a + (b.a - a.a) * amount,
    };
    macro_rules! colors { ($($field:ident),+ $(,)?) => { $(result.$field = mix(from.$field, to.$field);)+ }; }
    colors!(
        BG,
        CANVAS_DOT,
        PANEL_BG,
        PANEL_BORDER,
        PANEL_BORDER_FOCUS,
        PANEL_BORDER_IDLE,
        HEADER_BG,
        TEXT,
        TEXT_DIM,
        TEXT_USER,
        ACCENT,
        ACCENT_DIM,
        USER_ACCENT,
        AI_ACCENT,
        USER_BG,
        TOOL_BG,
        TOOL_TEXT,
        REASONING,
        REASONING_BG,
        TEXT_FAINT,
        TOOL_BORDER,
        ERROR_BG,
        CODE_BG,
        CODE_TEXT,
        INLINE_CODE_BG,
        CODE_BORDER,
        CODE_HEADER_BG,
        CODE_GUTTER,
        CODE_KEYWORD,
        CODE_STRING,
        CODE_COMMENT,
        CODE_NUMBER,
        CODE_TYPE,
        CODE_PUNCT,
        ACCENT_MUTED,
        QUOTE_BG,
        TABLE_STRIPE,
        INPUT_BG,
        INPUT_BORDER,
        CURSOR,
        SELECTION,
        ERROR,
        OK,
        WARN,
        HEADING,
        LINK,
        MINIMAP_TRACK,
        MINIMAP_TRACK_ACTIVE,
        MINIMAP_VIEWPORT,
        MINIMAP_PANEL,
        MINIMAP_PANEL_BUSY,
        MINIMAP_BG
    );
    result
}

fn themes() -> &'static [Theme; ThemePreset::ALL.len()] {
    static THEMES: OnceLock<[Theme; ThemePreset::ALL.len()]> = OnceLock::new();
    THEMES.get_or_init(|| {
        let warm = Theme::defaults();
        let mut studio = warm.clone();
        studio.BG = rgb_c(0x211914);
        studio.PANEL_BG = rgb_c(0x2d231d);
        studio.HEADER_BG = rgb_c(0x392b23);
        studio.ACCENT = rgb_c(0xd29a6a);
        studio.ACCENT_DIM = rgba_c(0xd29a6a24);
        studio.USER_ACCENT = rgb_c(0xe0ad7f);
        studio.PANEL_BORDER_FOCUS = rgb_c(0xa87550);
        studio.LINK = rgb_c(0xe0ad7f);
        studio.SELECTION = rgba_c(0xb66f4380);
        let mut dark = warm.clone();
        dark.BG = rgb_c(0x151719);
        dark.PANEL_BG = rgb_c(0x1d2023);
        dark.HEADER_BG = rgb_c(0x262a2e);
        dark.PANEL_BORDER = rgb_c(0x353a40);
        dark.PANEL_BORDER_FOCUS = rgb_c(0x76818c);
        dark.TEXT = rgb_c(0xe2e5e9);
        dark.TEXT_DIM = rgb_c(0x9ba3ac);
        dark.REASONING = dark.TEXT_DIM;
        dark.ACCENT = rgb_c(0x9aa8b6);
        dark.ACCENT_DIM = rgba_c(0x9aa8b620);
        dark.INPUT_BG = rgb_c(0x191c1f);
        dark.CODE_BG = rgb_c(0x151719);
        dark.USER_BG = rgb_c(0x262a2e);
        let mut light = warm.clone();
        light.BG = rgb_c(0xebe9e5);
        light.CANVAS_DOT = rgba_c(0x27252212);
        light.PANEL_BG = rgb_c(0xf8f7f4);
        light.PANEL_BORDER = rgb_c(0xd2cec7);
        light.PANEL_BORDER_FOCUS = rgb_c(0x7b746a);
        light.HEADER_BG = rgb_c(0xe2dfda);
        light.TEXT = rgb_c(0x292724);
        light.TEXT_USER = rgb_c(0x201e1b);
        light.TEXT_DIM = rgb_c(0x67625b);
        light.REASONING = light.TEXT_DIM;
        light.TEXT_FAINT = rgb_c(0x777169);
        light.ACCENT = rgb_c(0x665f57);
        light.ACCENT_DIM = rgba_c(0x665f5718);
        light.USER_ACCENT = rgb_c(0x725d4c);
        light.AI_ACCENT = rgb_c(0x53675b);
        light.USER_BG = rgb_c(0xe8e4de);
        light.TOOL_BG = rgb_c(0xefede8);
        light.TOOL_BORDER = rgb_c(0xd2cec7);
        light.TOOL_TEXT = rgb_c(0x67625b);
        light.CODE_BG = rgb_c(0xf0efec);
        light.CODE_TEXT = rgb_c(0x292724);
        light.INLINE_CODE_BG = rgb_c(0xe4e1dc);
        light.CODE_BORDER = rgb_c(0xd2cec7);
        light.CODE_HEADER_BG = rgb_c(0xe8e5df);
        light.CODE_GUTTER = rgb_c(0x746e66);
        light.CODE_KEYWORD = rgb_c(0x6f3f62);
        light.CODE_STRING = rgb_c(0x3f6848);
        light.CODE_COMMENT = rgb_c(0x68635c);
        light.CODE_NUMBER = rgb_c(0x7a542b);
        light.CODE_TYPE = rgb_c(0x4d587b);
        light.CODE_PUNCT = rgb_c(0x4e4a45);
        // Status colors are also small foreground text in tool token badges.
        // The inherited dark-theme pastels wash out against this light panel.
        light.OK = rgb_c(0x3f6848);
        light.WARN = rgb_c(0x7a542b);
        light.ERROR = rgb_c(0xa33b3b);
        light.INPUT_BG = rgb_c(0xffffff);
        light.INPUT_BORDER = rgb_c(0xbdb7ae);
        light.CURSOR = rgb_c(0x292724);
        light.HEADING = rgb_c(0x292724);
        light.LINK = rgb_c(0x655346);
        light.MINIMAP_BG = rgba_c(0xf8f7f4e6);
        light.MINIMAP_PANEL = rgb_c(0xaaa39a);
        let configured = Theme::configured;
        let result = [
            configured(warm),
            configured(studio),
            configured(dark),
            configured(light),
            configured(palettes::MIDNIGHT.theme()),
            configured(palettes::OCEAN.theme()),
            configured(palettes::FOREST.theme()),
            configured(palettes::PLUM.theme()),
            configured(palettes::ROSE_DAWN.theme()),
            configured(palettes::PARCHMENT.theme()),
        ];
        ACTIVE_THEME.store(
            ThemePreset::from_id(&crate::config::get().appearance.theme).index(),
            Ordering::Relaxed,
        );
        result
    })
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

    fn luminance(color: Rgba) -> f32 {
        let channel = |value: f32| {
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (bright, dark) = if luminance(a) > luminance(b) {
            (luminance(a), luminance(b))
        } else {
            (luminance(b), luminance(a))
        };
        (bright + 0.05) / (dark + 0.05)
    }

    #[test]
    fn preset_ids_indices_and_cycle_cover_every_palette() {
        let mut visited = std::collections::HashSet::new();
        let mut current = ThemePreset::ALL[0];
        for (index, preset) in ThemePreset::ALL.into_iter().enumerate() {
            assert_eq!(preset.index(), index);
            assert_eq!(ThemePreset::from_id(preset.id()), preset);
            assert_eq!(current, preset);
            assert!(visited.insert(preset.id()));
            current = current.next();
        }
        assert_eq!(current, ThemePreset::WarmNeutral);
        assert_eq!(ThemePreset::from_id("unknown"), ThemePreset::WarmNeutral);
    }

    #[test]
    fn new_palettes_keep_syntax_and_status_colors_legible() {
        for preset in &ThemePreset::ALL[4..] {
            let theme = &themes()[preset.index()];
            for (background, foregrounds) in [
                (
                    theme.CODE_BG,
                    vec![
                        theme.CODE_KEYWORD,
                        theme.CODE_STRING,
                        theme.CODE_COMMENT,
                        theme.CODE_NUMBER,
                        theme.CODE_TYPE,
                        theme.CODE_PUNCT,
                    ],
                ),
                (
                    theme.PANEL_BG,
                    vec![
                        theme.TEXT_FAINT,
                        theme.LINK,
                        theme.ACCENT,
                        theme.AI_ACCENT,
                        theme.ERROR,
                        theme.OK,
                        theme.WARN,
                    ],
                ),
                (theme.TOOL_BG, vec![theme.TOOL_TEXT]),
                (theme.HEADER_BG, vec![theme.TEXT_DIM]),
                (theme.INLINE_CODE_BG, vec![theme.CODE_TEXT]),
            ] {
                for foreground in foregrounds {
                    assert!(
                        contrast(foreground, background) >= 4.5,
                        "{} contrast was {} for {foreground:?}",
                        preset.id(),
                        contrast(foreground, background)
                    );
                }
            }
        }
    }

    #[test]
    fn every_preset_keeps_semantic_text_legible() {
        let themes = themes();
        for (preset, theme) in ThemePreset::ALL.into_iter().zip(themes) {
            for (foreground, background, role) in [
                (theme.TEXT, theme.PANEL_BG, "panel text"),
                (theme.TEXT_DIM, theme.PANEL_BG, "secondary text"),
                (theme.REASONING, theme.PANEL_BG, "thinking text"),
                (theme.OK, theme.PANEL_BG, "normal token badge"),
                (theme.WARN, theme.PANEL_BG, "warning token badge"),
                (theme.ERROR, theme.PANEL_BG, "large token badge"),
                (theme.CODE_TEXT, theme.CODE_BG, "code text"),
                (theme.TEXT_USER, theme.USER_BG, "user text"),
            ] {
                assert!(
                    contrast(foreground, background) >= 4.5,
                    "{} {role} contrast was {}",
                    preset.id(),
                    contrast(foreground, background)
                );
            }
        }
    }

    #[test]
    fn every_preset_distinguishes_pane_focus_without_dimming_text() {
        for (preset, theme) in ThemePreset::ALL.into_iter().zip(themes()) {
            let active = theme.panel_background(true);
            let inactive = theme.panel_background(false);
            assert_eq!(active, theme.PANEL_BG);
            assert_eq!(inactive.a, 1.0);
            let difference = (active.r - inactive.r)
                .abs()
                .max((active.g - inactive.g).abs())
                .max((active.b - inactive.b).abs());
            assert!(
                difference >= 10.0 / 255.0,
                "{} lacks pane contrast",
                preset.id()
            );
            for foreground in [theme.TEXT, theme.TEXT_DIM, theme.REASONING] {
                assert!(
                    contrast(foreground, inactive) >= 4.5,
                    "{} inactive text contrast was {}",
                    preset.id(),
                    contrast(foreground, inactive)
                );
            }
        }
    }

    #[test]
    fn transition_frames_begin_and_end_at_the_selected_palettes() {
        let frames = transition_frames();
        for from in 0..ThemePreset::ALL.len() {
            for to in 0..ThemePreset::ALL.len() {
                assert_eq!(frames[from][to][0].BG, themes()[from].BG);
                assert_eq!(frames[from][to][16].BG, themes()[to].BG);
                assert_eq!(frames[from][to].len(), 17);
            }
        }
    }
}
