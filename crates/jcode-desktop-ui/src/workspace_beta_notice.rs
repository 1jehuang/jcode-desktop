//! A dismissible launch overlay, never a permanent strip in the workspace.
use super::*;

impl Workspace {
    pub(super) fn dismiss_beta_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_beta_notice = false;
        self.restore_focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_beta_notice(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        div()
            .id("desktop-beta-overlay")
            .debug_selector(|| "desktop-beta-overlay".into())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.dismiss_beta_notice(window, cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .id("desktop-beta-notice")
                    .debug_selector(|| "desktop-beta-notice".into())
                    .w(px(420.0))
                    .max_w_full()
                    .p_6()
                    .rounded_lg()
                    .bg(theme.PANEL_BG)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .debug_selector(|| "desktop-beta-title".into())
                            .text_size(px(18.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.TEXT)
                            .child("Jcode Desktop is in beta testing"),
                    )
                    .child(
                        div()
                            .debug_selector(|| "desktop-beta-message".into())
                            .text_size(px(14.0))
                            .text_color(theme.TEXT)
                            .child("Expect bugs and unexpected behavior. Please report anything that goes wrong."),
                    )
                    .child(
                        div()
                            .id("desktop-beta-dismiss")
                            .debug_selector(|| "desktop-beta-dismiss".into())
                            .px_4()
                            .py_2()
                            .rounded_md()
                            .bg(theme.ACCENT.opacity(0.15))
                            .text_color(theme.TEXT)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.ACCENT.opacity(0.25)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_beta_notice(window, cx);
                                cx.stop_propagation();
                            }))
                            .child("Got it"),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn beta_overlay_dismissal_restores_composer_without_resizing(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_sidebar = false;
            w.open_startup_draft(cx);
            w.show_beta_notice = true;
            w.restore_focus(window, cx);
            w
        });
        vcx.run_until_parked();
        let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
        let notice = vcx.debug_bounds("desktop-beta-notice").unwrap();
        assert!(canvas.contains(&notice.center()));
        let button = vcx.debug_bounds("desktop-beta-dismiss").unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-notice").is_none());
        assert_eq!(vcx.debug_bounds("workspace-canvas").unwrap(), canvas);
        vcx.simulate_input("start working during beta");
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(
                w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                "start working during beta"
            );
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                let snapshot = w.snapshot(window, cx).unwrap();
                w.apply_snapshot(snapshot, cx);
                w.restore_focus(window, cx);
                cx.notify();
            })
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-notice").is_none());
    }

    #[gpui::test]
    fn beta_overlay_keyboard_dismissal_does_not_clear_or_submit_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.open_startup_draft(cx);
            w.restore_focus(window, cx);
            w
        });
        vcx.simulate_input("keep this draft");
        for key in ["escape", "enter", "space"] {
            vcx.update(|window, cx| {
                workspace.update(cx, |w, cx| {
                    w.show_beta_notice = true;
                    w.restore_focus(window, cx);
                    cx.notify();
                })
            });
            vcx.run_until_parked();
            vcx.simulate_keystrokes(key);
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("desktop-beta-notice").is_none(), "{key}");
            workspace.read_with(vcx, |w, cx| {
                assert_eq!(
                    w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                    "keep this draft",
                    "{key}"
                );
            });
        }
    }

    #[gpui::test]
    fn beta_overlay_fits_compact_and_wide_windows(cx: &mut gpui::TestAppContext) {
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_beta_notice = true;
            w.restore_focus(window, cx);
            w
        });
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [360.0, 640.0, 1440.0] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(700.0)));
            vcx.run_until_parked();
            let overlay = vcx.debug_bounds("desktop-beta-overlay").unwrap();
            let notice = vcx.debug_bounds("desktop-beta-notice").unwrap();
            let title = vcx.debug_bounds("desktop-beta-title").unwrap();
            let message = vcx.debug_bounds("desktop-beta-message").unwrap();
            let button = vcx.debug_bounds("desktop-beta-dismiss").unwrap();
            assert_eq!(overlay.size.width, px(width));
            assert!(notice.left() >= overlay.left() && notice.right() <= overlay.right());
            assert!(title.top() >= notice.top());
            assert!(title.bottom() <= message.top());
            assert!(message.bottom() <= button.top());
            assert!(button.bottom() <= notice.bottom());
        }
        // Clicking the scrim dismisses even when there is no session open.
        vcx.simulate_click(gpui::point(px(4.0), px(4.0)), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-notice").is_none());
    }
}
