//! Geometry retained across the welcome screen's first submission.
//!
//! Empty space is a minimum viewport height, not blank transcript rows. The
//! editor keeps its entity, width and height while content consumes that space.
use super::*;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartupLayout {
    input_top: f32,
    pub(super) committed: bool,
    pub messages_height: f32,
    preview: bool,
}

pub(super) fn input_marker(
    bounds: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
) -> gpui::AnyElement {
    gpui::canvas(|_, _, _| (), move |rect, _, _, _| bounds.set(Some(rect)))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
}

impl Panel {
    pub(super) fn release_startup_preview(&mut self) {
        if let Some(layout) = &mut self.startup_layout {
            if layout.preview && !self.items.is_empty() {
                self.stick_to_bottom = false;
            }
            layout.preview = false;
        }
    }

    pub(super) fn startup_prompt_preview(&mut self) -> bool {
        let Some(layout) = &mut self.startup_layout else {
            return false;
        };
        // A first prompt, including its attachments, should open at its start.
        // As soon as the answer arrives, ordinary tail following takes over.
        let first_prompt_only = self
            .items
            .iter()
            .filter(|item| matches!(item, Item::User(_)))
            .count()
            == 1
            && self
                .items
                .iter()
                .all(|item| matches!(item, Item::User(_) | Item::Image(_)))
            && self.streaming_text.is_empty()
            && self.streaming_reasoning.is_empty();
        if !self.items.is_empty() && !first_prompt_only {
            layout.preview = false;
        }
        layout.preview && first_prompt_only && self.stick_to_bottom
    }

    /// Observe the actual editor and list after paint, including wrapped text,
    /// attachments and session chrome. Do not guess line heights from strings.
    pub(super) fn startup_layout_observer(
        &self,
        fresh: bool,
        input: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
        body: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let panel = cx.entity().downgrade();
        let list = self.transcript_list.clone();
        gpui::canvas(
            |_, _, _| (),
            move |panel_bounds, _, _, cx| {
                let Some(input) = input.get() else { return };
                let Some(body) = body.get() else { return };
                let viewport = list.viewport_bounds();
                let measured_content =
                    f32::from(viewport.size.height + list.max_offset_for_scrollbar().y);
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |panel, cx| {
                        if fresh {
                            // Remember without notifying: this measurement does
                            // not change the already correct welcome frame.
                            panel.startup_layout = Some(StartupLayout {
                                input_top: f32::from(input.top() - panel_bounds.top()),
                                committed: false,
                                messages_height: f32::from(input.top() - body.top()),
                                preview: true,
                            });
                        } else if let Some(layout) = &mut panel.startup_layout {
                            let available = f32::from(body.size.height - input.size.height).max(0.);
                            let floor = (layout.input_top
                                - f32::from(body.top() - panel_bounds.top()))
                            .max(0.);
                            // messages_height includes the non-scrolling gap,
                            // while the list's measurements contain rows only.
                            let height = floor
                                .max(measured_content + TRANSCRIPT_BOTTOM_GAP)
                                .min(available);
                            if (height - layout.messages_height).abs() > 0.5 {
                                layout.messages_height = height;
                                cx.notify();
                            }
                        }
                    });
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }
}
