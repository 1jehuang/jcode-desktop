//! Stateful rich comparison content for the file-tree review surface.
use std::collections::HashMap;

use gpui::{AnyElement, App, Entity, ScrollHandle, div, prelude::*};

use crate::{diff_model::DiffPreview, diff_view::DiffView};

pub(crate) struct ReviewContent {
    preview: DiffPreview,
    selected: usize,
    done: bool,
    failed: bool,
    views: HashMap<usize, (Entity<DiffView>, ScrollHandle)>,
}

impl ReviewContent {
    pub(crate) fn new(
        preview: DiffPreview,
        selected: usize,
        done: bool,
        failed: bool,
        cx: &mut App,
    ) -> Self {
        let mut content = Self {
            preview,
            selected,
            done,
            failed,
            views: HashMap::new(),
        };
        content.select(selected, cx);
        content
    }

    pub(crate) fn select(&mut self, selected: usize, cx: &mut App) {
        self.selected = selected;
        if !self.views.contains_key(&selected)
            && let Some(file) = self.preview.files.get(selected)
        {
            let view = cx.new(|cx| {
                let mut view = DiffView::new(
                    DiffPreview {
                        files: vec![file.clone()],
                    },
                    cx,
                );
                view.set_status(self.done, self.failed, cx);
                view
            });
            self.views.insert(selected, (view, ScrollHandle::new()));
        }
    }

    pub(crate) fn scroll(&self) -> Option<&ScrollHandle> {
        self.views.get(&self.selected).map(|(_, scroll)| scroll)
    }

    pub(crate) fn render(&self) -> AnyElement {
        let Some((view, scroll)) = self.views.get(&self.selected) else {
            return div()
                .child("No change content supplied.")
                .into_any_element();
        };
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .relative()
            .child(
                div()
                    .id("rich-review-scroll")
                    .debug_selector(|| "rich-review-scroll".into())
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(scroll)
                    .p_2()
                    .child(view.clone()),
            )
            .child(crate::scrollbar::vertical(scroll, "rich-review-scrollbar"))
            .into_any_element()
    }
}
