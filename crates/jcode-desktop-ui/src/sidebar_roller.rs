//! A section menu for the top-left navigation, not the live sessions.
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

// Retain the internal name so workspace state wiring stays compatible.
#[derive(Default)]
pub(super) struct Roller {
    target: f32,
    remainder: f32,
    view: Option<SidebarView>,
    expanded: bool,
}

impl Roller {
    fn move_to(&mut self, target: f32) {
        self.target = target;
    }

    fn scroll(&mut self, delta: f32) {
        self.remainder -= delta;
        let steps = (self.remainder / STEP).trunc();
        self.remainder -= steps * STEP;
        if steps != 0.0 {
            self.move_to(self.target + steps.clamp(-(COUNT as f32), COUNT as f32));
        }
    }
}

impl Workspace {
    fn hover_sidebar_roller(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A pointer move, not a repaint during startup/reload or a drag.
        if event.pressed_button.is_none() && !self.sidebar_roller.expanded {
            self.sidebar_roller.expanded = true;
            self.sidebar_roller.remainder = 0.0;
            cx.notify();
        }
    }

    fn scroll_sidebar_roller(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // One wheel notch is one option. Precise touchpads accumulate smaller
        // movements instead of racing through the choices on every event.
        let delta = match event.delta {
            gpui::ScrollDelta::Lines(delta) => {
                let delta = if delta.x.abs() > delta.y.abs() {
                    delta.x
                } else {
                    delta.y
                };
                delta.signum() * STEP
            }
            gpui::ScrollDelta::Pixels(delta) => f32::from(if delta.x.abs() > delta.y.abs() {
                delta.x
            } else {
                delta.y
            }),
        };
        let previous = self.sidebar_roller.target;
        self.sidebar_roller.scroll(delta);
        if self.sidebar_roller.target != previous {
            self.preview_roller_tab();
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn preview_roller_tab(&mut self) {
        let index = self.sidebar_roller.target.rem_euclid(COUNT as f32) as usize;
        // Browsing pages is live, but passing an action must never launch it.
        if let Some(view) = TABS[index].2 {
            self.sidebar_view = view;
            self.sidebar_roller.view = Some(view);
        }
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
        // Scrolling previews pages live, but never launches an action.
        if self.sidebar_roller.view != Some(self.sidebar_view) {
            let index = TABS
                .iter()
                .position(|tab| tab.2 == Some(self.sidebar_view))
                .unwrap_or(0);
            let target = self.sidebar_roller.target + distance(index, self.sidebar_roller.target);
            self.sidebar_roller.move_to(target);
            self.sidebar_roller.view = Some(self.sidebar_view);
        }
        // Keep the existing scroll selection semantics, without projecting tabs
        // around a cylinder. The current page stays legible while browsing actions.
        let (id, label, _) = TABS
            .iter()
            .copied()
            .find(|tab| tab.2 == Some(self.sidebar_view))
            .unwrap_or(TABS[0]);
        let menu = div()
            .id("sidebar-navigation-tabs")
            .debug_selector(|| "sidebar-navigation-tabs".into())
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .pl(px(sidebar_header_left_padding(fullscreen)))
            .pr(px(12.0))
            .py(px(6.0))
            .bg(Theme::global().HEADER_BG)
            .on_mouse_move(cx.listener(Self::hover_sidebar_roller))
            .on_scroll_wheel(cx.listener(Self::scroll_sidebar_roller))
            .child(
                div()
                    .id("sidebar-section-trigger")
                    .debug_selector(|| "sidebar-section-trigger".into())
                    .size_full()
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(Theme::global().PANEL_BORDER)
                    .bg(if self.sidebar_roller.expanded {
                        Theme::global().PANEL_BG
                    } else {
                        Theme::global().HEADER_BG
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .relative()
                    .text_size(px(12.0))
                    .text_color(Theme::global().TEXT)
                    .cursor_pointer()
                    .hover(|el| el.bg(Theme::global().PANEL_BG))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.sidebar_roller.expanded = true;
                            cx.notify();
                        }),
                    )
                    .child(div().debug_selector(move || id.into()).child(label))
                    .child(
                        div()
                            .debug_selector(|| "sidebar-section-chevron".into())
                            .absolute()
                            .right(px(8.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(if self.sidebar_roller.expanded {
                                "⌃"
                            } else {
                                "⌄"
                            }),
                    ),
            );
        div()
            .relative()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .child(menu)
            .when(self.sidebar_roller.expanded, |el| {
                // Deferred paint keeps the pop-out above the sidebar body
                // without moving it or changing the workspace's geometry.
                el.child(gpui::deferred(self.render_roller_popout(fullscreen, cx)))
            })
            .into_any_element()
    }

    fn render_roller_popout(&self, _fullscreen: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut pages = div().flex().flex_col().gap_1();
        for pair in (0..6).collect::<Vec<_>>().chunks(2) {
            let mut row = div().flex().gap_1();
            for &index in pair {
                let (id, label, view) = TABS[index];
                let selected = view == Some(self.sidebar_view);
                row = row.child(
                    div()
                        .id(format!("{id}-expanded"))
                        .debug_selector(move || format!("{id}-expanded"))
                        .flex_1()
                        .min_w_0()
                        .h(px(32.0))
                        .px_2()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .bg(if selected {
                            Theme::global().ACCENT_DIM
                        } else {
                            Theme::global().PANEL_BG
                        })
                        .text_color(if selected {
                            Theme::global().TEXT
                        } else {
                            Theme::global().TEXT_DIM
                        })
                        .hover(|el| {
                            el.bg(Theme::global().HEADER_BG)
                                .text_color(Theme::global().TEXT)
                        })
                        .child(
                            div()
                                .w(px(8.0))
                                .text_color(Theme::global().ACCENT)
                                .child(if selected { "•" } else { "" }),
                        )
                        .child(label)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                this.activate_roller_tab(index, window, cx);
                            }),
                        ),
                );
            }
            pages = pages.child(row);
        }
        let target = self.sidebar_roller.target.rem_euclid(COUNT as f32) as usize;
        let mut actions = div().flex().flex_wrap().gap_1();
        for (index, &(id, label, _)) in TABS.iter().enumerate().skip(6) {
            actions = actions.child(
                div()
                    .id(format!("{id}-expanded"))
                    .debug_selector(move || format!("{id}-expanded"))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(Theme::global().TEXT_DIM)
                    .when(index == target, |el| el.bg(Theme::global().ACCENT_DIM))
                    .hover(|el| {
                        el.bg(Theme::global().HEADER_BG)
                            .text_color(Theme::global().TEXT)
                    })
                    .child(if label == "+" { "new chat" } else { label })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.sidebar_roller.expanded = false;
                            this.activate_roller_tab(index, window, cx);
                        }),
                    ),
            );
        }
        div()
            .id("sidebar-roller-popout")
            .debug_selector(|| "sidebar-roller-popout".into())
            .absolute()
            .top_0()
            .left_0()
            .w(px(SIDEBAR_WIDTH))
            // Include the original header in the hit area: crossing from the
            // collapsed control to an option never dismisses the pop-out.
            .pt(px(TITLEBAR_HEIGHT))
            .px_2()
            .pb_2()
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.sidebar_roller.expanded {
                    this.sidebar_roller.expanded = false;
                    this.sidebar_roller.remainder = 0.0;
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(Self::scroll_sidebar_roller))
            .child(
                div()
                    .debug_selector(|| "sidebar-section-menu".into())
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(Theme::global().PANEL_BORDER)
                    .shadow_lg()
                    .bg(Theme::global().PANEL_BG)
                    .text_size(px(12.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child("Sidebar")
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(Theme::global().TEXT_DIM)
                                    .child("Scroll to switch"),
                            ),
                    )
                    .child(pages)
                    .child(
                        div()
                            .border_t_1()
                            .border_color(Theme::global().PANEL_BORDER)
                            .pt_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(Theme::global().TEXT_FAINT)
                                    .child("Actions · click to open"),
                            )
                            .child(actions),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn roller_popout_scrolls_live_and_leaves_layout_focus_and_actions_alone(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(move |_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.bridge = bridge;
            w.push_test_panel("Keep this chat", cx);
            w
        });
        vcx.run_until_parked();
        let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
        let body = vcx.debug_bounds("sidebar-tab-body").unwrap();
        let header = vcx.debug_bounds("sidebar-navigation-tabs").unwrap();
        vcx.update(|window, cx| window.simulate_mouse_move(header.center(), cx));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-roller-popout").is_some());
        let option = vcx.debug_bounds("sidebar-files-tab-expanded").unwrap();
        vcx.update(|window, cx| window.simulate_mouse_move(option.center(), cx));
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("sidebar-roller-popout").is_some(),
            "no gap when entering options"
        );
        let pages = [
            SidebarView::Learn,
            SidebarView::Files,
            SidebarView::Accounts,
            SidebarView::Theme,
            SidebarView::Settings,
        ];
        for expected in pages
            .into_iter()
            .chain(std::iter::repeat_n(SidebarView::Settings, 5))
            .chain([SidebarView::Sessions])
        {
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: option.center(),
                delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -1.0)),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            vcx.run_until_parked();
            workspace.read_with(vcx, |w, _| {
                assert_eq!(w.sidebar_view, expected);
                assert_eq!(w.slots.len(), 1);
                assert_eq!(w.active, 0);
                assert_eq!(w.active_row, 0);
            });
            assert_eq!(vcx.debug_bounds("workspace-canvas").unwrap(), canvas);
            let current_body = vcx.debug_bounds("sidebar-tab-body").unwrap();
            assert_eq!(current_body.bottom(), body.bottom());
            assert_eq!(current_body.size.width, body.size.width);
            if expected == SidebarView::Sessions {
                assert_eq!(current_body, body);
                assert!(vcx.debug_bounds("sidebar-workflow-switch").is_some());
            } else {
                assert!(
                    current_body.top() < body.top(),
                    "non-chat sections reclaim the workflow switch space"
                );
                assert!(vcx.debug_bounds("sidebar-workflow-switch").is_none());
            }
            assert!(
                commands.try_recv().is_err(),
                "wheel must never dispatch an action"
            );
        }
        let theme = vcx.debug_bounds("sidebar-theme-tab-expanded").unwrap();
        vcx.simulate_click(theme.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("theme-settings").is_some());
        vcx.update(|window, cx| window.simulate_mouse_move(canvas.center(), cx));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-roller-popout").is_none());
        assert!(
            vcx.debug_bounds("theme-settings").is_some(),
            "leaving keeps the live selection"
        );
    }

    #[gpui::test]
    fn section_trigger_stays_visible_above_menu_and_hover_reveals_all_options(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_, vcx) = cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        vcx.run_until_parked();
        let trigger = vcx.debug_bounds("sidebar-section-trigger").unwrap();
        let label = vcx.debug_bounds("sidebar-sessions-tab").unwrap();
        let chevron = vcx.debug_bounds("sidebar-section-chevron").unwrap();
        assert!(label.left() >= trigger.left());
        assert_eq!(
            label.center().x, trigger.center().x,
            "section title is centered independently of the chevron"
        );
        assert!(label.right() < chevron.left());
        assert!(chevron.right() <= trigger.right());
        assert!(vcx.debug_bounds("sidebar-learn-tab").is_none());
        assert!(vcx.debug_bounds("sidebar-roller-previous").is_none());
        assert!(vcx.debug_bounds("sidebar-roller-next").is_none());
        vcx.update(|window, cx| window.simulate_mouse_move(trigger.center(), cx));
        vcx.run_until_parked();
        assert_eq!(
            vcx.debug_bounds("sidebar-section-trigger").unwrap(),
            trigger
        );
        assert_eq!(
            vcx.debug_bounds("sidebar-sessions-tab").unwrap().center().x,
            trigger.center().x
        );
        let menu = vcx.debug_bounds("sidebar-section-menu").unwrap();
        assert!(
            menu.top() >= trigger.bottom(),
            "menu must not replace the trigger"
        );
        for (id, _, _) in TABS {
            assert!(
                vcx.debug_bounds(Box::leak(format!("{id}-expanded").into_boxed_str()))
                    .is_some()
            );
        }
    }

    #[gpui::test]
    fn section_menu_restores_selected_page_without_scheduling_motion(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.sidebar_view = SidebarView::Settings;
            w
        });
        vcx.run_until_parked();
        let snapshot = workspace.update_in(vcx, |w, window, cx| {
            w.sidebar_roller.move_to(6.0);
            assert!(!w.animation_active());
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
        let trigger = vcx.debug_bounds("sidebar-section-trigger").unwrap();
        assert!(restored.left() >= trigger.left() && restored.right() <= trigger.right());
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.sidebar_view, SidebarView::Settings);
            assert_eq!(w.sidebar_roller.target, 5.0);
        });
    }

    #[gpui::test]
    fn roller_scrolls_pages_live_without_launching_actions_and_clicks_each_page(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        vcx.run_until_parked();
        let tabs = vcx.debug_bounds("sidebar-navigation-tabs").unwrap();
        for (dx, dy, expected) in [
            (-48.0, 0.0, SidebarView::Learn),
            (0.0, -96.0, SidebarView::Accounts),
            (-384.0, -1.0, SidebarView::Sessions),
            (48.0, 1.0, SidebarView::Sessions),
            (0.0, 480.0, SidebarView::Sessions),
        ] {
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: tabs.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(dx), px(dy))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            workspace.update(vcx, |w, cx| {
                assert_eq!(w.sidebar_view, expected);
                assert!(
                    w.slots.is_empty(),
                    "rolling over actions must not open a panel"
                );
                cx.notify();
            });
            vcx.run_until_parked();
        }
        vcx.update(|window, cx| window.simulate_mouse_move(tabs.center(), cx));
        vcx.run_until_parked();
        for (selector, _, view) in TABS.iter().take(6) {
            let selector = Box::leak(format!("{selector}-expanded").into_boxed_str());
            let tab = vcx.debug_bounds(selector).unwrap();
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
        let header = vcx.debug_bounds("sidebar-navigation-tabs").unwrap();
        vcx.update(|window, cx| window.simulate_mouse_move(header.center(), cx));
        vcx.run_until_parked();
        assert!(
            commands.try_recv().is_err(),
            "browsing must not create a session"
        );
        let folder = vcx.debug_bounds("sidebar-open-folder-expanded").unwrap();
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
        vcx.update(|window, cx| window.simulate_mouse_move(header.center(), cx));
        vcx.run_until_parked();
        let new = vcx.debug_bounds("sidebar-new-session-expanded").unwrap();
        vcx.simulate_click(new.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let Ok(Command::CreateSession {
            request_id: Some(request_id),
            ..
        }) = commands.try_recv()
        else {
            panic!("new local sessions must correlate their immediately opened draft");
        };
        assert!(crate::panel::Panel::is_pending_session_id(&request_id));
        assert!(workspace.read_with(vcx, |w, cx| {
            w.test_panel(0)
                .is_some_and(|panel| panel.read(cx).session_id == request_id)
        }));
        assert!(
            commands.try_recv().is_err(),
            "one click emits exactly one command"
        );
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
