//! Labeled live-session folder tabs, independent of the sidebar's navigation.
use super::*;
use crate::panel::folder_session_title;

/// Center a joined stack independently of the panel camera. Selection adds
/// only a 12px width accent and a 4px lift, so the panels do the large slide.
#[derive(Clone, Copy, Debug)]
struct TabLayout {
    start: f32,
    active_width: f32,
    inactive_width: f32,
    step: f32,
    right_step: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TabGeometry {
    left: f32,
    width: f32,
    height: f32,
}

impl TabLayout {
    fn new(available: f32, count: usize) -> Self {
        let available = available.max(0.0);
        let count = count.max(1);
        let active_width = if count == 1 {
            available.min(208.0)
        } else {
            (available * 0.55).min(208.0)
        }
        .floor();
        let inactive_width = (active_width - 12.0).max(active_width * 0.9).floor();
        let step = if count == 1 {
            0.0
        } else {
            ((available - active_width) / (count - 1) as f32)
                .min(inactive_width - 12.0_f32.min(inactive_width * 0.1))
        };
        let used = active_width + step * (count - 1) as f32;
        Self {
            start: (available - used) / 2.0,
            active_width,
            inactive_width,
            step,
            right_step: step,
        }
    }

    fn geometry(self, position: usize, selected: usize) -> TabGeometry {
        TabGeometry {
            left: self.start
                + self.step * position.min(selected) as f32
                + if position > selected {
                    self.active_width - self.inactive_width
                        + self.right_step * (position - selected) as f32
                } else {
                    0.0
                },
            width: if position == selected {
                self.active_width
            } else {
                self.inactive_width
            },
            height: if position == selected {
                FOLDER_CONTENT_INSET
            } else {
                FOLDER_CONTENT_INSET - 4.0
            },
        }
    }

    fn paint_order(count: usize, selected: usize) -> impl Iterator<Item = usize> {
        (0..selected)
            .chain((selected + 1..count).rev())
            .chain(std::iter::once(selected))
    }

    /// Keep labels, status dots, and click targets out of the overlapping lip.
    fn exposed(tabs: &[TabGeometry], position: usize, selected: usize) -> (f32, f32) {
        let tab = tabs[position];
        let mut left = tab.left;
        let mut right = tab.left + tab.width;
        for other in Self::paint_order(tabs.len(), selected)
            .skip_while(|&i| i != position)
            .skip(1)
            .map(|i| tabs[i])
        {
            if other.left >= right || other.left + other.width <= left {
                continue;
            }
            if other.left > left {
                right = right.min(other.left);
            } else {
                left = left.max(other.left + other.width).min(right);
            }
        }
        (left, (right - left).max(0.0))
    }
}

struct TabTween {
    target: TabGeometry,
    left: AnimatedValue,
    width: AnimatedValue,
    height: AnimatedValue,
}

impl TabTween {
    fn new(target: TabGeometry, duration: Duration) -> Self {
        Self {
            target,
            left: AnimatedValue::new(target.left, duration),
            width: AnimatedValue::new(target.width, duration),
            height: AnimatedValue::new(target.height, duration),
        }
    }

    fn sample(&mut self, now: Instant) -> TabGeometry {
        TabGeometry {
            left: self.left.sample(now),
            width: self.width.sample(now),
            height: self.height.sample(now),
        }
    }
}

#[derive(Default)]
pub(super) struct TabMotion {
    tabs: HashMap<u64, TabTween>,
    available: Option<f32>,
    pub(super) hit_targets: Vec<(usize, f32)>,
}

impl TabMotion {
    fn sample(
        &mut self,
        targets: &[(u64, TabGeometry)],
        available: f32,
        duration: Duration,
        now: Instant,
    ) -> Vec<TabGeometry> {
        // A window resize must not leave animated tabs outside the new bounds.
        let snap = self.available != Some(available) || duration.is_zero();
        self.available = Some(available);
        let keys: HashSet<_> = targets.iter().map(|(key, _)| *key).collect();
        self.tabs.retain(|key, _| keys.contains(key));
        targets
            .iter()
            .map(|(key, target)| {
                let tween = self
                    .tabs
                    .entry(*key)
                    .or_insert_with(|| TabTween::new(*target, duration));
                if snap {
                    *tween = TabTween::new(*target, duration);
                } else if tween.target != *target {
                    tween.left.set(target.left, now);
                    tween.width.set(target.width, now);
                    tween.height.set(target.height, now);
                    tween.target = *target;
                }
                tween.sample(now)
            })
            .collect()
    }

