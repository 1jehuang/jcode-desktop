//! A cylindrical folder selector for the top-left navigation, not the live sessions.
use super::*;

const COUNT: usize = 11;
const STEP: f32 = 48.0;
const TABS: [(&str, &str, Option<SidebarView>); COUNT] = [
    ("sidebar-sessions-tab", "chat", Some(SidebarView::Sessions)),
    ("sidebar-learn-tab", "learn", Some(SidebarView::Learn)),
    ("sidebar-files-tab", "files", Some(SidebarView::Files)),
    (
        "sidebar-accounts-tab",
        "accounts",
        Some(SidebarView::Accounts),
    ),
    ("sidebar-theme-tab", "theme", Some(SidebarView::Theme)),
    (
        "sidebar-settings-tab",
        "settings",
        Some(SidebarView::Settings),
    ),
    ("sidebar-unfinished-work", "todos", None),
    ("open-todoist", "todoist", None),
    ("open-gmail", "email", None),
    ("sidebar-open-folder", "folder", None),
    ("sidebar-new-session", "+", None),
];

fn distance(index: usize, center: f32) -> f32 {
    (index as f32 - center + COUNT as f32 / 2.0).rem_euclid(COUNT as f32) - COUNT as f32 / 2.0
}

#[derive(Clone, Copy, Debug)]
struct Geometry {
    left: f32,
    width: f32,
    height: f32,
    bottom: f32,
    depth: f32,
}

fn geometry(offset: f32, available: f32) -> Geometry {
    let angle = offset.clamp(-3.3, 3.3) * 0.45;
    let depth = angle.cos().max(0.08);
    let width = ((available * 0.35).min(84.0) * depth.powi(2)).max(7.0);
    Geometry {
        left: available / 2.0 + (available / 2.0 - 13.0) * angle.sin() - width / 2.0,
        width,
        height: 10.0 + 24.0 * depth,
        bottom: 7.0 * (1.0 - depth),
        depth,
    }
}

pub(super) struct Roller {
    position: AnimatedValue,
    target: f32,
    remainder: f32,
    view: Option<SidebarView>,
}

impl Default for Roller {
    fn default() -> Self {
        Self {
            position: AnimatedValue::new(0.0, transition::policy(Transition::Focus).duration),
            target: 0.0,
            remainder: 0.0,
            view: None,
        }
    }
}

impl Roller {
    fn move_to(&mut self, target: f32) {
        self.move_to_at(
            target,
            transition::policy(Transition::Focus).duration,
            Instant::now(),
        );
    }

    fn move_to_at(&mut self, target: f32, duration: Duration, now: Instant) {
        self.target = target;
        if duration.is_zero() {
            self.position = AnimatedValue::new(target, duration);
        } else {
            // Reapply the current policy after reduced motion is switched off,
            // preserving the sampled position when reversing mid-animation.
            let current = self.position.sample(now);
            self.position = AnimatedValue::new(current, duration);
            self.position.set(target, now);
        }
    }

    fn scroll(&mut self, delta: f32) {
        self.remainder -= delta;
        let steps = (self.remainder / STEP).trunc();
        self.remainder -= steps * STEP;
        if steps != 0.0 {
            self.move_to(self.target + steps.clamp(-(COUNT as f32), COUNT as f32));
        }
    }

    #[cfg(test)]
    pub(super) fn settle(&mut self) {
        self.position
            .sample(Instant::now() + Duration::from_secs(1));
    }

    pub(super) fn is_animating(&self) -> bool {
        self.position.is_animating()
    }
}

