//! Windows caption: the app draws its own title bar, Windows 11 style.
//!
//! Like Windows Terminal and Edge, the workspace tabs share the caption row
//! instead of sitting under an opaque system title bar. The window still
//! behaves natively because every region reports a standard hit-test code:
//! empty header space is `HTCAPTION` (drag, double-click maximize, Aero
//! Shake, the system menu) and the three buttons are `HTMINBUTTON`,
//! `HTMAXBUTTON` (which opens Snap Layouts on hover) and `HTCLOSE`. Windows
//! performs the actions itself, so the buttons carry no click listeners.
//!
//! Other platforms return their content unchanged.

use gpui::{Rgba, Window, WindowControlArea, div, prelude::*, px};

use crate::theme::Theme;

/// Windows 11 caption buttons are 46 effective pixels wide.
pub(crate) const BUTTON_WIDTH: f32 = 46.0;
/// Matches the workspace tab row (strip padding plus tab height), so the
/// buttons and tabs share one baseline.
pub(crate) const HEIGHT: f32 = 34.0;
/// Width the header must leave free on the right for the three buttons.
pub(crate) const BUTTONS_WIDTH: f32 = BUTTON_WIDTH * 3.0;

/// The app draws the caption on Windows only.
pub(crate) const fn enabled() -> bool {
    cfg!(target_os = "windows")
}

/// Close-button hover red used by Windows 11 (`#C42B1C`).
const CLOSE_HOVER: Rgba = Rgba {
    r: 0xC4 as f32 / 255.0,
    g: 0x2B as f32 / 255.0,
    b: 0x1C as f32 / 255.0,
    a: 1.0,
};

/// Glyph font for caption buttons. Windows 11 ships Segoe Fluent Icons and
/// Windows 10 ships Segoe MDL2 Assets. Both use the same code points.
fn glyph_font(window: &Window) -> &'static str {
    static FONT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        let names = window.text_system().all_font_names();
        if names.iter().any(|name| name == "Segoe Fluent Icons") {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        }
    })
}

/// Places the caption around a window's content: a drag region behind
/// everything along the top edge, and the buttons above everything at the
/// top-right corner. Interactive header controls occlude the drag region or
/// stop mouse-down propagation, so clicks on them never start a window move.
///
/// `inset` pushes content below the caption row, for screens without a
/// header row of their own to share it with.
pub(crate) fn wrap(content: gpui::Div, window: &Window, inset: bool) -> gpui::Div {
    if !enabled() || window.is_fullscreen() {
        return content;
    }
    div()
        .debug_selector(|| "window-caption-root".into())
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .when(inset, |el| el.pt(px(HEIGHT)))
        .child(
            div()
                .id("window-caption-drag")
                .debug_selector(|| "window-caption-drag".into())
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(HEIGHT))
                .window_control_area(WindowControlArea::Drag),
        )
        .child(content.flex_1().min_h_0())
        .child(buttons(window))
}

fn buttons(window: &Window) -> impl IntoElement {
    let theme = Theme::global();
    let active = window.is_window_active();
    let glyph = if active { theme.TEXT } else { theme.TEXT_DIM };
    let font = glyph_font(window);
    let button = |id: &'static str, area: WindowControlArea, symbol: &'static str| {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .w(px(BUTTON_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .font_family(font)
            .text_size(px(10.0))
            .text_color(glyph)
            .occlude()
            .window_control_area(area)
            .child(symbol)
    };
    let neutral_hover = theme.TEXT.opacity(0.08);
    let neutral_press = theme.TEXT.opacity(0.05);
    div()
        .debug_selector(|| "window-caption-buttons".into())
        .absolute()
        .top_0()
        .right_0()
        .h(px(HEIGHT))
        .flex()
        .child(
            button(
                "window-caption-minimize",
                WindowControlArea::Min,
                "\u{E921}",
            )
            .hover(move |el| el.bg(neutral_hover))
            .active(move |el| el.bg(neutral_press)),
        )
        .child(
            button(
                "window-caption-maximize",
                WindowControlArea::Max,
                if window.is_maximized() {
                    "\u{E923}"
                } else {
                    "\u{E922}"
                },
            )
            .hover(move |el| el.bg(neutral_hover))
            .active(move |el| el.bg(neutral_press)),
        )
        .child(
            button("window-caption-close", WindowControlArea::Close, "\u{E8BB}")
                .hover(|el| el.bg(CLOSE_HOVER).text_color(gpui::white()))
                .active(|el| el.bg(CLOSE_HOVER.opacity(0.9)).text_color(gpui::white())),
        )
}
