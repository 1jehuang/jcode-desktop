use super::*;

fn todo(status: &str) -> TodoCardItem {
    serde_json::from_value(serde_json::json!({"content":"verify work", "status":status})).unwrap()
}
fn done(state: &mut AutoPoke) {
    state.observe(&ApiEvent::TurnDone {
        session_id: "poke-test".into(),
    });
}

#[test]
fn auto_poke_requires_local_request_completion_and_enabled_feature() {
    let mut state = AutoPoke::default();
    done(&mut state);
    assert!(state.claim(true, &[todo("pending")]).is_none());
    state.start(0);
    assert!(state.claim(true, &[todo("pending")]).is_none());
    done(&mut state);
    assert!(state.claim(false, &[todo("pending")]).is_none());
    done(&mut state);
    assert_eq!(
        state.claim(true, &[todo("pending")]),
        Some(jcode_base::todo::build_auto_poke_message(1))
    );
    assert!(state.claim(true, &[todo("in_progress")]).is_none());
}

#[test]
fn auto_poke_ignores_blocked_finished_and_unknown_work() {
    for status in [
        "blocked",
        "completed",
        "done",
        "cancelled",
        "canceled",
        "waiting",
        "",
    ] {
        let mut state = AutoPoke::default();
        state.start(0);
        done(&mut state);
        assert!(state.claim(true, &[todo(status)]).is_none(), "{status}");
    }
    let mut state = AutoPoke::default();
    state.start(0);
    done(&mut state);
    let mut blocked = todo("pending");
    blocked.blocked_by.push("user approval".into());
    assert!(state.claim(true, &[blocked]).is_none());
}

#[test]
fn auto_poke_bounds_changing_work_and_stops_repeated_work() {
    let mut state = AutoPoke::default();
    state.start(0);
    for index in 0..MAX_FOLLOW_UPS + 2 {
        let mut item = todo("pending");
        item.content = index.to_string();
        done(&mut state);
        assert_eq!(state.claim(true, &[item]).is_some(), index < MAX_FOLLOW_UPS);
    }
    state.start(0);
    done(&mut state);
    assert!(state.claim(true, &[todo("pending")]).is_some());
    done(&mut state);
    assert!(state.claim(true, &[todo("in_progress")]).is_some());
    done(&mut state);
    assert!(state.claim(true, &[todo("pending")]).is_none());
}

#[test]
fn auto_poke_cancel_and_error_disarm_until_next_user_request() {
    let events = [
        serde_json::json!({"ev":"error", "code":"internal", "message":"failed"}),
        serde_json::json!({"ev":"session_status", "session_id":"poke-test", "status":"cancelled"}),
    ];
    for event in events {
        let mut state = AutoPoke::default();
        state.start(0);
        state.observe(&serde_json::from_value(event).unwrap());
        done(&mut state);
        assert!(state.claim(true, &[todo("pending")]).is_none());
        state.start(0);
        done(&mut state);
        assert!(state.claim(true, &[todo("pending")]).is_some());
    }
}

#[gpui::test]
fn auto_poke_real_panel_completion_is_once_and_user_queue_wins(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| {
        let mut p = Panel::new("poke-test".into(), None, None, bridge, cx);
        p.history_loaded = true;
        p
    });
    panel.update(cx, |p, cx| {
        p.submit_or_queue("do work".into(), vec![], false, cx);
        p.pending_users.clear();
        p.items.push(Item::Todos(TodoCardPayload {
            todos: vec![todo("pending")],
            ..Default::default()
        }));
        p.apply(
            &ApiEvent::TurnDone {
                session_id: p.session_id.clone(),
            },
            cx,
        );
        p.apply(
            &ApiEvent::TurnDone {
                session_id: p.session_id.clone(),
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "do work")
    );
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if jcode_base::todo::is_auto_poke_message(&content))
    );
    assert!(commands.try_recv().is_err());
    panel.update(cx, |p, cx| {
        p.pending_users.clear();
        p.status = "running".into();
        p.submit_or_queue("user priority".into(), vec![], true, cx);
        p.apply(
            &ApiEvent::TurnDone {
                session_id: p.session_id.clone(),
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "user priority")
    );
    assert!(commands.try_recv().is_err());
}

#[test]
fn auto_poke_is_not_restored_from_window_snapshot() {
    let mut queue = PromptQueue::default();
    queue.auto_poke.start(0);
    let restored: PromptQueue =
        serde_json::from_value(serde_json::to_value(queue).unwrap()).unwrap();
    assert_eq!(restored.auto_poke, AutoPoke::default());
}

#[gpui::test]
fn auto_poke_panel_suppresses_stale_todos_cancel_and_send_failure(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| {
        let mut p = Panel::new("poke-test".into(), None, None, bridge, cx);
        p.history_loaded = true;
        p.items.push(Item::Todos(TodoCardPayload {
            todos: vec![todo("pending")],
            ..Default::default()
        }));
        p
    });
    // Old todos cannot continue an unrelated new request.
    panel.update(cx, |p, cx| {
        p.submit_or_queue("new request".into(), vec![], false, cx);
        p.pending_users.clear();
        p.apply(
            &ApiEvent::TurnDone {
                session_id: p.session_id.clone(),
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "new request")
    );
    assert!(commands.try_recv().is_err());
    for failed_send in [false, true] {
        panel.update(cx, |p, cx| {
            p.submit_or_queue("retry explicitly".into(), vec![], false, cx);
            p.pending_users.clear();
            p.items.push(Item::Todos(TodoCardPayload {
                todos: vec![todo("pending")],
                ..Default::default()
            }));
            // A local stop or a send failure wins even before its cancellation
            // status arrives from the server, including a deferred completion.
            p.apply(
                &ApiEvent::TurnDone {
                    session_id: p.session_id.clone(),
                },
                cx,
            );
            if failed_send {
                p.message_failed("send failed".into(), cx);
            } else {
                p.prompt_queue.paused = true;
            }
        });
        cx.run_until_parked();
        assert!(
            matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "retry explicitly")
        );
        assert!(commands.try_recv().is_err());
    }
}
