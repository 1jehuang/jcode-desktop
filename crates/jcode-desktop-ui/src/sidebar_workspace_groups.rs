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
            .w(px(64.0))
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
            .child(
                div()
                    .ml_1()
                    .text_size(px(10.0))
                    .text_color(theme.TEXT_DIM)
                    .child(count.to_string()),
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

    #[gpui::test]
    fn live_slots_without_catalog_entries_appear_immediately_in_map_order(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            // Creation order deliberately differs from the map's row-major order.
            workspace.active_row = 1;
            workspace.push_test_panel("session_fox_lower", cx);
            workspace.active_row = 0;
            workspace.push_test_panel("session_owl_upper_left", cx);
            workspace.push_test_panel("session_hare_upper_right", cx);
            workspace.active = 1;
            assert!(workspace.sessions.is_empty());
            workspace
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert!(
                workspace.sessions.is_empty(),
                "rendering must not populate the catalog"
            );
            assert_eq!(
                workspace
                    .sidebar_session_layout
                    .iter()
                    .map(|item| item.session_id.as_str())
                    .collect::<Vec<_>>(),
                vec![
                    "session_owl_upper_left",
                    "session_hare_upper_right",
                    "session_fox_lower"
                ],
                "live slots must not wait for a Sessions update"
            );
        });
        let first = vcx
            .debug_bounds("sidebar-session-0")
            .expect("first live slot paints immediately");
        let second = vcx
            .debug_bounds("sidebar-session-1")
            .expect("second live slot paints immediately");
        let lower = vcx
            .debug_bounds("sidebar-workspace-group-1")
            .expect("uncatalogued workspace has a disclosure");
        assert!(first.bottom() <= second.top());
        assert!(second.bottom() <= lower.top());
        assert!(vcx.debug_bounds("sidebar-session-2").is_none());
        vcx.simulate_click(lower.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-session-2").is_some());
        for (selector, expected) in [
            ("sidebar-session-0", "session_owl_upper_left"),
            ("sidebar-session-1", "session_hare_upper_right"),
            ("sidebar-session-2", "session_fox_lower"),
        ] {
            let bounds = vcx.debug_bounds(selector).unwrap();
            vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            workspace.read_with(vcx, |workspace, cx| {
                assert_eq!(
                    workspace.slots.len(),
                    3,
                    "clicking a live row must not open a duplicate"
                );
                assert_eq!(
                    workspace.slots[workspace.active].panel.read(cx).session_id,
                    expected
                );
            });
        }
    }

    #[gpui::test]
    fn uncatalogued_inactive_live_rows_are_compact_and_remeasure_on_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for name in [
                "session_fox_first",
                "session_owl_second",
                "session_hare_third",
            ] {
                workspace.push_test_panel(name, cx);
            }
            for slot in &workspace.slots {
                slot.panel.update(cx, |panel, _| {
                    panel.working_dir = Some("/project/live-work".into());
                });
            }
            workspace.active = 0;
            assert!(workspace.sessions.is_empty());
            workspace
        });
        vcx.run_until_parked();
        for active in [0, 1, 2, 0] {
            workspace.update(vcx, |workspace, cx| {
                workspace.active = active;
                cx.notify();
            });
            vcx.run_until_parked();
            workspace.read_with(vcx, |workspace, _| {
                assert!(workspace.sessions.is_empty());
                assert_eq!(workspace.sidebar_session_layout.len(), 3);
                for (index, item) in workspace.sidebar_session_layout.iter().enumerate() {
                    assert_eq!(
                        item.details,
                        index == active,
                        "virtual-list measurements must track the visible detail row"
                    );
                }
            });
            for (index, selector) in [
                "sidebar-session-0",
                "sidebar-session-1",
                "sidebar-session-2",
            ]
            .into_iter()
            .enumerate()
            {
                let bounds = vcx.debug_bounds(selector).unwrap();
                if index == active {
                    assert!(
                        bounds.size.height > px(24.0),
                        "active row retains working-directory details"
                    );
                } else {
                    assert_eq!(
                        bounds.size.height,
                        px(24.0),
                        "inactive live rows stay single-line compact"
                    );
                }
            }
        }
    }
}
