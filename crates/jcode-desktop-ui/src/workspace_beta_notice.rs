//! A non-blocking startup notification with a three-second countdown.
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
    pub(super) fn dismiss_beta_notice(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_beta_notice = false;
        cx.notify();
    }

    pub(super) fn render_beta_notice(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let workspace = cx.weak_entity();
        // The task lives only while the notification is rendered. Repaints do not
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
            .id("desktop-beta-notice")
            .debug_selector(|| "desktop-beta-notice".into())
            .absolute()
            .top(px(48.0))
            .right(px(16.0))
            .w(px(380.0))
            .max_w((window.viewport_size().width - px(32.0)).max(px(0.0)))
            .p_3()
            .rounded_lg()
            .bg(theme.PANEL_BG)
            .border_1()
            .border_color(theme.ACCENT.opacity(0.2))
            .shadow_md()
            .occlude()
            .flex()
            .items_start()
            .gap_3()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| "desktop-beta-title".into())
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.TEXT)
                            .child("Jcode Desktop is in beta testing"),
                    )
                    .child(
                        div()
                            .debug_selector(|| "desktop-beta-message".into())
                            .text_size(px(12.0))
                            .text_color(theme.TEXT_DIM)
                            .child("Expect bugs. Please report anything that goes wrong."),
                    ),
            )
            .child(
                div()
                    .id("desktop-beta-dismiss")
                    .debug_selector(|| "desktop-beta-dismiss".into())
                    .flex_none()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.ACCENT.opacity(0.15)))
                    .tooltip(|_, cx| {
                        cx.new(|_| super::remotes::HeaderTooltip("Dismiss notification".into()))
                            .into()
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dismiss_beta_notice(window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .debug_selector(|| "desktop-beta-countdown".into())
                            .with_animation(
                                "desktop-beta-countdown-animation",
                                Animation::new(COUNTDOWN).with_max_fps(30.0),
                                |ring, progress| ring.child(countdown_ring(progress)),
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
    fn beta_countdown_expires_without_interrupting_input(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.open_startup_draft(cx);
            w.show_beta_notice = true;
            w.restore_focus(window, cx);
            w
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-countdown").is_some());
        vcx.simulate_input("typing immediately ");
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(
                w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                "typing immediately "
            );
        });
        assert!(vcx.debug_bounds("desktop-beta-notice").is_some());
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
                "typing immediately ready after countdown"
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
        let button = vcx.debug_bounds("desktop-beta-dismiss").unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
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
    fn beta_notification_dismissal_preserves_composer_without_resizing(
        cx: &mut gpui::TestAppContext,
    ) {
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
    fn beta_notification_does_not_capture_keys_or_steal_focus(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.open_startup_draft(cx);
            w.show_beta_notice = true;
            w.restore_focus(window, cx);
            w
        });
        vcx.run_until_parked();
        vcx.simulate_input("hello");
        vcx.simulate_keystrokes("space");
        vcx.simulate_input("world");
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(
                w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                "hello world"
            );
        });
        assert!(vcx.debug_bounds("desktop-beta-notice").is_some());
        let other_focus = vcx.update(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            focus
        });
        vcx.executor().advance_clock(COUNTDOWN);
        vcx.run_until_parked();
        vcx.update(|window, _| assert!(other_focus.is_focused(window)));
        assert!(vcx.debug_bounds("desktop-beta-notice").is_none());
    }

    #[gpui::test]
    fn beta_notification_fits_compact_and_wide_windows(cx: &mut gpui::TestAppContext) {
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
            assert!(vcx.debug_bounds("desktop-beta-overlay").is_none());
            let notice = vcx.debug_bounds("desktop-beta-notice").unwrap();
            let title = vcx.debug_bounds("desktop-beta-title").unwrap();
            let message = vcx.debug_bounds("desktop-beta-message").unwrap();
            let button = vcx.debug_bounds("desktop-beta-dismiss").unwrap();
            assert!(notice.left() >= px(16.0) && notice.right() <= px(width - 16.0));
            assert!(notice.top() >= px(40.0) && notice.bottom() < px(200.0));
            assert!(title.top() >= notice.top());
            assert!(title.bottom() <= message.top());
            assert!(message.right() <= button.left());
            assert!(button.bottom() <= notice.bottom());
        }
        // No full-window scrim: clicks elsewhere do not dismiss or get captured.
        vcx.simulate_click(gpui::point(px(4.0), px(400.0)), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-beta-notice").is_some());
    }
}
