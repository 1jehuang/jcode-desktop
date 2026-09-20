//! Sidebar disclosure is presentation only. It never closes or moves a session.
use super::*;

#[derive(Default)]
pub(super) struct State {
    focused: Option<usize>,
    expanded: [bool; STRIP_COUNT],
}

impl State {
    pub(super) fn sync_focus(&mut self, row: usize) {
        if self.focused != Some(row) {
            self.focused = Some(row);
            self.expanded.fill(false);
            self.expanded[row] = true;
        }
    }

    pub(super) fn expanded(&self, row: usize) -> bool {
        self.expanded[row]
    }

    fn toggle(&mut self, row: usize) {
        self.expanded[row] = !self.expanded[row];
    }
}

impl Workspace {
    pub(super) fn render_workspace_group_marker(
        &self,
        row: usize,
        count: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let expanded = self.sidebar_workspace_groups.expanded(row);
        let active = row == self.active_row;
        let theme = Theme::global();
        let tooltip = format!(
            "{} workspace {} · {} session{}",
            if expanded { "Collapse" } else { "Expand" },
            row + 1,
            count,
            if count == 1 { "" } else { "s" },
        );
        div()
            .id(("sidebar-workspace-group", row))
            .debug_selector(move || format!("sidebar-workspace-group-{row}"))
            // A compact header above the sessions, not a reserved left gutter.
            .ml_2()
            .mb_1()
            .w(px(32.0))
            .h(px(26.0))
            .flex()
            .items_center()
            .gap(px(1.0))
            .rounded_md()
            .cursor_pointer()
            .hover(|el| el.bg(theme.TOOL_BG))
            .tooltip(move |_, cx| {
                cx.new(|_| remotes::HeaderTooltip(tooltip.clone().into()))
                    .into()
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.sidebar_workspace_groups.toggle(row);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if active {
                        theme.workspace_accent(row)
                    } else {
                        theme.TEXT_DIM
                    })
                    .when(active, |el| el.bg(theme.ACCENT_DIM))
                    .child((row + 1).to_string()),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme.TEXT_DIM)
                    .child(if expanded { "⌄" } else { "›" }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_expands_only_its_workspace_and_manual_disclosure_survives_renders() {
        let mut state = State::default();
        state.sync_focus(1);
        assert_eq!(state.expanded, [false, true, false, false]);
        state.toggle(0);
        state.sync_focus(1);
        assert_eq!(state.expanded, [true, true, false, false]);
        state.toggle(1);
        state.sync_focus(1);
        assert_eq!(state.expanded, [true, false, false, false]);
        state.sync_focus(2);
        assert_eq!(state.expanded, [false, false, true, false]);
    }
}
