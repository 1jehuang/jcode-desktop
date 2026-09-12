//! Compact chrome is a presentation of the same workspace, not a saved layout.
use super::*;

const COMPACT_BREAKPOINT: f32 = 1100.0;
pub(super) const SINGLE_PANEL_BREAKPOINT: f32 = 1100.0;
const RAIL_WIDTH: f32 = 48.0;

pub(super) fn is_compact(width: f32) -> bool {
    width < COMPACT_BREAKPOINT
}

pub(super) fn sidebar_width(visible: bool, compact: bool) -> f32 {
    if !visible {
        0.0
    } else if compact {
        RAIL_WIDTH
    } else {
        SIDEBAR_WIDTH
    }
}

struct RailTooltip(gpui::SharedString);

impl Render for RailTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(Theme::global().PANEL_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_size(px(12.0))
            .text_color(Theme::global().TEXT)
            .child(self.0.clone())
    }
}

impl Workspace {
    pub(super) fn render_compact_navigation(
        &mut self,
        fullscreen: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let button = |id: &'static str, label: &'static str, title: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .size(px(36.0))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(17.0))
                .text_color(Theme::global().TEXT_DIM)
                .hover(|el| {
                    el.bg(Theme::global().PANEL_BG)
                        .text_color(Theme::global().TEXT)
                })
                .tooltip(move |_, cx| cx.new(|_| RailTooltip(title.into())).into())
                .child(label)
        };
        let mut rail = div()
            .debug_selector(|| "compact-sidebar".into())
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(if cfg!(target_os = "macos") && !fullscreen {
                TITLEBAR_HEIGHT
            } else {
                10.0
            }))
            .gap(px(6.0))
            .child(
                button("compact-sidebar-toggle", "☰", "Open sidebar").on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.compact_sidebar_open = !this.compact_sidebar_open;
                        window.focus(&this.focus_handle, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("compact-new-session", "+", "New session").on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.new_panel(&NewPanel, window, cx);
                        cx.stop_propagation();
                    }),
                ),
            )
            .child(
                div()
                    .w(px(20.0))
                    .h(px(1.0))
                    .my_2()
                    .bg(Theme::global().PANEL_BORDER),
            );
        for row in 0..STRIP_COUNT {
            let selected = row == self.active_row;
            rail = rail.child(
                div()
                    .id(("compact-workspace", row))
                    .debug_selector(move || format!("compact-workspace-{row}"))
                    .size(px(32.0))
                    .flex_none()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(if selected {
                        Theme::global().TEXT
                    } else {
                        Theme::global().TEXT_DIM
                    })
                    .when(selected, |el| {
                        el.bg(Theme::global().PANEL_BG)
                            .border_1()
                            .border_color(Theme::global().PANEL_BORDER)
                    })
                    .hover(|el| el.bg(Theme::global().PANEL_BG))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.compact_sidebar_open = false;
                            let position = this.active_position_in_row();
                            this.select_row(row, position);
                            this.focus_active(window, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .tooltip(move |_, cx| {
                        cx.new(|_| RailTooltip(format!("Workspace {}", row + 1).into()))
                            .into()
                    })
                    .child((row + 1).to_string()),
            );
        }
        rail.into_any_element()
    }

    pub(super) fn render_compact_sidebar_overlay(
        &mut self,
        fullscreen: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .absolute()
            .top(px(FPS_HEADER_HEIGHT))
            .bottom_0()
            .left_0()
            .right_0()
            .debug_selector(|| "compact-sidebar-overlay".into())
            .child(
                div()
                    .id("compact-sidebar-backdrop")
                    .debug_selector(|| "compact-sidebar-backdrop".into())
                    .absolute()
                    .size_full()
                    .bg(gpui::rgba(0x00000066))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.compact_sidebar_open = false;
                            this.focus_active(window, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(SIDEBAR_WIDTH))
                    .bg(Theme::global().HEADER_BG)
                    .border_r_1()
                    .border_color(Theme::global().PANEL_BORDER)
                    .occlude()
                    .child(self.render_sidebar(fullscreen, cx)),
            )
            .child(
                div()
                    .id("compact-sidebar-close")
                    .debug_selector(|| "compact-sidebar-close".into())
                    .absolute()
                    .left(px(SIDEBAR_WIDTH + 8.0))
                    .top(px(10.0))
                    .size(px(36.0))
                    .rounded_md()
                    .bg(Theme::global().PANEL_BG)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("×")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.compact_sidebar_open = false;
                            this.focus_active(window, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn resize_prioritizes_one_chat_and_restores_width_presets(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            for name in ["First", "Second"] {
                w.push_test_panel(name, cx);
            }
            for slot in &mut w.slots {
                slot.width_fraction = 0.5;
                slot.animated_width =
                    AnimatedValue::new(0.5, transition::policy(Transition::PanelWidth).duration);
            }
            w.active = 1;
            w
        });
        let handle = vcx.update(|window, _| window.window_handle());
        for (width, height) in [
            (1600., 900.),
            (960., 700.),
            (800., 600.),
            (640., 480.),
            (480., 640.),
            (1600., 600.),
        ] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
            vcx.run_until_parked();
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
            let panel = vcx.debug_bounds("panel-1").unwrap();
            assert!(
                panel.left() >= canvas.left() - px(1.0),
                "{width}: {panel:?} vs {canvas:?}"
            );
            assert!(
                panel.right() <= canvas.right() + px(1.0),
                "{width}: {panel:?} vs {canvas:?}"
            );
            assert!(panel.bottom() <= px(height));
            if is_compact(width) {
                assert!(vcx.debug_bounds("sidebar").is_none());
                assert_eq!(
                    vcx.debug_bounds("compact-sidebar").unwrap().size.width,
                    px(RAIL_WIDTH)
                );
                assert!((panel.size.width - canvas.size.width).abs() < px(2.0));
                if let Some(other) = vcx.debug_bounds("panel-0") {
                    assert!(other.right() <= canvas.left() + px(1.0));
                }
            } else {
                assert!(vcx.debug_bounds("compact-sidebar").is_none());
                assert_eq!(
                    vcx.debug_bounds("sidebar").unwrap().size.width,
                    px(SIDEBAR_WIDTH)
                );
                assert!((panel.size.width - canvas.size.width / 2.0).abs() < px(2.0));
            }
            workspace.read_with(vcx, |w, _| {
                assert!(w.show_sidebar);
                assert_eq!(w.active, 1);
                assert!(w.slots.iter().all(|s| s.width_fraction == 0.5));
            });
        }
    }

    #[gpui::test]
    fn compact_sidebar_opens_without_reflow_and_escape_preserves_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("Draft", cx);
            w
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(800.), px(600.)));
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            w.focus_pending = true;
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.simulate_input("Keep this draft");
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        let before = vcx.debug_bounds("workspace-canvas").unwrap();
        for dismiss in ["escape", "close", "backdrop"] {
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            let toggle = vcx.debug_bounds("compact-sidebar-toggle").unwrap();
            vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("sidebar").is_some());
            assert_eq!(vcx.debug_bounds("workspace-canvas").unwrap(), before);
            match dismiss {
                "escape" => vcx.simulate_keystrokes("escape"),
                "close" => {
                    let close = vcx.debug_bounds("compact-sidebar-close").unwrap();
                    vcx.simulate_click(close.center(), gpui::Modifiers::default());
                }
                _ => {
                    vcx.simulate_click(gpui::point(px(750.), px(300.)), gpui::Modifiers::default())
                }
            }
            vcx.run_until_parked();
            assert!(
                vcx.debug_bounds("compact-sidebar-overlay").is_none(),
                "{dismiss}"
            );
            workspace.read_with(vcx, |w, cx| {
                assert!(!w.compact_sidebar_open);
                assert_eq!(
                    w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                    "Keep this draft"
                );
            });
        }
        // Explicitly hiding the sidebar remains respected across resizing.
        workspace.update(vcx, |w, cx| {
            w.show_sidebar = false;
            cx.notify();
        });
        for width in [800., 1600., 640.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("sidebar").is_none());
            assert!(vcx.debug_bounds("compact-sidebar").is_none());
        }
    }
}
