//! Small overlay scrollbars for GPUI's scrollable divs.

use gpui::{AnyElement, ScrollHandle, div, prelude::*, px};

use crate::theme::Theme;

const WIDTH: f32 = 4.0;
const INSET: f32 = 4.0;
const MIN_THUMB_HEIGHT: f32 = 28.0;

/// Paint a thin, rounded vertical thumb for `handle` when its content overflows.
///
/// The scrollable element remains responsible for wheel and touchpad input. This
/// is deliberately an overlay, so adding it never changes transcript wrapping.
pub fn vertical(handle: &ScrollHandle, selector: &'static str) -> AnyElement {
    let viewport_height = f32::from(handle.bounds().size.height);
    let max_offset = f32::from(handle.max_offset().y).max(0.0);
    if viewport_height <= 0.0 || max_offset <= 0.5 {
        return div().into_any_element();
    }

    let track_height = (viewport_height - INSET * 2.0).max(0.0);
    let content_height = viewport_height + max_offset;
    let thumb_height = (track_height * viewport_height / content_height)
        .clamp(MIN_THUMB_HEIGHT.min(track_height), track_height);
    let progress = (-f32::from(handle.offset().y) / max_offset).clamp(0.0, 1.0);
    let thumb_top = INSET + (track_height - thumb_height) * progress;

    div()
        .debug_selector(move || selector.into())
        .absolute()
        .top(px(thumb_top))
        .right(px(INSET))
        .w(px(WIDTH))
        .h(px(thumb_height))
        .rounded_full()
        .bg(Theme::TEXT_FAINT)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_dimensions_stay_thin_and_touch_friendly() {
        assert!(WIDTH <= 4.0);
        assert!(MIN_THUMB_HEIGHT >= 24.0);
    }
}
