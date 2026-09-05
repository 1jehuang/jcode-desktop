//! Coordinated palettes. Every semantic role is derived here so new themes do
//! not accidentally inherit warm-neutral surfaces or unreadable light accents.
use super::{Theme, platform_font, rgb_c, rgba_c};

pub(super) struct Palette {
    bg: u32,
    panel: u32,
    raised: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    secondary: u32,
    keyword: u32,
    string: u32,
    number: u32,
    error: u32,
    ok: u32,
    warn: u32,
}

impl Palette {
    pub(super) fn theme(&self) -> Theme {
        let alpha = |color: u32, opacity: u32| rgba_c((color << 8) | opacity);
        Theme {
            BG: rgb_c(self.bg),
            CANVAS_DOT: alpha(self.text, 0x0a),
            PANEL_BG: rgb_c(self.panel),
            PANEL_BORDER: rgb_c(self.border),
            PANEL_BORDER_FOCUS: rgb_c(self.accent),
            PANEL_BORDER_IDLE: rgba_c(0),
            HEADER_BG: rgb_c(self.raised),
            TEXT: rgb_c(self.text),
            TEXT_DIM: rgb_c(self.muted),
            TEXT_USER: rgb_c(self.text),
            ACCENT: rgb_c(self.accent),
            ACCENT_DIM: alpha(self.accent, 0x20),
            USER_ACCENT: rgb_c(self.accent),
            AI_ACCENT: rgb_c(self.secondary),
            USER_BG: rgb_c(self.raised),
            TOOL_BG: rgb_c(self.raised),
            TOOL_TEXT: rgb_c(self.muted),
            REASONING: rgb_c(self.muted),
            REASONING_BG: alpha(self.text, 0x06),
            TEXT_FAINT: rgb_c(self.muted),
            TOOL_BORDER: rgb_c(self.border),
            ERROR_BG: alpha(self.error, 0x14),
            CODE_BG: rgb_c(self.bg),
            CODE_TEXT: rgb_c(self.text),
            INLINE_CODE_BG: rgb_c(self.raised),
            CODE_BORDER: rgb_c(self.border),
            CODE_HEADER_BG: rgb_c(self.raised),
            CODE_GUTTER: rgb_c(self.muted),
            CODE_KEYWORD: rgb_c(self.keyword),
            CODE_STRING: rgb_c(self.string),
            CODE_COMMENT: rgb_c(self.muted),
            CODE_NUMBER: rgb_c(self.number),
            CODE_TYPE: rgb_c(self.secondary),
            CODE_PUNCT: rgb_c(self.muted),
            ACCENT_MUTED: rgb_c(self.accent),
            QUOTE_BG: alpha(self.text, 0x07),
            TABLE_STRIPE: alpha(self.text, 0x06),
            INPUT_BG: rgb_c(self.bg),
            INPUT_BORDER: rgb_c(self.border),
            CURSOR: rgb_c(self.accent),
            SELECTION: alpha(self.accent, 0x40),
            ERROR: rgb_c(self.error),
            OK: rgb_c(self.ok),
            WARN: rgb_c(self.warn),
            HEADING: rgb_c(self.text),
            LINK: rgb_c(self.accent),
            MINIMAP_TRACK: alpha(self.text, 0x06),
            MINIMAP_TRACK_ACTIVE: alpha(self.text, 0x0d),
            MINIMAP_VIEWPORT: alpha(self.accent, 0x66),
            MINIMAP_PANEL: rgb_c(self.border),
            MINIMAP_PANEL_BUSY: rgb_c(self.accent),
            MINIMAP_BG: alpha(self.panel, 0xe6),
            FONT_UI: platform_font(),
            FONT_MONO: platform_font(),
        }
    }
}

// Ink-blue surfaces with periwinkle focus and cool syntax colors.
pub(super) const MIDNIGHT: Palette = Palette {
    bg: 0x111522,
    panel: 0x1a2032,
    raised: 0x252d43,
    border: 0x3b4764,
    text: 0xe1e7f5,
    muted: 0xa6b2cc,
    accent: 0xa5b4fc,
    secondary: 0x8ed7df,
    keyword: 0xc4b5fd,
    string: 0xa7d9ac,
    number: 0xf0c592,
    error: 0xf2a0ab,
    ok: 0xa7d9ac,
    warn: 0xf0c592,
};

// Deep teal rather than another blue-gray dark theme.
pub(super) const OCEAN: Palette = Palette {
    bg: 0x102126,
    panel: 0x172d34,
    raised: 0x203b43,
    border: 0x375660,
    text: 0xdceef0,
    muted: 0xa4c1c7,
    accent: 0x79d5cc,
    secondary: 0x9cc8f2,
    keyword: 0xadc4ff,
    string: 0xadd8a1,
    number: 0xeac68e,
    error: 0xf0a3a0,
    ok: 0xadd8a1,
    warn: 0xeac68e,
};

// Evergreen surfaces, sage focus, and warm gold literals.
pub(super) const FOREST: Palette = Palette {
    bg: 0x17201a,
    panel: 0x202e25,
    raised: 0x2c3b30,
    border: 0x445848,
    text: 0xe1eadd,
    muted: 0xb0c0a9,
    accent: 0xa8ce91,
    secondary: 0x8ecdc3,
    keyword: 0xd0b6df,
    string: 0xb9d89b,
    number: 0xe5c18c,
    error: 0xefa69c,
    ok: 0xb9d89b,
    warn: 0xe5c18c,
};

// Aubergine and mauve with a restrained rose accent.
pub(super) const PLUM: Palette = Palette {
    bg: 0x211924,
    panel: 0x2d2232,
    raised: 0x3b2e41,
    border: 0x57435f,
    text: 0xeee2f0,
    muted: 0xc4acc9,
    accent: 0xdfafd5,
    secondary: 0xb6b8ee,
    keyword: 0xd3b5f5,
    string: 0xb5d5aa,
    number: 0xeec396,
    error: 0xf1a4af,
    ok: 0xb5d5aa,
    warn: 0xeec396,
};

// Blush paper with dark berry accents, including accessible status colors.
pub(super) const ROSE_DAWN: Palette = Palette {
    bg: 0xf0e4e4,
    panel: 0xfff7f5,
    raised: 0xf3e8e7,
    border: 0xcfb8bd,
    text: 0x422f39,
    muted: 0x755d68,
    accent: 0x934365,
    secondary: 0x486570,
    keyword: 0x804b88,
    string: 0x486c4c,
    number: 0x885a30,
    error: 0xad354a,
    ok: 0x426c4b,
    warn: 0x865b18,
};

// Warm cream paper, sepia ink, and olive details for daytime reading.
pub(super) const PARCHMENT: Palette = Palette {
    bg: 0xece2cc,
    panel: 0xfaf3e3,
    raised: 0xefe5cf,
    border: 0xc6b797,
    text: 0x3e3528,
    muted: 0x71634c,
    accent: 0x85602d,
    secondary: 0x49655f,
    keyword: 0x80506a,
    string: 0x536b37,
    number: 0x925326,
    error: 0xa63c32,
    ok: 0x536b37,
    warn: 0x805d16,
};