struct Label(&'static str);
impl Render for Label {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let label = self.0;
        div()
            .debug_selector(move || format!("sidebar-roller-tooltip-{label}"))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(Theme::global().PANEL_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_color(Theme::global().TEXT)
            .text_size(px(12.0))
            .child(self.0)
    }
}

impl Workspace {
    fn scroll_sidebar_roller(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(window.line_height());
        let delta = if delta.x.abs() > delta.y.abs() {
            delta.x
        } else {
            delta.y
        };
        self.sidebar_roller.scroll(f32::from(delta));
        cx.stop_propagation();
        cx.notify();
    }

    fn activate_roller_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_roller.remainder = 0.0;
        self.sidebar_roller
            .move_to(self.sidebar_roller.target + distance(index, self.sidebar_roller.target));
        if let Some(view) = TABS[index].2 {
            self.sidebar_view = view;
            self.sidebar_roller.view = Some(view);
        } else {
            match index {
                6 => self.new_unfinished_work(&NewUnfinishedWork, window, cx),
                7 => self.open_todoist(&OpenTodoist, window, cx),
                8 => self.open_gmail(&OpenGmail, window, cx),
                9 => self.open_folder(&OpenFolder, window, cx),
                10 => {
                    self.missed("new_panel", cx);
                    self.open_new_session(cx);
                }
                _ => unreachable!(),
            }
        }
        cx.notify();
    }

    pub(super) fn render_sidebar_roller(
        &mut self,
        fullscreen: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Restore from the existing sidebar view, including after a hot reload.
        // Merely browsing the wheel never launches an action or changes the page.
        if self.sidebar_roller.view != Some(self.sidebar_view) {
            let index = TABS
                .iter()
                .position(|tab| tab.2 == Some(self.sidebar_view))
                .unwrap_or(0);
            let target = self.sidebar_roller.target + distance(index, self.sidebar_roller.target);
            if self.sidebar_roller.view.is_none() {
                self.sidebar_roller.position =
                    AnimatedValue::new(target, transition::policy(Transition::Focus).duration);
                self.sidebar_roller.target = target;
            } else {
                self.sidebar_roller.move_to(target);
            }
            self.sidebar_roller.view = Some(self.sidebar_view);
        }
        let center = self.sidebar_roller.position.sample(Instant::now());
        let left = sidebar_header_left_padding(fullscreen);
        let available = SIDEBAR_WIDTH - left - 12.0;
        let mut visible = (0..COUNT)
            .filter_map(|index| {
                let offset = distance(index, center);
                (offset.abs() <= 3.3).then_some((index, offset))
            })
            .collect::<Vec<_>>();
        // Far edges paint first so the front folder occludes them naturally.
        visible.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
        let mut roller = div()
            .id("sidebar-navigation-tabs")
            .debug_selector(|| "sidebar-navigation-tabs".into())
            .relative()
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .overflow_hidden()
            .bg(Theme::global().HEADER_BG)
            .on_scroll_wheel(cx.listener(Self::scroll_sidebar_roller))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.0))
                    .bg(Theme::global().PANEL_BORDER),
            );
        for (index, offset) in visible {
            let (id, label, view) = TABS[index];
            let selected = view == Some(self.sidebar_view);
            let g = geometry(offset, available);
            let exposed = if offset.abs() <= 0.5 {
                g.width
            } else {
                let nearer = geometry(offset - offset.signum(), available);
                if offset > 0.0 {
                    (g.left + g.width - nearer.left - nearer.width).clamp(0.0, g.width)
                } else {
                    (nearer.left - g.left).clamp(0.0, g.width)
                }
            };
            roller = roller.child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .absolute()
                    .left(px(left + g.left))
                    .bottom(px(g.bottom))
                    .w(px(g.width))
                    .h(px(g.height))
                    .overflow_hidden()
                    .rounded_t(px(5.0 + 5.0 * g.depth))
                    .border_1()
                    .border_color(if selected {
                        Theme::global().PANEL_BORDER_FOCUS
                    } else {
                        Theme::global().PANEL_BORDER
                    })
                    .when(selected && offset.abs() < 0.02, |el| el.border_b_0())
                    .bg(if selected {
                        Theme::global().HEADER_BG
                    } else {
                        Theme::global().PANEL_BG
                    })
                    .text_color(if selected {
                        Theme::global().TEXT
                    } else if g.depth > 0.7 {
                        Theme::global().TEXT_DIM
                    } else {
                        Theme::global().TEXT_FAINT
                    })
                    .text_size(px(8.0 + 3.0 * g.depth))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(offset > 0.5, |el| el.justify_end())
                    .when(offset < -0.5, |el| el.justify_start())
                    .cursor_pointer()
                    .occlude()
                    .on_scroll_wheel(cx.listener(Self::scroll_sidebar_roller))
                    .hover(|el| {
                        el.bg(Theme::global().HEADER_BG)
                            .text_color(Theme::global().TEXT)
                    })
                    .tooltip(move |_, cx| cx.new(|_| Label(label)).into())
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.activate_roller_tab(index, window, cx);
                        }),
                    )
                    .when(exposed >= 16.0, |el| {
                        el.child(
                            div()
                                .debug_selector(move || format!("sidebar-roller-label-{label}"))
                                .flex_none()
                                .w(px((exposed - 4.0).max(0.0)))
                                .mx(px(2.0))
                                .text_center()
                                .truncate()
                                .child(label),
                        )
                    }),
            );
        }
        // Explicit controls make hidden folders discoverable without a trackpad.
        for (id, label, step) in [
            ("sidebar-roller-previous", "‹", -1.0),
            ("sidebar-roller-next", "›", 1.0),
        ] {
            roller = roller.child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .absolute()
                    .top(px(2.0))
                    .h(px(14.0))
                    .w(px(22.0))
                    .left(px(left
                        + available / 2.0
                        + if step < 0.0 { -26.0 } else { 4.0 }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .text_size(px(14.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .cursor_pointer()
                    .hover(|el| {
                        el.bg(Theme::global().PANEL_BG)
                            .text_color(Theme::global().TEXT)
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.sidebar_roller.remainder = 0.0;
                            this.sidebar_roller
                                .move_to(this.sidebar_roller.target + step);
                            cx.notify();
                        }),
                    )
                    .child(label),
            );
        }
        roller.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roller_motion_reverses_without_jumps_and_resumes_after_reduced_motion() {
        let now = Instant::now();
        let duration = Duration::from_millis(150);
        let mut roller = Roller::default();
        roller.move_to_at(1.0, duration, now);
        assert_eq!(roller.position.sample(now), 0.0);
        let mid_time = now + Duration::from_millis(50);
        let middle = roller.position.sample(mid_time);
        assert!(middle > 0.0 && middle < 1.0);
        roller.move_to_at(0.0, duration, mid_time);
        assert_eq!(roller.position.sample(mid_time), middle);
        assert_eq!(
            roller.position.sample(now + Duration::from_millis(250)),
            0.0
        );
        assert!(!roller.is_animating());
        let restart = now + Duration::from_millis(300);
        roller.move_to_at(3.0, Duration::ZERO, restart);
        assert_eq!(roller.position.sample(restart), 3.0);
        assert!(!roller.is_animating());
        roller.move_to_at(4.0, duration, restart);
        let resumed = roller.position.sample(restart + Duration::from_millis(50));
        assert!(resumed > 3.0 && resumed < 4.0);
        assert!(roller.is_animating());
    }

    #[gpui::test]
    fn roller_exposed_labels_do_not_overlap_and_thin_tabs_keep_full_tooltips(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_, vcx) = cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        vcx.run_until_parked();
        let selected = vcx.debug_bounds("sidebar-sessions-tab").unwrap();
        let label = vcx.debug_bounds("sidebar-roller-label-learn").unwrap();
        let tab = vcx.debug_bounds("sidebar-learn-tab").unwrap();
        assert!(label.left() >= selected.right());
        assert!(label.right() <= tab.right());
        assert!(label.top() >= tab.top() && label.bottom() <= tab.bottom());
        let thin = vcx.debug_bounds("sidebar-accounts-tab").unwrap();
        assert!(thin.size.width < px(16.0));
        assert!(vcx.debug_bounds("sidebar-roller-label-accounts").is_none());
        vcx.update(|window, cx| window.simulate_mouse_move(thin.center(), cx));
        vcx.run_until_parked();
        vcx.executor().advance_clock(Duration::from_secs(1));
        vcx.run_until_parked();
        let tooltip = vcx.debug_bounds("sidebar-roller-tooltip-accounts").unwrap();
        assert!(tooltip.size.width > thin.size.width);
    }

    #[gpui::test]
    fn roller_restores_selected_page_and_hidden_motion_does_not_schedule_frames(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.sidebar_view = SidebarView::Settings;
            w
        });
        vcx.run_until_parked();
        let snapshot = workspace.update_in(vcx, |w, window, cx| {
            w.sidebar_roller
                .move_to_at(6.0, Duration::from_secs(1), Instant::now());
            assert!(w.animation_active());
            w.show_sidebar = false;
            assert!(!w.animation_active());
            w.show_sidebar = true;
            w.layout_mode = crate::config::LayoutMode::Normal;
            assert!(!w.animation_active());
            w.layout_mode = crate::config::LayoutMode::FolderTabs;
            w.snapshot(window, cx).unwrap()
        });
        workspace.update(vcx, |w, cx| {
            w.sidebar_roller = Roller::default();
            w.apply_snapshot(snapshot, cx);
            cx.notify();
        });
        vcx.run_until_parked();
        let restored = vcx.debug_bounds("sidebar-settings-tab").unwrap();
        assert!((f32::from(restored.center().x) - SIDEBAR_WIDTH / 2.0).abs() < 1.0);
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.sidebar_view, SidebarView::Settings);
            assert!(!w.sidebar_roller.is_animating());
        });
    }

    #[gpui::test]
    fn roller_browses_without_launching_and_clicks_each_sidebar_page(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        vcx.run_until_parked();
        let tabs = vcx.debug_bounds("sidebar-navigation-tabs").unwrap();
        for (dx, dy) in [
            (-48.0, 0.0),
            (0.0, -96.0),
            (-384.0, -1.0),
            (48.0, 1.0),
            (0.0, 480.0),
        ] {
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: tabs.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(dx), px(dy))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            workspace.update(vcx, |w, cx| {
                w.sidebar_roller.settle();
                assert_eq!(w.sidebar_view, SidebarView::Sessions);
                assert!(
                    w.slots.is_empty(),
                    "rolling over actions must not open a panel"
                );
                cx.notify();
            });
            vcx.run_until_parked();
        }
        for (index, (selector, _, view)) in TABS.iter().enumerate().take(6) {
            if index > 0 {
                let next = vcx.debug_bounds("sidebar-roller-next").unwrap();
                vcx.simulate_click(next.center(), gpui::Modifiers::default());
                workspace.update(vcx, |w, cx| {
                    w.sidebar_roller.settle();
                    cx.notify();
                });
                vcx.run_until_parked();
            }
            let tab = vcx.debug_bounds(*selector).unwrap();
            assert!((f32::from(tab.center().x) - SIDEBAR_WIDTH / 2.0).abs() < 1.0);
            vcx.simulate_click(tab.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            workspace.read_with(vcx, |w, _| assert_eq!(Some(w.sidebar_view), *view));
        }
    }

    #[gpui::test]
    fn roller_folder_and_new_session_clicks_preserve_their_action_contracts(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(move |_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.bridge = bridge;
            w
        });
        vcx.run_until_parked();
        for _ in 0..2 {
            let previous = vcx.debug_bounds("sidebar-roller-previous").unwrap();
            vcx.simulate_click(previous.center(), gpui::Modifiers::default());
            workspace.update(vcx, |w, cx| {
                w.sidebar_roller.settle();
                cx.notify();
            });
            vcx.run_until_parked();
        }
        assert!(
            commands.try_recv().is_err(),
            "browsing must not create a session"
        );
        let folder = vcx.debug_bounds("sidebar-open-folder").unwrap();
        vcx.simulate_click(folder.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("folder-picker-overlay").is_some());
        assert!(
            !vcx.did_prompt_for_paths(),
            "folder action stays in the app"
        );
        let cancel = vcx.debug_bounds("folder-picker-cancel").unwrap();
        vcx.simulate_click(cancel.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
        assert!(commands.try_recv().is_err());
        let next = vcx.debug_bounds("sidebar-roller-next").unwrap();
        vcx.simulate_click(next.center(), gpui::Modifiers::default());
        workspace.update(vcx, |w, cx| {
            w.sidebar_roller.settle();
            cx.notify();
        });
        vcx.run_until_parked();
        let new = vcx.debug_bounds("sidebar-new-session").unwrap();
        vcx.simulate_click(new.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::CreateSession {
                request_id: None,
                ..
            })
        ));
        assert!(
            commands.try_recv().is_err(),
            "one click emits exactly one command"
        );
    }

    #[test]
    fn roller_projection_curves_and_compresses_symmetrically() {
        for available in [156.0, 240.0] {
            let center = geometry(0.0, available);
            assert_eq!(center.bottom, 0.0);
            assert_eq!(center.height, 34.0);
            for offset in [0.5, 1.0, 2.0, 3.0, 3.3] {
                let left = geometry(-offset, available);
                let right = geometry(offset, available);
                assert!((left.left - (available - right.left - right.width)).abs() < 0.001);
                assert!(left.width < center.width && left.bottom > 0.0);
                assert!(left.left >= 0.0 && right.left + right.width <= available);
            }
        }
    }

    #[test]
    fn roller_wraps_and_accumulates_small_scrolls_without_activating() {
        let mut roller = Roller::default();
        roller.scroll(-20.0);
        roller.scroll(-20.0);
        assert_eq!(roller.target, 0.0);
        roller.scroll(-8.0);
        assert_eq!(roller.target, 1.0);
        roller.scroll(96.0);
        assert_eq!(roller.target, -1.0);
        assert_eq!(distance(10, roller.target), 0.0);
        assert_eq!(distance(0, 10.0), 1.0);
        assert_eq!(roller.view, None);
    }
}