    pub(super) fn is_animating(&self) -> bool {
        self.tabs.values().any(|tab| {
            tab.left.is_animating() || tab.width.is_animating() || tab.height.is_animating()
        })
    }

    #[cfg(test)]
    fn settle(&mut self) {
        for tab in self.tabs.values_mut() {
            tab.sample(Instant::now() + Duration::from_secs(1));
        }
    }
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
        let mut entries = Vec::new();
        for row in 0..STRIP_COUNT {
            let indices: Vec<_> = self.row_indices(row).collect();
            if indices.is_empty() && row == self.active_row {
                entries.push((None, row, 0));
            }
            for (row_position, index) in indices.into_iter().enumerate() {
                entries.push((Some(index), row, row_position));
            }
        }
        let selected = entries
            .iter()
            .position(|(index, row, _)| {
                *row == self.active_row && index.is_none_or(|index| index == self.active)
            })
            .unwrap_or(0);
        let available = (canvas_width - right).max(0.0);
        let layout = TabLayout::new(available, entries.len());
        let targets: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(position, (index, row, _))| {
                let key = index
                    .map(|index| self.slots[index].panel.entity_id().as_u64())
                    .unwrap_or(u64::MAX - *row as u64);
                (key, layout.geometry(position, selected))
            })
            .collect();
        // Never chase camera coordinates or clipped-panel widths. Focus only
        // nudges tab geometry, with its own short, continuously retargeted tween.
        let geometry = self.live_tabs.sample(
            &targets,
            available,
            transition::policy(Transition::Focus).duration,
            Instant::now(),
        );
        let mut tabs = div()
            .id("live-session-tabs")
            .debug_selector(|| "live-session-tabs".into())
            .absolute()
            .top(px(STRIP_PADDING_Y))
            .left_0()
            .right(px(right))
            .h(px(FOLDER_CONTENT_INSET));
        let populated_rows = (0..STRIP_COUNT)
            .filter(|row| self.row_indices(*row).next().is_some())
            .count();
        self.live_tabs.hit_targets.clear();
        for position in TabLayout::paint_order(entries.len(), selected) {
            let (index, row, row_position) = entries[position];
            let focused = position == selected;
            let current = geometry[position];
            let (exposed_left, visible) = TabLayout::exposed(&geometry, position, selected);
            if let Some(index) = index {
                let x = exposed_left + visible / 2.0;
                self.live_tabs.hit_targets.push((index, x));
            }
            let padding = (visible / 12.0).min(6.0);
            let (title, emoji, activity, state) = match index {
                Some(index) => {
                    let panel = self.slots[index].panel.read(cx);
                    (
                        folder_session_title(&panel.session_id, panel.title.as_ref()),
                        jcode_core::id::extract_session_name(&panel.session_id)
                            .map(jcode_core::id::session_icon)
                            .unwrap_or("💫"),
                        panel.tab_activity(),
                        Some(panel.minimap_state()),
                    )
                }
                None => (format!("Workspace {}", row + 1).into(), "📁", None, None),
            };
            let text = div()
                .absolute()
                .left(px(exposed_left - current.left))
                .flex_none()
                .w(px((visible - 2.0).max(0.0)))
                .min_w_0()
                .overflow_hidden()
                .px(px(padding))
                .h_full()
                .flex()
                .items_center()
                .gap(px((visible / 12.0).min(4.0)))
                .child(
                    div()
                        .debug_selector(move || match index {
                            Some(index) => format!("live-session-tab-{index}-emoji"),
                            None => "live-session-empty-tab-emoji".into(),
                        })
                        .flex_none()
                        .size(px((visible - 2.0 - 2.0 * padding).clamp(1.0, 20.0)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px((visible - 2.0 - 2.0 * padding).clamp(1.0, 14.0)))
                        .child(match activity {
                            Some(activity) => div()
                                .debug_selector(move || {
                                    format!("live-session-tab-{}-working-emoji", index.unwrap())
                                })
                                .size_full()
                                .child(activity)
                                .into_any_element(),
                            None => div().child(emoji).into_any_element(),
                        }),
                )
                .when(
                    visible >= 64.0 && populated_rows > 1 && row_position == 0,
                    |el| {
                        el.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(Theme::global().TEXT_FAINT)
                                .child(format!("{}", row + 1)),
                        )
                    },
                )
                .when(focused || visible >= 52.0, |el| {
                    el.child(
                        div()
                            .debug_selector(move || match index {
                                Some(index) => format!("live-session-tab-{index}-title"),
                                None => "live-session-empty-tab-title".into(),
                            })
                            .min_w_0()
                            .truncate()
                            .child(title.clone()),
                    )
                });
            tabs = tabs.child(
                div()
                    .id(("workspace-session", index.unwrap_or(usize::MAX)))
                    .debug_selector(move || match index {
                        Some(index) => format!("live-session-tab-{index}"),
                        None => "live-session-empty-tab".into(),
                    })
                    .absolute()
                    .left(px(current.left))
                    .bottom_0()
                    .w(px(current.width))
                    .min_w_0()
                    .overflow_hidden()
                    .h(px(current.height))
                    .flex()
                    .items_center()
                    .rounded_t_md()
                    // Crowded off-screen tabs can be narrower than two pixels.
                    // Their border must not force the layout wider than its slot.
                    .border(px((current.width / 2.0).min(1.0)))
                    .border_b_0()
                    .border_color(if focused {
                        Theme::global().PANEL_BORDER_FOCUS.opacity(0.50)
                    } else {
                        Theme::global().PANEL_BORDER.opacity(0.45)
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
                    .occlude()
                    .cursor_pointer()
                    .tooltip({
                        let title: gpui::SharedString = title.into();
                        move |_, cx| cx.new(|_| TabTooltip(title.clone())).into()
                    })
                    .hover(|el| {
                        el.bg(Theme::global().PANEL_BG)
                            .text_color(Theme::global().TEXT)
                    })
                    .when(focused && index.is_some(), |el| {
                        el.child(div().absolute().inset_0().debug_selector(move || {
                            format!("live-session-tab-{}-focused", index.unwrap())
                        }))
                    })
                    .when_some(index.zip(state), |el, (index, state)| {
                        let (state_name, state_color) = match state {
                            crate::panel::MinimapSessionState::Idle => {
                                ("idle", Theme::global().TEXT_FAINT)
                            }
                            crate::panel::MinimapSessionState::Working => {
                                ("working", Theme::global().WARN)
                            }
                            crate::panel::MinimapSessionState::Streaming => {
                                ("streaming", Theme::global().ACCENT)
                            }
                            crate::panel::MinimapSessionState::Complete => {
                                ("complete", Theme::global().OK)
                            }
                            crate::panel::MinimapSessionState::Error => {
                                ("error", Theme::global().ERROR)
                            }
                        };
                        el.child(
                            div()
                                .absolute()
                                .right(px(
                                    (current.left + current.width - exposed_left - visible) + 4.0
                                ))
                                .top_1()
                                .size(px(5.0))
                                .rounded_full()
                                .bg(state_color)
                                .debug_selector(move || {
                                    format!("live-session-tab-{index}-{state_name}")
                                }),
                        )
                    })
                    .child(text)
                    .when_some(index, |el, index| {
                        el.on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                window.prevent_default();
                                this.set_active(index, cx);
                                this.overview = false;
                                this.overview_progress.set(0.0, Instant::now());
                                this.focus_active(window, cx);
                                cx.notify();
                            }),
                        )
                    }),
            );
        }
        tabs.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn live_tabs_animate_the_working_emoji_without_shifting_the_title(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.show_sidebar = false;
            workspace.push_test_panel("activity-tab", cx);
            workspace.push_test_panel("quiet-tab", cx);
            workspace
        });
        vcx.run_until_parked();
        let idle_title_left = vcx.debug_bounds("live-session-tab-0-title").unwrap().left();
        for (status, active) in [
            ("running", true),
            ("thinking", true),
            ("generating", true),
            ("streaming", true),
            ("running_tools", true),
            ("busy", true),
            ("idle", false),
            ("attached", false),
            ("connected", false),
            ("lost: disconnected", false),
            ("error", false),
            ("crashed", false),
        ] {
            workspace.update(vcx, |workspace, cx| {
                workspace.apply(
                    Update::Event {
                        session_id: "activity-tab".into(),
                        event: jcode_sdk::ApiEvent::SessionStatus {
                            session_id: "activity-tab".into(),
                            status: status.into(),
                        },
                    },
                    cx,
                );
                cx.notify();
            });
            vcx.run_until_parked();
            assert_eq!(
                vcx.debug_bounds("live-session-tab-0-working-emoji")
                    .is_some(),
                active,
                "status {status}",
            );
            assert!(vcx.debug_bounds("live-session-tab-1-spinner").is_none());
            assert!(vcx.debug_bounds("panel-session-title").is_none());
            assert!(vcx.debug_bounds("panel-activity-label").is_none());
            let title = vcx.debug_bounds("live-session-tab-0-title").unwrap();
            assert!(vcx.debug_bounds("live-session-tab-0-spinner").is_none());
            assert!(
                vcx.debug_bounds("live-session-tab-1-working-emoji")
                    .is_none()
            );
            let emoji = vcx.debug_bounds("live-session-tab-0-emoji").unwrap();
            assert!(emoji.right() <= title.left());
            assert_eq!(
                title.left(),
                idle_title_left,
                "activity must not shift the title"
            );
        }
    }

    #[test]
    fn live_tabs_overlap_preserves_folder_width_and_centers_the_stack() {
        for available in [0.0, 40.0, 180.0, 352.0, 800.0, 2400.0] {
            for count in 1..=200 {
                let layout = TabLayout::new(available, count);
                for selected in [0, count / 2, count - 1] {
                    let first = layout.geometry(0, selected);
                    let last = layout.geometry(count - 1, selected);
                    assert!(first.left >= 0.0);
                    assert!(last.left + last.width <= available + 0.001);
                    assert!((first.left - (available - last.left - last.width)).abs() < 0.001);
                    let active = layout.geometry(selected, selected);
                    assert!(active.width >= layout.inactive_width);
                    assert_eq!(
                        TabLayout::paint_order(count, selected).last(),
                        Some(selected)
                    );
                    if available > 0.0 && count > 1 {
                        assert!(layout.step > 0.0);
                    }
                }
            }
        }
        let crowded = TabLayout::new(512.0, 12);
        assert_eq!(crowded.active_width, 208.0);
        assert_eq!(crowded.inactive_width, 196.0);
        assert!(crowded.step < crowded.inactive_width);
        assert!(TabLayout::new(1152.0, 3).start > 0.0);
    }

    #[test]
    fn live_tabs_center_two_and_four_panel_groups_with_subtle_focus_motion() {
        for available in [40.0, 192.0, 512.0, 1152.0, 1600.0] {
            for count in [2, 4, 6, 12, 200] {
                let layout = TabLayout::new(available, count);
                for selected in 0..count {
                    let tabs: Vec<_> = (0..count).map(|i| layout.geometry(i, selected)).collect();
                    let first = tabs[0];
                    let last = tabs[count - 1];
                    assert!((first.left + last.left + last.width - available).abs() < 0.001);
                    for (i, tab) in tabs.iter().enumerate() {
                        assert!(tab.left >= 0.0);
                        assert!(tab.left + tab.width <= available + 0.001);
                        assert!(TabLayout::exposed(&tabs, i, selected).1 > 0.0);
                        let next = layout.geometry(i, (selected + 1) % count);
                        assert!((next.left - tab.left).abs() <= 12.001);
                        assert!((next.width - tab.width).abs() <= 12.001);
                        assert!((next.height - tab.height).abs() <= 4.001);
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn live_tabs_two_panel_cluster_keeps_every_offscreen_tab_visible_and_clickable(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_sidebar = false;
            for i in 0..8 {
                w.push_test_panel(&format!("session-{i}"), cx);
                w.slots[i].width_fraction = 0.5;
                w.slots[i].animated_width = AnimatedValue::new(0.5, Duration::ZERO);
            }
            w
        });
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [1440.0, 1000.0, 640.0] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(700.0)));
            vcx.run_until_parked();
            for selected in [3, 4, 0, 7, 1, 6, 2, 5] {
                workspace.update(vcx, |w, cx| {
                    w.camera_x[w.active_row] = w.camera_target[w.active_row];
                    w.camera_started[w.active_row] = None;
                    w.live_tabs.settle();
                    cx.notify();
                });
                vcx.run_until_parked();
                let track = vcx.debug_bounds("live-session-tabs").unwrap();
                let targets = workspace.read_with(vcx, |w, _| w.live_tabs.hit_targets.clone());
                assert_eq!(targets.len(), 8, "off-screen panels must retain their tabs");
                for &(index, x) in &targets {
                    let tab = vcx
                        .debug_bounds(Box::leak(
                            format!("live-session-tab-{index}").into_boxed_str(),
                        ))
                        .unwrap();
                    assert!(tab.left() >= track.left() - px(0.01));
                    assert!(tab.right() <= track.right() + px(0.01));
                    assert!(tab.size.width > px(0.0));
                    assert!(track.left() + px(x) > tab.left());
                    assert!(track.left() + px(x) < tab.right());
                }
                let x = targets
                    .iter()
                    .find(|(index, _)| *index == selected)
                    .unwrap()
                    .1;
                vcx.simulate_click(
                    gpui::point(track.left() + px(x), track.bottom() - px(12.0)),
                    gpui::Modifiers::default(),
                );
                vcx.run_until_parked();
                assert_eq!(
                    workspace.read_with(vcx, |w, _| w.active),
                    selected,
                    "window={width}, selected={selected}, track={track:?}, targets={targets:?}, camera={:?}",
                    workspace.read_with(vcx, |w, _| (w.camera_x, w.camera_target, w.camera_dirty))
                );
            }
        }
    }

    #[gpui::test]
    fn live_tabs_four_panel_group_stays_centered_when_focus_changes(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_sidebar = false;
            for i in 0..4 {
                w.push_test_panel(&format!("panel-{i}"), cx);
                w.slots[i].width_fraction = 0.25;
                w.slots[i].animated_width = AnimatedValue::new(0.25, Duration::ZERO);
            }
            w
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(1600.0), px(700.0)));
        vcx.run_until_parked();
        let selectors = [
            "live-session-tab-0",
            "live-session-tab-1",
            "live-session-tab-2",
            "live-session-tab-3",
        ];
        let initial = selectors.map(|selector| vcx.debug_bounds(selector).unwrap());
        let track = vcx.debug_bounds("live-session-tabs").unwrap();
        assert!(
            ((initial[0].left() + initial[3].right()) / 2.0 - track.center().x).abs() < px(0.01)
        );
        for pair in initial.windows(2) {
            assert_eq!(pair[0].right() - pair[1].left(), px(12.0));
        }
        for selected in [1, 2, 3, 0] {
            vcx.simulate_click(initial[selected].center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            workspace.update(vcx, |w, cx| {
                w.live_tabs.settle();
                cx.notify();
            });
            vcx.run_until_parked();
            assert_eq!(workspace.read_with(vcx, |w, _| w.active), selected);
            for (i, selector) in selectors.iter().enumerate() {
                let tab = vcx.debug_bounds(*selector).unwrap();
                assert!((tab.left() - initial[i].left()).abs() <= px(12.01));
                let panel = vcx
                    .debug_bounds(["panel-0", "panel-1", "panel-2", "panel-3"][i])
                    .unwrap();
                assert_eq!(tab.bottom(), panel.top());
            }
        }
    }

    #[gpui::test]
    fn live_tabs_stay_still_while_the_panel_camera_moves(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_sidebar = false;
            for i in 0..4 {
                w.push_test_panel(&format!("panel-{i}"), cx);
            }
            w
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(1200.0), px(700.0)));
        vcx.run_until_parked();
        let selectors = [
            "live-session-tab-0",
            "live-session-tab-1",
            "live-session-tab-2",
            "live-session-tab-3",
        ];
        let initial = selectors.map(|selector| vcx.debug_bounds(selector).unwrap());
        let panel_left = vcx.debug_bounds("panel-1").unwrap().left();
        for camera in [80.0, 160.0, 480.0] {
            workspace.update(vcx, |w, cx| {
                w.camera_x[0] = camera;
                w.camera_target[0] = camera;
                w.camera_started[0] = None;
                w.camera_dirty[0] = false;
                cx.notify();
            });
            vcx.run_until_parked();
            for (i, selector) in selectors.iter().enumerate() {
                assert_eq!(vcx.debug_bounds(*selector).unwrap(), initial[i]);
            }
            assert!(
                (panel_left - vcx.debug_bounds("panel-1").unwrap().left() - px(camera)).abs()
                    < px(0.01)
            );
        }
    }

    #[test]
    fn live_tabs_motion_expands_lifts_and_retargets_without_jumping() {
        let now = Instant::now();
        let duration = Duration::from_millis(150);
        let layout = TabLayout::new(512.0, 12);
        let targets = |selected| {
            (0..12)
                .map(|index| (index as u64, layout.geometry(index, selected)))
                .collect::<Vec<_>>()
        };
        let mut motion = TabMotion::default();
        let initial = motion.sample(&targets(0), 512.0, duration, now);
        assert!(!motion.is_animating());
        let start = motion.sample(&targets(5), 512.0, duration, now);
        assert_eq!(initial, start);
        assert!(motion.is_animating());
        let middle = motion.sample(
            &targets(5),
            512.0,
            duration,
            now + Duration::from_millis(50),
        );
        assert!(middle[5].width > initial[5].width && middle[5].width < 208.0);
        assert!(middle[5].height > 28.0 && middle[5].height < 32.0);
        assert!(middle[5].left < initial[5].left);
        let reversal = motion.sample(
            &targets(0),
            512.0,
            duration,
            now + Duration::from_millis(50),
        );
        assert_eq!(middle, reversal);
        let settled = motion.sample(
            &targets(0),
            512.0,
            duration,
            now + Duration::from_millis(250),
        );
        assert_eq!(settled, initial);
        assert!(!motion.is_animating());
        let reduced = motion.sample(&targets(5), 512.0, Duration::ZERO, now);
        assert_eq!(reduced[5], layout.geometry(5, 5));
        assert!(!motion.is_animating());
        let smaller = TabLayout::new(192.0, 12);
        let targets: Vec<_> = (0..12)
            .map(|i| (i as u64, smaller.geometry(i, 5)))
            .collect();
        let resized = motion.sample(&targets, 192.0, duration, now);
        assert!(!motion.is_animating());
        assert!(
            resized
                .iter()
                .all(|tab| tab.left >= 0.0 && tab.left + tab.width <= 192.001)
        );
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
        workspace.update(vcx, |w, cx| {
            // Finish both independent animations before checking selection.
            w.camera_x[w.active_row] = w.camera_target[w.active_row];
            w.camera_started[w.active_row] = None;
            w.live_tabs.settle();
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.debug_bounds("live-session-tab-11").unwrap().size.width,
            px(208.)
        );
        vcx.simulate_keystrokes("super-u");
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            // Finish both independent animations before checking selection.
            w.camera_x[w.active_row] = w.camera_target[w.active_row];
            w.camera_started[w.active_row] = None;
            w.live_tabs.settle();
            cx.notify();
        });
        vcx.run_until_parked();
        let first = vcx.debug_bounds("live-session-tab-0").unwrap();
        assert!(first.left() >= track.left());
        vcx.update(|window, cx| window.simulate_mouse_move(first.center(), cx));
        vcx.run_until_parked();
        vcx.executor().advance_clock(Duration::from_secs(1));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("live-session-tab-tooltip").is_some());
        for width in [1440., 800., 640., 480., 1000.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            vcx.run_until_parked();
            // Resize retargets camera/tab motion. Click settled geometry rather
            // than bounds from an animation frame that can move before dispatch.
            workspace.update(vcx, |workspace, cx| {
                workspace.camera_x[workspace.active_row] =
                    workspace.camera_target[workspace.active_row];
                workspace.camera_started[workspace.active_row] = None;
                workspace.live_tabs.settle();
                cx.notify();
            });
            vcx.run_until_parked();
            let track = vcx.debug_bounds("live-session-tabs").unwrap();
            // Crowded folders retain exposed edges inside the centered group.
            for index in 0..12 {
                let tab = vcx
                    .debug_bounds(Box::leak(
                        format!("live-session-tab-{index}").into_boxed_str(),
                    ))
                    .unwrap();
                assert!(tab.left() >= track.left());
                assert!(
                    tab.right() <= track.right() + px(1.),
                    "window={width} index={index} tab={tab:?} track={track:?}"
                );
                assert!(tab.size.width > px(0.) && tab.size.width <= px(208.));
                assert!(
                    vcx.debug_bounds(Box::leak(
                        format!("live-session-tab-{index}-emoji").into_boxed_str()
                    ))
                    .is_some()
                );
            }
            let last = vcx.debug_bounds("live-session-tab-11").unwrap();
            let edge = vcx.debug_bounds("edge-new-session").unwrap();
            assert!(edge.top() >= last.bottom());
            let previous = vcx.debug_bounds("live-session-tab-10").unwrap();
            let x = track.left()
                + px(workspace.read_with(vcx, |w, _| {
                    w.live_tabs
                        .hit_targets
                        .iter()
                        .find(|(i, _)| *i == 11)
                        .unwrap()
                        .1
                }));
            vcx.simulate_click(gpui::point(x, last.center().y), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert_eq!(
                workspace.read_with(vcx, |w, _| w.active),
                11,
                "width={width}, last={last:?}, previous={previous:?}, x={x:?}"
            );
            workspace.update_in(vcx, |workspace, window, cx| {
                assert_eq!(workspace.navigation_state(window, cx)["keyboard_panel"], 11);
                assert_eq!(workspace.test_coach().trace("new_panel").slow_paths, 0);
            });
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
