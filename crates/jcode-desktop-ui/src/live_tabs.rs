//! Labeled live-session folder tabs, independent of the sidebar's navigation.
use super::*;
use crate::panel::{MinimapSessionState, folder_session_title};

impl Workspace {
    pub(super) fn render_workspace_bar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut tabs = div()
            .id("live-session-tabs")
            .debug_selector(|| "live-session-tabs".into())
            .absolute()
            .top(px(STRIP_PADDING_Y))
            .left_0()
            .right(px(
                if self.show_minimap && !self.slots.is_empty() && !self.overview {
                    MINIMAP_WIDTH + MINIMAP_RIGHT + 8.0
                } else {
                    0.0
                },
            ))
            .h(px(FOLDER_CONTENT_INSET))
            .flex()
            .items_end()
            .gap(px(4.0))
            .overflow_x_scroll()
            .track_scroll(&self.live_tabs_scroll)
            .on_scroll_wheel(
                cx.listener(|this, event: &gpui::ScrollWheelEvent, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    let dx = if delta.x != px(0.0) { delta.x } else { delta.y };
                    let handle = &this.live_tabs_scroll;
                    let x = (handle.offset().x + dx).clamp(-handle.max_offset().x, px(0.0));
                    handle.set_offset(gpui::point(x, px(0.0)));
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        let populated_rows = (0..STRIP_COUNT)
            .filter(|row| self.row_indices(*row).next().is_some())
            .count();
        let mut position = 0;
        for row in 0..STRIP_COUNT {
            let indices: Vec<_> = self.row_indices(row).collect();
            if indices.is_empty() && row == self.active_row {
                if self.live_tabs_selection != Some((usize::MAX, position)) {
                    self.live_tabs_scroll.scroll_to_item(position);
                    self.live_tabs_selection = Some((usize::MAX, position));
                }
                tabs = tabs.child(
                    div()
                        .debug_selector(|| "live-session-empty-tab".into())
                        .flex_none()
                        .h(px(FOLDER_CONTENT_INSET))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded_t_md()
                        .bg(Theme::global().PANEL_BG)
                        .text_size(px(12.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(format!("Workspace {}", row + 1)),
                );
                position += 1;
            }
            for (row_position, index) in indices.into_iter().enumerate() {
                let focused = row == self.active_row && index == self.active;
                let panel = self.slots[index].panel.read(cx);
                let title = folder_session_title(&panel.session_id, panel.title.as_ref());
                let (state, color) = match panel.minimap_state() {
                    MinimapSessionState::Idle => ("idle", Theme::global().TEXT_FAINT),
                    MinimapSessionState::Working => ("working", Theme::global().WARN),
                    MinimapSessionState::Streaming => ("streaming", Theme::global().ACCENT),
                    MinimapSessionState::Complete => ("complete", Theme::global().OK),
                    MinimapSessionState::Error => ("error", Theme::global().ERROR),
                };
                if focused && self.live_tabs_selection != Some((index, position)) {
                    self.live_tabs_scroll.scroll_to_item(position);
                    self.live_tabs_selection = Some((index, position));
                }
                tabs = tabs.child(
                    div()
                        .id(("workspace-session", index))
                        .debug_selector(move || format!("live-session-tab-{index}"))
                        .relative()
                        .flex_none()
                        .max_w(px(220.0))
                        .min_w(px(100.0))
                        .h(px(if focused {
                            FOLDER_CONTENT_INSET
                        } else {
                            FOLDER_CONTENT_INSET - 4.0
                        }))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_t_md()
                        .border_1()
                        .border_b_0()
                        .border_color(if focused {
                            Theme::global().PANEL_BORDER_FOCUS
                        } else {
                            Theme::global().PANEL_BORDER
                        })
                        .bg(if focused {
                            Theme::global().PANEL_BG
                        } else {
                            Theme::global().HEADER_BG
                        })
                        .text_size(px(12.0))
                        .text_color(if focused {
                            Theme::global().TEXT
                        } else {
                            Theme::global().TEXT_DIM
                        })
                        .cursor_pointer()
                        .hover(|el| {
                            el.bg(Theme::global().PANEL_BG)
                                .text_color(Theme::global().TEXT)
                        })
                        .when(focused, |el| {
                            el.child(div().absolute().size_full().debug_selector(move || {
                                format!("live-session-tab-{index}-focused")
                            }))
                        })
                        .when(populated_rows > 1 && row_position == 0, |el| {
                            el.child(
                                div()
                                    .debug_selector(move || format!("live-session-row-label-{row}"))
                                    .text_size(px(10.0))
                                    .text_color(Theme::global().TEXT_FAINT)
                                    .child(format!("{}", row + 1)),
                            )
                        })
                        .child(
                            div()
                                .debug_selector(move || format!("live-session-tab-{index}-{state}"))
                                .flex_none()
                                .size(px(5.0))
                                .rounded_full()
                                .bg(color),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(title),
                        )
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.set_active(index, cx);
                                this.overview = false;
                                this.overview_progress.set(0.0, Instant::now());
                                this.focus_active(window, cx);
                                cx.notify();
                            }),
                        ),
                );
                position += 1;
            }
        }
        tabs.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn live_tabs_switch_rows_and_keep_the_folder_baseline(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.show_sidebar = false;
            for name in ["First session", "Second session", "Another workspace"] {
                workspace.push_test_panel(name, cx);
            }
            workspace.slots[2].row = 1;
            workspace
        });
        vcx.run_until_parked();
        let first = vcx.debug_bounds("live-session-tab-0").unwrap();
        let other = vcx.debug_bounds("live-session-tab-2").unwrap();
        assert_eq!(first.bottom(), other.bottom());
        assert_eq!(first.size.height - other.size.height, px(4.0));
        assert_eq!(first.bottom(), vcx.debug_bounds("panel-0").unwrap().top());
        assert!(vcx.debug_bounds("workspace-bar").is_none());
        assert!(vcx.debug_bounds("live-session-row-label-0").is_some());
        assert!(vcx.debug_bounds("live-session-row-label-1").is_some());
        vcx.simulate_click(other.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert_eq!(workspace.active, 2);
            assert_eq!(workspace.active_row, 1);
            assert!(!workspace.overview);
        });
        assert!(vcx.debug_bounds("live-session-tab-2-focused").is_some());
        assert!(vcx.debug_bounds("live-session-tab-0-focused").is_none());
    }

    #[gpui::test]
    fn live_tabs_reveal_keyboard_selection_in_a_narrow_window(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for index in 0..12 {
                workspace.push_test_panel(&format!("A long session title number {index}"), cx);
            }
            workspace
        });
        let handle = vcx.update(|window, cx| {
            window.focus(&workspace.read(cx).focus_handle.clone(), cx);
            window.window_handle()
        });
        vcx.simulate_window_resize(handle, gpui::size(px(800.), px(600.)));
        vcx.run_until_parked();
        vcx.simulate_keystrokes("super-p");
        vcx.run_until_parked();
        assert_eq!(workspace.read_with(vcx, |w, _| w.active), 11);
        let track = vcx.debug_bounds("live-session-tabs").unwrap();
        let selected = vcx.debug_bounds("live-session-tab-11").unwrap();
        assert!(selected.left() >= track.left());
        assert!(selected.right() <= track.right());
        assert!(selected.size.width >= px(100.));
        vcx.simulate_keystrokes("super-u");
        vcx.run_until_parked();
        let first = vcx.debug_bounds("live-session-tab-0").unwrap();
        assert_eq!(first.left(), track.left());
    }

    #[gpui::test]
    fn live_tabs_reserve_space_in_normal_mode_and_beside_minimap(cx: &mut gpui::TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.layout_mode = crate::config::LayoutMode::Normal;
            workspace.enable_test_minimap();
            workspace.push_test_panel("Session", cx);
            workspace
        });
        vcx.run_until_parked();
        let tabs = vcx.debug_bounds("live-session-tabs").unwrap();
        assert!(tabs.right() < vcx.debug_bounds("minimap").unwrap().left());
        assert!(tabs.bottom() <= vcx.debug_bounds("panel-0").unwrap().top());
    }

    #[gpui::test]
    fn empty_workspace_does_not_mark_another_rows_session_as_focused(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("Session", cx);
            workspace.select_row(2, 0);
            workspace
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("live-session-empty-tab").is_some());
        assert!(vcx.debug_bounds("live-session-tab-0-focused").is_none());
        let tab = vcx.debug_bounds("live-session-tab-0").unwrap();
        vcx.simulate_click(tab.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(workspace.read_with(vcx, |w, _| w.active_row), 0);
        assert!(vcx.debug_bounds("live-session-empty-tab").is_none());
        assert!(vcx.debug_bounds("live-session-tab-0-focused").is_some());
    }
}
