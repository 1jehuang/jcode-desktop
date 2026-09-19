use super::*;
use serde_json::json;

fn event(value: serde_json::Value) -> ApiEvent {
    serde_json::from_value(value).unwrap()
}

#[gpui::test]
fn ctrl_enter_queues_until_completion_and_enter_still_sends(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        let mut panel = Panel::new("queue-test".into(), None, None, bridge, cx);
        panel.history_loaded = true;
        panel.status = "running".into();
        panel
    });
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        let handle = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    vcx.simulate_input("first queued prompt");
    vcx.simulate_keystrokes("ctrl-enter");
    vcx.simulate_input("second queued prompt");
    vcx.simulate_keystrokes("ctrl-enter");
    vcx.run_until_parked();
    assert!(
        commands.try_recv().is_err(),
        "queue must not steer the active turn"
    );
    assert!(vcx.debug_bounds("prompt-queue").is_some());
    panel.read_with(vcx, |p, cx| {
        assert_eq!(p.prompt_queue.prompts.len(), 2);
        assert!(p.input.read(cx).content.is_empty());
        assert!(
            p.items.is_empty(),
            "waiting prompts are not transcript messages yet"
        );
    });
    panel.update(vcx, |p, cx| {
        p.apply(
            &event(json!({"ev":"turn_done", "session_id":"queue-test"})),
            cx,
        );
        p.apply(
            &event(json!({"ev":"session_status", "session_id":"queue-test", "status":"idle"})),
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, images, .. })
        if content == "first queued prompt\n\nsecond queued prompt" && images.is_empty())
    );
    assert!(
        commands.try_recv().is_err(),
        "duplicate completion must not replay the queue"
    );
    assert!(vcx.debug_bounds("prompt-queue").is_none());

    vcx.simulate_input("send immediately");
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "send immediately")
    );
}

#[gpui::test]
fn queue_survives_reload_with_images_and_pauses_on_cancel(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| {
        let mut p = Panel::new("queue-test".into(), None, None, bridge, cx);
        p.history_loaded = true;
        p.status = "running".into();
        p
    });
    let images = vec![("image/png".into(), "image bytes".into())];
    panel.update(cx, |p, cx| {
        p.submit_or_queue("look at this".into(), images.clone(), true, cx);
        let snapshot = serde_json::to_vec(&p.snapshot(cx)).unwrap();
        p.prompt_queue = Default::default();
        p.restore_snapshot(serde_json::from_slice(&snapshot).unwrap(), cx);
        assert_eq!(p.prompt_queue.prompts[0].images, images);
        p.apply(
            &event(json!({"ev":"session_status", "session_id":"queue-test", "status":"cancelled"})),
            cx,
        );
        p.apply(
            &event(json!({"ev":"turn_done", "session_id":"queue-test"})),
            cx,
        );
    });
    cx.run_until_parked();
    assert!(commands.try_recv().is_err());
    panel.update(cx, |p, cx| {
        assert!(p.prompt_queue.paused);
        p.prompt_queue.paused = false;
        p.send_queued_prompts(cx);
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, images: actual, .. })
        if content == "look at this" && actual == images)
    );
}

#[gpui::test]
fn idle_queue_shortcut_sends_and_unaccepted_prompt_gates_next_queue(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| {
        let mut p = Panel::new("queue-test".into(), None, None, bridge, cx);
        p.history_loaded = true;
        p.status = "idle".into();
        p
    });
    panel.update(cx, |p, cx| {
        p.submit_or_queue("idle prompt".into(), vec![], true, cx);
        p.submit_or_queue("waiting for acceptance".into(), vec![], true, cx);
        p.apply(
            &event(json!({"ev":"session_status", "session_id":"queue-test", "status":"idle"})),
            cx,
        );
    });
    cx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "idle prompt")
    );
    assert!(commands.try_recv().is_err());
    panel.update(cx, |p, cx| {
        p.apply(
            &event(json!({"ev":"message_accepted", "session_id":"queue-test"})),
            cx,
        );
        p.apply(
            &event(json!({"ev":"turn_done", "session_id":"queue-test"})),
            cx,
        );
    });
    cx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "waiting for acceptance")
    );
}

fn attached_session(id: &str) -> jcode_sdk::SessionInfo {
    serde_json::from_value(json!({
        "session_id": id, "title": "Connected session", "status": "idle"
    }))
    .unwrap()
}

