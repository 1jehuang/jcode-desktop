//! Workspace navigation stays outside the virtualized chat/history list.
use super::*;

impl Workspace {
    pub(super) fn render_sidebar_workspaces(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        // Use live panels, not session metadata: newly opened workspaces must
        // be reachable before the next session-list refresh arrives.
        let rows = (0..STRIP_COUNT)
            .map(|row| {
                (
                    row,
                    self.row_indices(row)
                        .filter(|&index| !self.slots[index].closing)
                        .count(),
                )
            })
            .filter(|(row, count)| *count > 0 || *row == self.active_row)
            .collect::<Vec<_>>();
        if rows.len() < 2 {
            return None;
        }

        Some(
            div()
                .id("sidebar-workspaces")
                .debug_selector(|| "sidebar-workspaces".into())
                .flex_none()
                .px_2()
                .pt_2()
                .pb_2()
                .flex()
                .gap_1()
                .children(rows.into_iter().map(|(row, count)| {
                    let active = row == self.active_row;
                    div()
                        .id(("sidebar-workspace-switch", row))
                        .debug_selector(move || format!("sidebar-workspace-switch-{row}"))
                        .h(px(28.0))
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(Theme::global().workspace_accent(row))
                        .when(active, |el| {
                            el.bg(Theme::global().workspace_accent(row).opacity(0.16))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                        })
                        .hover(|el| el.bg(Theme::global().HEADER_BG))
                        .tooltip(move |_, cx| cx.new(|_| remotes::HeaderTooltip(
                            format!("Workspace {} · {count} {}", row + 1, if count == 1 { "chat" } else { "chats" }).into()
                        )).into())
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                this.switch_row_animated(row, window, cx);
                                cx.notify();
                            }),
                        )
                        .child((row + 1).to_string())
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn sidebar_workspaces_stay_pinned_and_navigate_with_long_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |w, cx| {
            for row in 0..STRIP_COUNT {
                w.active_row = row;
                w.push_test_panel(&format!("live-{row}"), cx);
            }
            // Deliberately exclude live panels from the metadata refresh.
            w.sessions = (0..200)
                .map(|index| {
                    super::super::tests::session_info(&format!("history-{index}"), Some("Old chat"))
                })
                .collect();
            cx.notify();
        });
        vcx.run_until_parked();
        let footer = vcx.debug_bounds("sidebar-workspaces").unwrap();
        let sidebar = vcx.debug_bounds("sidebar").unwrap();
        let list = vcx.debug_bounds("sidebar-session-list").unwrap();
        assert_eq!(footer.bottom(), sidebar.bottom());
        assert!(list.bottom() <= footer.top());
        workspace.update(vcx, |w, cx| {
            w.sidebar_sessions_list.scroll_to_end();
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(vcx.debug_bounds("sidebar-workspaces").unwrap(), footer);
        assert!(vcx.debug_bounds("sidebar-session-199").is_some());
        for row in 0..STRIP_COUNT {
            let button = vcx
                .debug_bounds(
                    [
                        "sidebar-workspace-switch-0",
                        "sidebar-workspace-switch-1",
                        "sidebar-workspace-switch-2",
                        "sidebar-workspace-switch-3",
                    ][row],
                )
                .unwrap();
            assert!(button.top() >= footer.top() && button.bottom() <= footer.bottom());
            vcx.simulate_click(button.center(), gpui::Modifiers::default());
            workspace.read_with(vcx, |w, cx| {
                assert_eq!(w.active_row, row);
                assert_eq!(
                    w.slots[w.active].panel.read(cx).session_id,
                    format!("live-{row}")
                );
            });
        }
        let handle = vcx.update(|window, _| window.window_handle());
        for mode in [
            crate::config::LayoutMode::Normal,
            crate::config::LayoutMode::FolderTabs,
        ] {
            for (width, height) in [(640., 480.), (1440., 1000.)] {
                vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
                workspace.update(vcx, |w, cx| {
                    w.layout_mode = mode;
                    w.compact_sidebar_open = responsive::is_compact(width);
                    cx.notify();
                });
                vcx.run_until_parked();
                let footer = vcx.debug_bounds("sidebar-workspaces").unwrap();
                let sidebar = vcx.debug_bounds("sidebar").unwrap();
                let list = vcx.debug_bounds("sidebar-session-list").unwrap();
                assert_eq!(footer.bottom(), sidebar.bottom());
                assert!(list.bottom() <= footer.top());
                assert!(list.size.height > px(100.0));
                let last = vcx.debug_bounds("sidebar-workspace-switch-3").unwrap();
                assert!(last.bottom() <= sidebar.bottom());
            }
        }
    }

    #[gpui::test]
    fn sidebar_workspaces_follow_live_rows_and_only_show_in_chat(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |w, cx| {
            w.push_test_panel("first", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-workspaces").is_none());
        workspace.update(vcx, |w, cx| {
            w.active_row = 2;
            w.push_test_panel("second", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-workspace-switch-0").is_some());
        assert!(vcx.debug_bounds("sidebar-workspace-switch-1").is_none());
        assert!(vcx.debug_bounds("sidebar-workspace-switch-2").is_some());
        workspace.update(vcx, |w, cx| {
            w.sidebar_view = SidebarView::Files;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-workspaces").is_none());
        workspace.update(vcx, |w, cx| {
            w.sidebar_view = SidebarView::Sessions;
            w.slots[0].closing = true;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-workspaces").is_none());
    }
}
