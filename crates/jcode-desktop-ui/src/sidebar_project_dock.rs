//! Stable project shortcuts above the account footer. Membership and geometry
//! never depend on which session-list rows happened to be measured last frame.
use super::*;

const ROW_HEIGHT: f32 = 24.0;
const VISIBLE_ROWS: usize = 3;

/// Keep navigation alphabetical even when activity reorders the session groups.
/// The item index is only a click destination, not the identity of a shortcut.
fn shortcuts(groups: &[(Project, usize, usize)], rows: &[SidebarRow]) -> Vec<(usize, usize)> {
    if groups.len() < 2 {
        return Vec::new();
    }
    let mut entries = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.header)
        .map(|(item, row)| (item, row.project))
        .collect::<Vec<_>>();
    entries.sort_by_cached_key(|(_, project)| {
        let info = &groups[*project].0;
        (info.label.to_lowercase(), info.key.clone())
    });
    entries
}

impl Workspace {
    pub(in super::super) fn render_project_dock(
        &self,
        groups: &[(Project, usize, usize)],
        rows: &[SidebarRow],
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let entries = shortcuts(groups, rows);
        if entries.is_empty() {
            return None;
        }
        let height = entries.len().min(VISIBLE_ROWS) as f32 * ROW_HEIGHT;
        Some(
            div()
                .id("sidebar-project-dock")
                .debug_selector(|| "sidebar-project-dock".into())
                .flex_none()
                .w_full()
                .pt_1()
                .pr(px(crate::scrollbar::GUTTER))
                .child(
                    div()
                        .id("sidebar-project-dock-scroll")
                        .debug_selector(|| "sidebar-project-dock-scroll".into())
                        .h(px(height))
                        .overflow_y_scroll()
                        .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                        .children(entries.into_iter().map(|(item, project)| {
                            let (info, _, count) = &groups[project];
                            let key = info.key.clone();
                            div()
                                .id(gpui::SharedString::from(format!(
                                    "sidebar-project-dock-item-{key}"
                                )))
                                .debug_selector(move || format!("sidebar-project-dock-{key}"))
                                .flex_none()
                                .mx_2()
                                .pl_2()
                                .pr_1()
                                .h(px(ROW_HEIGHT))
                                .flex()
                                .items_center()
                                .gap_1()
                                .rounded_full()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(Theme::global().TEXT_DIM)
                                .hover(|el| {
                                    el.bg(Theme::global().TOOL_BG)
                                        .text_color(Theme::global().TEXT)
                                })
                                .child(div().w(px(12.0)).flex_none().text_size(px(10.0)).child("›"))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(info.label.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(16.0))
                                        .text_right()
                                        .text_size(px(10.0))
                                        .child(count.to_string()),
                                )
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        this.sidebar_sessions_list.scroll_to(gpui::ListOffset {
                                            item_ix: item,
                                            offset_in_item: px(0.0),
                                        });
                                        cx.notify();
                                    }),
                                )
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(key: &str, label: &str) -> (Project, usize, usize) {
        (
            Project {
                key: key.into(),
                label: label.into(),
                location: None,
            },
            0,
            1,
        )
    }

    fn row(project: usize) -> SidebarRow {
        SidebarRow {
            open: true,
            live: true,
            session: super::super::super::tests::session_info("session_fox_dock", None),
            project,
            subdirectory: None,
            header: true,
            collapsed: false,
            divider: false,
            checkout: None,
            checkout_header: false,
            section: None,
            agents: (0, 0),
        }
    }

    #[test]
    fn shortcuts_ignore_activity_order_and_keep_current_jump_indices() {
        let groups = vec![
            group("/z", "zulu"),
            group("/b", "Beta"),
            group("/a", "alpha"),
        ];
        let rows = vec![row(0), row(1), row(2)];
        assert_eq!(shortcuts(&groups, &rows), vec![(2, 2), (1, 1), (0, 0)]);
        let reordered = vec![groups[2].clone(), groups[0].clone(), groups[1].clone()];
        assert_eq!(shortcuts(&reordered, &rows), vec![(0, 0), (2, 2), (1, 1)]);
    }

    #[test]
    fn equal_labels_use_project_key_for_deterministic_order() {
        let groups = vec![group("/z", "Repo"), group("/a", "repo")];
        assert_eq!(shortcuts(&groups, &[row(0), row(1)]), vec![(1, 1), (0, 0)]);
    }

    #[test]
    fn lone_project_needs_no_shortcut_dock() {
        assert!(shortcuts(&[], &[]).is_empty());
        assert!(shortcuts(&[group("/a", "alpha")], &[row(0)]).is_empty());
    }

    fn fixture(cx: &mut Context<Workspace>) -> Workspace {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for name in ["zulu", "alpha", "beta", "delta", "gamma"] {
            workspace.push_test_panel(&format!("session_fox_dock_{name}"), cx);
            workspace
                .slots
                .last()
                .unwrap()
                .panel
                .update(cx, |panel, _| {
                    panel.working_dir = Some(format!("/nowhere/{name}"));
                });
        }
        for index in 0..40 {
            let mut session = super::super::super::tests::session_info(
                &format!("session_owl_dock_old_{index}"),
                Some("Previous alpha chat"),
            );
            session.working_dir = Some("/nowhere/alpha".into());
            workspace.sessions.push(session);
        }
        workspace.active = 0;
        workspace
    }

    #[gpui::test]
    fn dock_geometry_and_order_survive_scroll_focus_collapse_and_remeasurement(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| fixture(cx));
        vcx.run_until_parked();
        let dock = vcx.debug_bounds("sidebar-project-dock").unwrap();
        let account = vcx.debug_bounds("sidebar-account").unwrap();
        let alpha = vcx
            .debug_bounds("sidebar-project-dock-/nowhere/alpha")
            .unwrap();
        let beta = vcx
            .debug_bounds("sidebar-project-dock-/nowhere/beta")
            .unwrap();
        let delta = vcx
            .debug_bounds("sidebar-project-dock-/nowhere/delta")
            .unwrap();
        assert_eq!(
            vcx.debug_bounds("sidebar-project-dock-scroll")
                .unwrap()
                .size
                .height,
            px(72.0)
        );
        assert!(alpha.bottom() <= beta.top() && beta.bottom() <= delta.top());
        assert!(dock.bottom() <= account.top());
        for stage in 0..6 {
            workspace.update(vcx, |w, cx| {
                match stage {
                    0 => w.sidebar_sessions_list.scroll_to_end(),
                    1 => w.active = 1,
                    2 => {
                        w.sidebar_projects
                            .collapsed
                            .insert("/nowhere/alpha".into(), true);
                    }
                    3 => w
                        .sidebar_sessions_list
                        .reset(w.sidebar_session_layout.len()),
                    4 => w.active = 0,
                    _ => w.sidebar_sessions_list.scroll_to(gpui::ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.0),
                    }),
                }
                cx.notify();
            });
            vcx.run_until_parked();
            assert_eq!(
                vcx.debug_bounds("sidebar-project-dock").unwrap(),
                dock,
                "stage {stage}"
            );
            assert_eq!(
                vcx.debug_bounds("sidebar-account").unwrap(),
                account,
                "stage {stage}"
            );
            assert_eq!(
                vcx.debug_bounds("sidebar-project-dock-/nowhere/alpha")
                    .unwrap(),
                alpha
            );
            assert_eq!(
                vcx.debug_bounds("sidebar-project-dock-/nowhere/beta")
                    .unwrap(),
                beta
            );
        }
    }

    #[gpui::test]
    fn overflow_shortcuts_scroll_independently_and_jump_without_changing_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| fixture(cx));
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            w.sidebar_sessions_list.scroll_to_end();
            cx.notify();
        });
        vcx.run_until_parked();
        let dock = vcx.debug_bounds("sidebar-project-dock-scroll").unwrap();
        let offset = workspace.read_with(vcx, |w, _| {
            w.sidebar_sessions_list.scroll_px_offset_for_scrollbar()
        });
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: dock.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-96.0))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        let zulu = vcx
            .debug_bounds("sidebar-project-dock-/nowhere/zulu")
            .unwrap();
        assert!(zulu.top() >= dock.top() && zulu.bottom() <= dock.bottom());
        workspace.read_with(vcx, |w, _| {
            assert_eq!(
                w.sidebar_sessions_list.scroll_px_offset_for_scrollbar(),
                offset
            );
        });
        vcx.simulate_click(zulu.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-project-0").is_some());
        workspace.read_with(vcx, |w, _| assert_eq!(w.active, 0));
        assert_eq!(
            vcx.debug_bounds("sidebar-project-dock-scroll").unwrap(),
            dock
        );
    }
}