#[gpui::test]
fn startup_enter_and_ctrl_enter_queue_until_attached_history_ready(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(Panel::STARTUP_SESSION_ID.into(), None, None, bridge, cx)
    });
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        let handle = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    vcx.simulate_input("first startup prompt");
    vcx.simulate_keystrokes("enter");
    vcx.simulate_input("second startup prompt");
    vcx.simulate_keystrokes("ctrl-enter");
    vcx.run_until_parked();
    assert!(commands.try_recv().is_err());
    assert!(vcx.debug_bounds("prompt-queue").is_some());
    panel.read_with(vcx, |p, cx| {
        assert!(p.input.read(cx).content.is_empty());
        assert_eq!(p.prompt_queue.prompts.len(), 2);
        assert!(p.prompt_queue.waiting_for_connection);
        assert!(p.items.is_empty());
    });
    panel.update(vcx, |p, cx| {
        p.attach_startup_session(attached_session("session-ready"), cx);
        assert!(!p.history_loaded);
    });
    assert!(
        commands.try_recv().is_err(),
        "attachment alone must not drain"
    );
    panel.update(vcx, |p, cx| {
        // The real bridge sends SessionConnected before History, without
        // necessarily producing an idle event for a newly created session.
        p.status = "connected".into();
        p.load_history(vec![], vec![], cx);
        p.load_history(vec![], vec![], cx);
        p.send_queued_prompts(cx);
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, images })
        if session_id == "session-ready" && content == "first startup prompt\n\nsecond startup prompt" && images.is_empty())
    );
    assert!(commands.try_recv().is_err(), "readiness cannot send twice");
}

#[gpui::test]
fn startup_queue_preserves_images_through_snapshot_and_waits_for_identity(
    cx: &mut gpui::TestAppContext,
) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new(Panel::STARTUP_SESSION_ID.into(), None, None, bridge, cx));
    let images = vec![("image/png".into(), "encoded image".into())];
    panel.update(cx, |p, cx| {
        p.submit_or_queue("with attachment".into(), images.clone(), false, cx);
        let saved = serde_json::to_vec(&p.snapshot(cx)).unwrap();
        p.prompt_queue = Default::default();
        p.restore_snapshot(serde_json::from_slice(&saved).unwrap(), cx);
        assert!(p.prompt_queue.waiting_for_connection);
        assert_eq!(p.prompt_queue.prompts[0].images, images);
        p.status = "idle".into();
        p.load_history(vec![], vec![], cx);
    });
    assert!(
        commands.try_recv().is_err(),
        "history cannot send to a draft ID"
    );
    panel.update(cx, |p, cx| {
        p.attach_startup_session(attached_session("local-recovery"), cx);
        p.send_queued_prompts(cx);
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, images: actual })
        if session_id == "local-recovery" && content == "with attachment" && actual == images)
    );
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn startup_slash_command_stays_in_editor_until_explicit_ready_submission(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(Panel::STARTUP_SESSION_ID.into(), None, None, bridge, cx)
    });
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        let handle = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    vcx.simulate_input("/clear");
    vcx.simulate_keystrokes("enter");
    assert!(commands.try_recv().is_err());
    panel.read_with(vcx, |p, cx| {
        assert!(p.prompt_queue.prompts.is_empty());
        assert_eq!(p.input.read(cx).content.as_ref(), "/clear");
    });
    vcx.simulate_keystrokes("ctrl-enter");
    assert!(commands.try_recv().is_err());
    panel.update(vcx, |p, cx| {
        p.attach_startup_session(attached_session("session-ready"), cx);
    });
    vcx.simulate_keystrokes("enter");
    assert!(
        commands.try_recv().is_err(),
        "commands also wait for history"
    );
    panel.read_with(vcx, |p, cx| {
        assert_eq!(p.input.read(cx).content.as_ref(), "/clear");
        assert!(p.prompt_queue.prompts.is_empty());
    });
    panel.update(vcx, |p, cx| {
        p.load_history(vec![], vec![], cx);
        p.send_queued_prompts(cx);
    });
    assert!(
        commands.try_recv().is_err(),
        "connecting does not execute editor commands"
    );
    vcx.simulate_keystrokes("enter");
    assert!(matches!(commands.try_recv(), Ok(Command::SessionOperation {
        session_id, operation: SessionOperation::Clear,
    }) if session_id == "session-ready"));
    assert!(
        commands.try_recv().is_err(),
        "slash command is not a chat prompt"
    );
}

#[gpui::test]
fn removing_all_startup_prompts_clears_waiting_state_when_ready(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new(Panel::STARTUP_SESSION_ID.into(), None, None, bridge, cx));
    panel.update(cx, |p, cx| {
        p.submit_or_queue("remove this".into(), vec![], false, cx);
        p.prompt_queue.prompts.clear();
        p.prompt_queue.paused = true;
        p.attach_startup_session(attached_session("session-ready"), cx);
        assert!(p.prompt_queue.waiting_for_connection);
        p.load_history(vec![], vec![], cx);
        assert!(!p.prompt_queue.waiting_for_connection);
        p.status = "connected".into();
        p.submit_or_queue("send normally".into(), vec![], false, cx);
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, .. })
        if session_id == "session-ready" && content == "send normally")
    );
    assert!(commands.try_recv().is_err());
}
