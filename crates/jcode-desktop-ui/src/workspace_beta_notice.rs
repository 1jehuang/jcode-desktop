//! A dismissible launch overlay, never a permanent strip in the workspace.
use super::*;
use gpui::{Animation, AnimationExt};

const COUNTDOWN: Duration = Duration::from_secs(3);

fn countdown_ring(progress: f32) -> gpui::AnyElement {
    let theme = Theme::global();
    let remaining = (1.0 - progress).clamp(0.0, 1.0);
    div()
        .relative()
        .size(px(28.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .child(format!("{:.0}", (remaining * 3.0).ceil()))
        .child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    for (fraction, color) in
                        [(1.0, theme.ACCENT.opacity(0.18)), (remaining, theme.ACCENT)]
                    {
                        if fraction <= 0.0 {
                            continue;
                        }
                        let mut path = gpui::PathBuilder::stroke(px(2.0));
                        let steps = (96.0 * fraction).ceil() as usize;
                        for step in 0..=steps {
                            let angle = -std::f32::consts::FRAC_PI_2
                                + std::f32::consts::TAU * fraction * step as f32 / steps as f32;
                            let point = bounds.center()
                                + gpui::point(px(angle.cos() * 12.0), px(angle.sin() * 12.0));
                            if step == 0 {
                                path.move_to(point);
                            } else {
                                path.line_to(point);
                            }
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .into_any_element()
}

impl Workspace {
    pub(super) fn dismiss_beta_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_beta_notice = false;
        self.restore_focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_beta_notice(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let workspace = cx.weak_entity();
        // The task lives only while the overlay is rendered. Repaints do not
        // restart it, and manual dismissal cancels it without stealing focus later.
        window.use_keyed_state("desktop-beta-timer", cx, |window, cx| {
            let timer = cx.background_executor().timer(COUNTDOWN);
            cx.spawn_in(window, async move |_, cx| {
                timer.await;
                let _ = cx.update(|window, cx| {
                    let _ = workspace.update(cx, |workspace, cx| {
                        if workspace.show_beta_notice {
                            workspace.dismiss_beta_notice(window, cx);
                        }
                    });
                });
            })
        });
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
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .hover(|style| style.bg(theme.ACCENT.opacity(0.25)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_beta_notice(window, cx);
                                cx.stop_propagation();
                            }))
                            .child("Got it")
                            .child(
                                div()
                                    .debug_selector(|| "desktop-beta-countdown".into())
                                    .with_animation(
                                        "desktop-beta-countdown-animation",
                                        Animation::new(COUNTDOWN).with_max_fps(30.0),
                                        |ring, progress| ring.child(countdown_ring(progress)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn beta_countdown_expires_after_three_seconds_and_restores_input(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.open_startup_draft(cx);
            w.show_beta_notice = true;
            w.restore_focus(window, cx);
            w
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-countdown").is_some());
        vcx.executor().advance_clock(Duration::from_millis(2000));
        // An unrelated workspace repaint must not restart the countdown.
        workspace.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        vcx.executor().advance_clock(Duration::from_millis(999));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-notice").is_some());
        vcx.executor().advance_clock(Duration::from_millis(1));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-notice").is_none());
        vcx.simulate_input("ready after countdown");
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(
                w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                "ready after countdown"
            );
        });
    }

    #[gpui::test]
    fn early_beta_dismissal_does_not_refocus_after_timeout(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.open_startup_draft(cx);
            w.show_beta_notice = true;
            w.restore_focus(window, cx);
            w
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        let other_focus = vcx.update(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            focus
        });
        vcx.executor().advance_clock(COUNTDOWN);
        vcx.run_until_parked();
        vcx.update(|window, _| assert!(other_focus.is_focused(window)));
        workspace.read_with(vcx, |w, _| assert!(!w.show_beta_notice));
    }

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
