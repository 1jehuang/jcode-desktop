use super::*;

const RECENTLY_CLOSED_LIMIT: usize = 5;
const RECENTLY_CLOSED_AGE: Duration = Duration::from_secs(30 * 60);

#[derive(Default)]
pub(super) struct History {
    pub show_all: bool,
    recently_closed: Vec<(String, Instant)>,
}

impl History {
    pub fn closed(&mut self, id: String, now: Instant) {
        self.recently_closed.retain(|(old, at)| {
            old != &id && now.saturating_duration_since(*at) < RECENTLY_CLOSED_AGE
        });
        self.recently_closed.insert(0, (id, now));
        self.recently_closed.truncate(RECENTLY_CLOSED_LIMIT);
    }

    pub fn visible(&self, id: &str, now: Instant) -> bool {
        self.show_all
            || self.recently_closed.iter().any(|(closed, at)| {
                closed == id && now.saturating_duration_since(*at) < RECENTLY_CLOSED_AGE
            })
    }
}

impl Workspace {
    pub(super) fn render_sidebar_history_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().px_3().py_1().child(
            div()
                .id("sidebar-history-toggle")
                .debug_selector(|| "sidebar-history-toggle".into())
                .rounded_full()
                .px_3()
                .py_1()
                .text_size(px(11.0))
                .text_color(Theme::global().TEXT_DIM)
                .cursor_pointer()
                .child(if self.sidebar_history.show_all {
                    "Hide past sessions"
                } else {
                    "Show past sessions"
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.sidebar_history.show_all = !this.sidebar_history.show_all;
                        cx.notify();
                    }),
                ),
        )
    }
}

#[derive(Default)]
pub(super) struct Selection {
    pub ids: HashSet<String>,
    anchor: Option<String>,
}

impl Selection {
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    pub fn retain(&mut self, order: &[String]) {
        self.ids.retain(|id| order.contains(id));
        if self.anchor.as_ref().is_some_and(|id| !order.contains(id)) {
            self.anchor = None;
        }
    }

    pub fn click(&mut self, id: &str, order: &[String], shift: bool, toggle: bool) {
        if shift {
            let anchor = self
                .anchor
                .as_ref()
                .and_then(|id| order.iter().position(|s| s == id));
            let target = order.iter().position(|s| s == id);
            if let (Some(a), Some(b)) = (anchor, target) {
                if !toggle {
                    self.ids.clear();
                }
                self.ids.extend(order[a.min(b)..=a.max(b)].iter().cloned());
                return;
            }
        }
        if !toggle {
            self.ids.clear();
        }
        if !toggle || !self.ids.remove(id) {
            self.ids.insert(id.to_owned());
        }
        self.anchor = Some(id.to_owned());
    }
}

