//! Labeled live-session folder tabs, independent of the sidebar's navigation.
use super::*;
use crate::panel::{MinimapSessionState, folder_session_title};

// Share the available row equally, including gaps. Never impose a minimum
// width that would push another live session off-screen.
fn tab_sizing(available: f32, count: usize) -> (f32, f32) {
    let share = available.max(0.0) / count.max(1) as f32;
    let gap = (share * 0.05).min(2.0);
    let width = ((available.max(0.0) - gap * count.saturating_sub(1) as f32) / count.max(1) as f32)
        .min(144.0);
    (width, gap)
}

struct TabTooltip(gpui::SharedString);

impl Render for TabTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "live-session-tab-tooltip".into())
            .max_w(px(360.0))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .bg(Theme::global().PANEL_BG)
            .text_color(Theme::global().TEXT)
            .text_size(px(12.0))
            .child(self.0.clone())
    }
}

impl Workspace {
    pub(super) fn render_workspace_bar(
        &mut self,
        canvas_width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let right = if self.show_minimap && !self.slots.is_empty() && !self.overview {
            MINIMAP_WIDTH + MINIMAP_RIGHT + 8.0
        } else {
            0.0
        };
        let count =
            self.slots.len() + usize::from(self.row_indices(self.active_row).next().is_none());
        let (width, gap) = tab_sizing(canvas_width - right, count);
        let padding = (width / 8.0).min(6.0);
        let mut tabs = div()
            .id("live-session-tabs")
            .debug_selector(|| "live-session-tabs".into())
            .absolute()
            .top(px(STRIP_PADDING_Y))
            .left_0()
            .right(px(right))
            .h(px(FOLDER_CONTENT_INSET))
            .flex()
            .items_end()
            .gap(px(gap));
        let populated_rows = (0..STRIP_COUNT)
            .filter(|row| self.row_indices(*row).next().is_some())
            .count();
        for row in 0..STRIP_COUNT {
            let indices: Vec<_> = self.row_indices(row).collect();
            if indices.is_empty() && row == self.active_row {
                tabs = tabs.child(
                    div()
                        .debug_selector(|| "live-session-empty-tab".into())
                        .flex_1()
                        .max_w(px(144.0))
                        .min_w_0()
                        .overflow_hidden()
                        .h(px(FOLDER_CONTENT_INSET))
                        .px(px(padding))
                        .flex()
                        .items_center()
                        .rounded_t_md()
                        .bg(Theme::global().PANEL_BG)
                        .text_size(px(11.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(format!("Workspace {}", row + 1)),
                        ),
                );
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
                tabs = tabs.child(
                    div()
                        .id(("workspace-session", index))
                        .debug_selector(move || format!("live-session-tab-{index}"))
                        .relative()
                        .flex_1()
                        .max_w(px(144.0))
                        .min_w_0()
                        .overflow_hidden()
                        .h(px(if focused {
                            FOLDER_CONTENT_INSET
                        } else {
                            FOLDER_CONTENT_INSET - 4.0
                        }))
                        .px(px(padding))
                        .flex()
                        .items_center()
                        .gap(px((width / 12.0).min(4.0)))
                        .rounded_t_md()
                        .border(px((width / 12.0).min(1.0)))
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
                        .text_size(px(11.0))
                        .text_color(if focused {
                            Theme::global().TEXT
                        } else {
                            Theme::global().TEXT_DIM
                        })
                        .cursor_pointer()
                        .tooltip({
                            let title: gpui::SharedString = title.clone().into();
                            move |_, cx| cx.new(|_| TabTooltip(title.clone())).into()
                        })
                        .hover(|el| {
                            el.bg(Theme::global().PANEL_BG)
                                .text_color(Theme::global().TEXT)
                        })
                        .when(focused, |el| {
                            el.child(div().absolute().size_full().debug_selector(move || {
                                format!("live-session-tab-{index}-focused")
                            }))
                        })
                        .when(
                            width >= 64.0 && populated_rows > 1 && row_position == 0,
                            |el| {
                                el.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(Theme::global().TEXT_FAINT)
                                        .child(format!("{}", row + 1)),
                                )
                            },
                        )
                        .when(width >= 32.0, |el| {
                            el.child(
                                div()
                                    .debug_selector(move || {
                                        format!("live-session-tab-{index}-{state}")
                                    })
                                    .flex_none()
                                    .size(px(5.0))
                                    .rounded_full()
                                    .bg(color),
                            )
                        })
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
            }
        }
        tabs.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_tabs_sizing_always_fits_without_a_minimum_width() {
        for available in [0.0, 40.0, 180.0, 352.0, 800.0, 2400.0] {
            for count in 1..=200 {
                let (width, gap) = tab_sizing(available, count);
                assert!(width >= 0.0 && width <= 144.0);
                assert!(width * count as f32 + gap * (count - 1) as f32 <= available + 0.001);
            }
        }
    }

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
    fn live_tabs_all_remain_visible_during_resize_and_keyboard_navigation(
        cx: &mut gpui::TestAppContext,
    ) {
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
        assert!(
            selected.right() <= track.right() + px(1.),
            "selected={selected:?}, track={track:?}"
        );
        assert!(selected.size.width < px(100.));
        vcx.simulate_keystrokes("super-u");
        vcx.run_until_parked();
        let first = vcx.debug_bounds("live-session-tab-0").unwrap();
        assert_eq!(first.left(), track.left());
        vcx.update(|window, cx| window.simulate_mouse_move(first.center(), cx));
        vcx.run_until_parked();
        vcx.executor().advance_clock(Duration::from_secs(1));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("live-session-tab-tooltip").is_some());
        for width in [1440., 800., 640., 480., 1000.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            vcx.run_until_parked();
            let track = vcx.debug_bounds("live-session-tabs").unwrap();
            let mut previous_right = track.left();
            for index in 0..12 {
                let tab = vcx
                    .debug_bounds(Box::leak(
                        format!("live-session-tab-{index}").into_boxed_str(),
                    ))
                    .unwrap();
                assert!(tab.left() >= previous_right);
                assert!(tab.right() <= track.right() + px(1.));
                assert!(tab.size.width > px(0.) && tab.size.width <= px(144.));
                previous_right = tab.right();
            }
            let last = vcx.debug_bounds("live-session-tab-11").unwrap();
            vcx.simulate_click(last.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(workspace.read_with(vcx, |w, _| w.active), 11);
        }
    }

    #[gpui::test]
    fn live_tabs_reserve_space_in_normal_mode_and_beside_minimap(cx: &mut gpui::TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.layout_mode = crate::config::LayoutMode::Normal;
            workspace.enable_test_minimap();
            for index in 0..24 {
                workspace.push_test_panel(&format!("Session {index}"), cx);
            }
            workspace
        });
        vcx.run_until_parked();
        let tabs = vcx.debug_bounds("live-session-tabs").unwrap();
        assert!(tabs.right() < vcx.debug_bounds("minimap").unwrap().left());
        assert!(tabs.bottom() <= vcx.debug_bounds("panel-0").unwrap().top());
        for index in 0..24 {
            let tab = vcx
                .debug_bounds(Box::leak(
                    format!("live-session-tab-{index}").into_boxed_str(),
                ))
                .unwrap();
            assert!(tab.left() >= tabs.left());
            assert!(tab.right() <= tabs.right() + px(1.));
        }
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
