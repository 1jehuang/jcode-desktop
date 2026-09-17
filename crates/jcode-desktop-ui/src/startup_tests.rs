//! Startup acceptance through the real GPUI input and workspace reducers.
use super::*;

fn created(id: &str) -> jcode_sdk::SessionInfo {
    jcode_sdk::SessionInfo {
        session_id: id.into(),
        title: None,
        working_dir: Some("/workspace".into()),
        status: "idle".into(),
        transcript_bytes: None,
        saved: false,
        updated_at_ms: None,
        last_active_at_ms: None,
        archived: false,
        archived_at_ms: None,
        parent_session_id: None,
        agent_label: None,
        swarm_status: None,
        edit_stats: None,
    }
}

#[gpui::test]
fn startup_draft_accepts_typing_before_connection_and_promotes_in_place(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.connected = false;
        workspace.open_startup_draft(cx);
        workspace.restore_focus(window, cx);
        workspace
    });
    vcx.run_until_parked();
    assert!(
        commands.try_recv().is_err(),
        "no Watch for a nonexistent session"
    );
    let panel = workspace.read_with(vcx, |workspace, cx| {
        assert!(!workspace.connected);
        assert_eq!(workspace.slots.len(), 1);
        assert!(!workspace.slots[0].animated_width.is_animating());
        assert!(workspace.slots[0].panel.read(cx).is_startup_draft());
        workspace.slots[0].panel.clone()
    });
    let input = panel.read_with(vcx, |panel, _| panel.input.clone());
    vcx.simulate_input("typed before the runtime started");
    vcx.simulate_keystrokes("enter");
    assert_eq!(
        input.read_with(vcx, |input, _| input.content.to_string()),
        "typed before the runtime started"
    );
    assert!(
        commands.try_recv().is_err(),
        "Enter retains the unsubmitted draft"
    );
    workspace.update(vcx, |workspace, cx| {
        workspace.apply(
            Update::Disconnected {
                reason: "still starting".into(),
            },
            cx,
        );
        workspace.apply(Update::Connected, cx);
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { request_id: Some(id), .. }) if id == Panel::STARTUP_SESSION_ID)
    );
    workspace.update(vcx, |workspace, cx| {
        workspace.apply(
            Update::SessionCreated {
                session: created("session-ready"),
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        assert_eq!(workspace.slots.len(), 1);
        assert_eq!(workspace.slots[0].panel.entity_id(), panel.entity_id());
        assert_eq!(panel.read(cx).input.entity_id(), input.entity_id());
    });
    vcx.simulate_input(" and after");
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, .. })
        if session_id == "session-ready" && content == "typed before the runtime started and after")
    );
}

#[gpui::test]
fn startup_draft_survives_reload_without_watching_placeholder(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.connected = false;
        workspace.open_startup_draft(cx);
        workspace.restore_focus(window, cx);
        workspace
    });
    vcx.run_until_parked();
    vcx.simulate_input("keep this across reload");
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            let snapshot = workspace.snapshot(window, cx).unwrap();
            workspace.apply_snapshot(snapshot, cx);
            workspace.restore_focus(window, cx);
        });
    });
    assert!(commands.try_recv().is_err());
    vcx.simulate_input(" too");
    vcx.simulate_keystrokes("enter");
    workspace.read_with(vcx, |workspace, cx| {
        assert!(workspace.slots[0].panel.read(cx).is_startup_draft());
        assert_eq!(
            workspace.slots[0]
                .panel
                .read(cx)
                .input
                .read(cx)
                .content
                .as_ref(),
            "keep this across reload too"
        );
    });
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn startup_completion_neither_steals_focus_nor_reopens_a_closed_draft(
    cx: &mut gpui::TestAppContext,
) {
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.connected = false;
        workspace.open_startup_draft(cx);
        workspace
    });
    workspace.update(vcx, |workspace, cx| {
        // A separate user request can finish first. It must not claim the draft.
        workspace.apply(
            Update::SessionCreated {
                session: created("other-session"),
                request_id: None,
            },
            cx,
        );
        assert!(workspace.slots[0].panel.read(cx).is_startup_draft());
        let active = workspace.active;
        workspace.focus_pending = false;
        workspace.apply(
            Update::SessionCreated {
                session: created("startup-session"),
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        assert_eq!(workspace.active, active);
        assert!(!workspace.focus_pending);
        workspace.slots.clear();
        workspace.apply(
            Update::SessionCreated {
                session: created("closed-session"),
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        workspace.apply(Update::Connected, cx);
        assert!(workspace.slots.is_empty());
    });
    let sent: Vec<_> = commands.try_iter().collect();
    assert!(sent.iter().any(|command| matches!(command, Command::Unwatch { session_id } if session_id == "closed-session")));
    assert!(
        !sent
            .iter()
            .any(|command| matches!(command, Command::CreateSession { .. }))
    );
}

#[gpui::test]
fn startup_creation_finishing_during_close_releases_its_worker(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.open_startup_draft(cx);
        workspace.slots[0].closing = true;
        workspace
    });
    workspace.update(vcx, |workspace, cx| {
        workspace.apply(Update::Connected, cx);
        workspace.apply(
            Update::SessionCreated {
                session: created("closing-session"),
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        assert!(workspace.slots[0].panel.read(cx).is_startup_draft());
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Unwatch { session_id }) if session_id == "closing-session")
    );
    assert!(commands.try_recv().is_err());
}
