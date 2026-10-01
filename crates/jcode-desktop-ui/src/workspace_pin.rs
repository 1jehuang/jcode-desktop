//! Sidebar pinning. A pin is the native `/save` bookmark, so pinned sessions
//! share the TUI's saved ordering and the resume browser's saved filter.
use super::*;

impl Workspace {
    pub(super) fn set_session_pinned(&mut self, session_id: &str, pinned: bool, cx: &mut Context<Self>) {
        // Optimistic, so the row moves at once. SessionSaved confirms it, and
        // a failure is reported while the next catalog refresh restores truth.
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
        {
            session.saved = pinned;
            if !pinned {
                session.save_label = None;
            }
        }
        self.bridge.send(Command::SetSessionSaved {
            session_id: session_id.to_string(),
            saved: pinned,
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn pin_button_saves_history_sessions_and_moves_them_to_the_top(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w
        });
        workspace.update(vcx, |w, cx| {
            let mut newer = crate::workspace::tests::session_info("session_owl_newer", Some("newer"));
            newer.updated_at_ms = Some(2_000);
            let mut older = crate::workspace::tests::session_info("session_fox_older", Some("older"));
            older.updated_at_ms = Some(1_000);
            w.apply(Update::Sessions { sessions: vec![newer, older] }, cx);
            cx.notify();
        });
        vcx.run_until_parked();
        while commands.try_recv().is_ok() {}
        assert!(vcx.debug_bounds("sidebar-session-1").is_some(), "history rows render");

        // The older session sits second; pin it from the sidebar.
        let row = vcx.debug_bounds("sidebar-session-1").unwrap();
        vcx.simulate_mouse_move(row.center(), None, gpui::Modifiers::none());
        vcx.run_until_parked();
        let pin = vcx.debug_bounds("sidebar-pin-1").expect("pin button on history row");
        vcx.simulate_click(pin.center(), gpui::Modifiers::none());
        vcx.run_until_parked();

        let mut sent = None;
        while let Ok(command) = commands.try_recv() {
            if let Command::SetSessionSaved { session_id, saved } = command {
                sent = Some((session_id, saved));
            }
        }
        assert_eq!(sent, Some(("session_fox_older".to_string(), true)));
        workspace.update(vcx, |w, _| {
            let order = sidebar_session_order(&w.sessions);
            assert_eq!(order[0].session_id, "session_fox_older", "pinned sessions lead");
            assert!(order[0].saved);
        });

        // Clicking again unpins.
        vcx.run_until_parked();
        let pin = vcx.debug_bounds("sidebar-pin-0").expect("pinned row leads");
        vcx.simulate_click(pin.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        let mut unpinned = None;
        while let Ok(command) = commands.try_recv() {
            if let Command::SetSessionSaved { session_id, saved } = command {
                unpinned = Some((session_id, saved));
            }
        }
        assert_eq!(unpinned, Some(("session_fox_older".to_string(), false)));
        workspace.update(vcx, |w, _| {
            assert!(!w.sessions.iter().any(|session| session.saved));
        });
    }
}