impl Workspace {
    pub(super) fn close_sidebar_sessions(
        &mut self,
        ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let original = self.slots.get(self.active).map(|s| s.panel.entity_id());
        // Resolve IDs on each iteration: the default-directory close path can
        // remove a slot immediately, while ordinary panels animate out.
        for id in ids {
            if let Some(index) = self
                .slots
                .iter()
                .position(|s| !s.closing && s.panel.read(cx).session_id == id)
            {
                self.set_active(index, cx);
                self.close_panel(&ClosePanel, window, cx);
            }
            self.sidebar_selection.ids.remove(&id);
        }
        if let Some(index) = self
            .slots
            .iter()
            .position(|s| !s.closing && Some(s.panel.entity_id()) == original)
        {
            self.set_active(index, cx);
        } else if self.slots.get(self.active).is_none_or(|s| s.closing) {
            if let Some(index) = self.slots.iter().position(|s| !s.closing) {
                self.set_active(index, cx);
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn sidebar_hides_old_sessions_but_keeps_recent_and_live_sessions(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::super::tests::session_info;
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("open", cx);
            let mut external = session_info("external", Some("External user"));
            external.user_connected = true;
            workspace.sessions = vec![
                session_info("open", None),
                session_info("old", None),
                session_info("recent", None),
                external,
            ];
            workspace
                .sidebar_history
                .closed("recent".into(), Instant::now());
            workspace
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            let ids = w
                .sidebar_session_layout
                .iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>();
            assert!(ids.contains(&"open"));
            assert!(ids.contains(&"external"));
            assert!(ids.contains(&"recent"));
            assert!(!ids.contains(&"old"));
        });
        let toggle = vcx.debug_bounds("sidebar-history-toggle").unwrap();
        vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            assert!(
                w.sidebar_session_layout
                    .iter()
                    .any(|row| row.session_id == "old")
            )
        });
        let toggle = vcx.debug_bounds("sidebar-history-toggle").unwrap();
        vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            assert!(
                !w.sidebar_session_layout
                    .iter()
                    .any(|row| row.session_id == "old")
            )
        });
    }

    #[test]
    fn history_is_hidden_except_recent_closures_and_can_be_expanded() {
        let now = Instant::now();
        let mut history = History::default();
        assert!(!history.visible("old", now));
        history.closed("recent".into(), now);
        assert!(history.visible("recent", now));
        assert!(!history.visible("recent", now + RECENTLY_CLOSED_AGE));
        history.show_all = true;
        assert!(history.visible("old", now));
        history.show_all = false;
        assert!(!history.visible("old", now));
    }

    #[test]
    fn recent_closures_are_bounded_and_reclosing_refreshes_recency() {
        let now = Instant::now();
        let mut history = History::default();
        for index in 0..6 {
            history.closed(index.to_string(), now);
        }
        assert!(!history.visible("0", now));
        assert!(history.visible("5", now));
        history.closed("1".into(), now + Duration::from_secs(1));
        assert_eq!(history.recently_closed.len(), RECENTLY_CLOSED_LIMIT);
        assert_eq!(history.recently_closed[0].0, "1");
        history.closed(
            "new".into(),
            now + RECENTLY_CLOSED_AGE + Duration::from_secs(1),
        );
        assert_eq!(history.recently_closed.len(), 1);
    }

    #[test]
    fn range_selection_keeps_anchor_and_prunes_removed_sessions() {
        let order = ["a", "b", "c", "d"].map(String::from);
        let mut s = Selection::default();
        s.click("b", &order, false, false);
        s.click("d", &order, true, false);
        assert_eq!(s.ids, ["b", "c", "d"].map(String::from).into());
        s.click("a", &order, true, false);
        assert_eq!(s.ids, ["a", "b"].map(String::from).into());
        s.click("d", &order, false, true);
        s.click("b", &order, false, true);
        assert_eq!(s.ids, ["a", "d"].map(String::from).into());
        s.retain(&order[2..]);
        assert_eq!(s.ids, ["d"].map(String::from).into());
        s.click("c", &order[2..], true, false);
        assert_eq!(s.ids, ["c"].map(String::from).into());
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    fn exercise_sidebar(cx: &mut gpui::TestAppContext, mode: crate::config::LayoutMode) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.layout_mode = mode;
            for id in ["first", "second", "third", "fourth"] {
                w.push_test_panel(id, cx);
                w.sessions.push(jcode_sdk::SessionInfo {
                    user_connected: false,
                    session_id: id.into(),
                    working_dir: None,
                    title: Some(id.into()),
                    status: "idle".into(),
                    transcript_bytes: None,
                    saved: false,
                    updated_at_ms: None,
                    last_active_at_ms: None,
                    archived: false,
                    archived_at_ms: None,
                    save_label: None,
                    parent_session_id: None,
                    agent_label: None,
                    swarm_status: None,
                    edit_stats: None,
                });
            }
            w
        });
        vcx.run_until_parked();
        let first = vcx.debug_bounds("sidebar-session-0").unwrap();
        vcx.simulate_click(first.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let third = vcx.debug_bounds("sidebar-session-2").unwrap();
        vcx.simulate_click(
            third.center(),
            gpui::Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.sidebar_selection.ids.len(), 3);
            assert_eq!(
                w.slots[w.active].panel.read(cx).session_id,
                "first",
                "range selection must not navigate"
            );
        });
        let close = vcx.debug_bounds("sidebar-close-selected").unwrap();
        vcx.simulate_click(close.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            let live = w
                .slots
                .iter()
                .filter(|s| !s.closing)
                .map(|s| s.panel.read(cx).session_id.clone())
                .collect::<Vec<_>>();
            assert_eq!(live, ["fourth"]);
            assert!(w.sidebar_selection.ids.is_empty());
            assert_eq!(w.sessions.len(), 4, "closing must preserve session history");
        });
        // The inline close must not bubble into the row and reopen the panel.
        let row = vcx.debug_bounds("sidebar-session-0").unwrap();
        vcx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        vcx.run_until_parked();
        let close = vcx.debug_bounds("sidebar-close-0").unwrap();
        vcx.simulate_click(close.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| assert!(w.slots.iter().all(|s| s.closing)));
    }

    #[gpui::test]
    fn shift_click_and_bulk_close_folder_sidebar(cx: &mut gpui::TestAppContext) {
        exercise_sidebar(cx, crate::config::LayoutMode::FolderTabs);
    }

    #[gpui::test]
    fn shift_click_and_bulk_close_normal_sidebar(cx: &mut gpui::TestAppContext) {
        exercise_sidebar(cx, crate::config::LayoutMode::Normal);
    }

    #[gpui::test]
    fn closing_selection_across_rows_preserves_unselected_focus(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            for id in ["keep", "close-a", "close-b"] {
                w.push_test_panel(id, cx);
            }
            w.slots[2].row = 1;
            w
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.set_active(0, cx);
                w.close_sidebar_sessions(vec!["close-a".into(), "close-b".into()], window, cx);
                assert_eq!(w.active, 0);
                assert_eq!(w.active_row, 0);
                assert!(!w.slots[0].closing);
                assert!(w.slots[1].closing && w.slots[2].closing);
            })
        });
    }
}
