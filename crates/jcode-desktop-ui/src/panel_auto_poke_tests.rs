use super::*;
use jcode_sdk::todo::{ConfidenceState, TodoItem, TodoSnapshot, is_auto_poke_message};

fn item(content: &str, status: &str) -> TodoItem {
    TodoItem {
        content: content.into(),
        status: status.into(),
        priority: "high".into(),
        id: content.into(),
        ..Default::default()
    }
}

fn snapshot(todos: Vec<TodoItem>) -> Option<TodoSnapshot> {
    Some(TodoSnapshot {
        todos,
        ..Default::default()
    })
}

fn card(status: &str) -> TodoCardItem {
    serde_json::from_value(serde_json::json!({"content": "verify work", "status": status})).unwrap()
}

fn panel(cx: &mut gpui::TestAppContext) -> (Entity<Panel>, std::sync::mpsc::Receiver<Command>) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| {
        let mut p = Panel::new("poke-test".into(), None, None, bridge, cx);
        p.history_loaded = true;
        // Tests must not depend on the developer's config file.
        p.prompt_queue.auto_poke.policy = Some(FollowUpPolicy::new(true));
        p
    });
    (panel, commands)
}

fn turn_done(p: &mut Panel, cx: &mut Context<Panel>) {
    p.pending_users.clear();
    p.apply(
        &ApiEvent::TurnDone {
            session_id: p.session_id.clone(),
            pending_soft_interrupts: None,
        },
        cx,
    );
}

fn drain(commands: &std::sync::mpsc::Receiver<Command>) -> Vec<Command> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

fn sent(commands: &[Command]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Send { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

fn asked_for_state(commands: &[Command]) -> bool {
    commands
        .iter()
        .any(|command| matches!(command, Command::TodoState { .. }))
}

#[gpui::test]
fn turn_end_fetches_sdk_todo_state_and_sends_the_policy_follow_up(cx: &mut gpui::TestAppContext) {
    let (panel, commands) = panel(cx);
    panel.update(cx, |p, cx| {
        p.submit_or_queue("do work".into(), vec![], false, cx);
        turn_done(p, cx);
    });
    cx.run_until_parked();
    let first = drain(&commands);
    assert_eq!(sent(&first), ["do work"]);
    assert!(
        asked_for_state(&first),
        "turn end must query the SDK todo state"
    );

    panel.update(cx, |p, cx| {
        p.todo_state_received(snapshot(vec![item("a", "pending")]), cx)
    });
    let poke = sent(&drain(&commands));
    assert_eq!(poke.len(), 1);
    assert!(is_auto_poke_message(&poke[0]));
    // A reply nobody asked for is ignored.
    panel.update(cx, |p, cx| {
        p.todo_state_received(snapshot(vec![item("a", "pending")]), cx)
    });
    assert!(drain(&commands).is_empty());
}

#[gpui::test]
fn completed_work_runs_quality_gates_then_final_response_once(cx: &mut gpui::TestAppContext) {
    let (panel, commands) = panel(cx);
    let weak = TodoItem {
        completion_confidence: Some(ConfidenceState::Speculative),
        ..item("ship", "completed")
    };
    let strong = TodoItem {
        completion_confidence: Some(ConfidenceState::Verified),
        ..item("ship", "completed")
    };
    panel.update(cx, |p, cx| {
        p.submit_or_queue("ship it".into(), vec![], false, cx)
    });
    let mut followups = Vec::new();
    for state in [weak, strong.clone(), strong] {
        panel.update(cx, |p, cx| turn_done(p, cx));
        cx.run_until_parked();
        drain(&commands);
        panel.update(cx, |p, cx| p.todo_state_received(snapshot(vec![state]), cx));
        followups.extend(sent(&drain(&commands)));
    }
    assert_eq!(followups.len(), 2, "{followups:?}");
    assert!(followups[0].starts_with(jcode_sdk::todo::TODO_COMPLETION_CONTINUATION_MESSAGE));
    assert_eq!(
        followups[1],
        jcode_sdk::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE
    );
}

#[gpui::test]
fn gate_digest_effects_are_acknowledged_to_the_runtime(cx: &mut gpui::TestAppContext) {
    let (panel, commands) = panel(cx);
    panel.update(cx, |p, cx| {
        p.submit_or_queue("work".into(), vec![], false, cx);
        turn_done(p, cx);
    });
    cx.run_until_parked();
    drain(&commands);
    let state = TodoSnapshot {
        todos: vec![TodoItem {
            completion_confidence: Some(ConfidenceState::Verified),
            ..item("ship", "completed")
        }],
        gate_observations: vec![jcode_sdk::todo::GateObservation {
            kind: jcode_sdk::todo::GateObservationKind::ClosedFeedbackLoop,
            group: None,
            state: None,
        }],
        ..Default::default()
    };
    panel.update(cx, |p, cx| p.todo_state_received(Some(state), cx));
    let commands = drain(&commands);
    assert!(commands.iter().any(|command| matches!(
        command,
        Command::AckTodoFollowUp { effects, .. } if effects.clear_gate_observations
    )));
    let sent = sent(&commands);
    assert!(sent[0].starts_with(jcode_sdk::todo::TODO_GATE_DIGEST_PREFIX));
}

#[gpui::test]
fn user_input_wins_over_an_in_flight_decision(cx: &mut gpui::TestAppContext) {
    let (panel, commands) = panel(cx);
    panel.update(cx, |p, cx| {
        p.submit_or_queue("work".into(), vec![], false, cx);
        turn_done(p, cx);
    });
    cx.run_until_parked();
    drain(&commands);
    panel.update(cx, |p, cx| {
        p.status = "running".into();
        p.submit_or_queue("user priority".into(), vec![], true, cx);
        p.todo_state_received(snapshot(vec![item("a", "pending")]), cx);
    });
    assert!(
        sent(&drain(&commands))
            .iter()
            .all(|text| !is_auto_poke_message(text))
    );
}

#[gpui::test]
fn stop_and_poke_off_suppress_follow_ups(cx: &mut gpui::TestAppContext) {
    let (panel, commands) = panel(cx);
    panel.update(cx, |p, cx| {
        p.submit_or_queue("work".into(), vec![], false, cx);
        turn_done(p, cx);
        // A local stop lands after the turn ended but before the reply.
        p.pause_queue_for_stop();
        p.todo_state_received(snapshot(vec![item("a", "pending")]), cx);
    });
    cx.run_until_parked();
    assert!(
        sent(&drain(&commands))
            .iter()
            .all(|text| !is_auto_poke_message(text))
    );

    panel.update(cx, |p, cx| {
        assert!(p.handle_slash_command("/poke off", cx));
        p.submit_or_queue("more".into(), vec![], false, cx);
        turn_done(p, cx);
    });
    cx.run_until_parked();
    let after_off = drain(&commands);
    assert!(
        !asked_for_state(&after_off),
        "disabled poke must not query state"
    );

    panel.update(cx, |p, cx| {
        assert!(p.handle_slash_command("/poke status", cx));
        assert!(matches!(p.items.last(), Some(Item::Assistant(text)) if text.contains("off")));
        assert!(p.handle_slash_command("/poke on", cx));
        assert!(p.handle_slash_command("/poke maybe", cx));
        assert!(matches!(p.items.last(), Some(Item::Error(text)) if text.contains("Usage")));
    });
}

#[gpui::test]
fn older_runtimes_fall_back_to_the_latest_todo_card(cx: &mut gpui::TestAppContext) {
    let (panel, commands) = panel(cx);
    panel.update(cx, |p, cx| {
        // A card from an earlier request must not drive this one.
        p.items.push(Item::Todos(TodoCardPayload {
            todos: vec![card("pending")],
            ..Default::default()
        }));
        p.submit_or_queue("new request".into(), vec![], false, cx);
        turn_done(p, cx);
    });
    cx.run_until_parked();
    drain(&commands);
    panel.update(cx, |p, cx| p.todo_state_received(None, cx));
    assert!(sent(&drain(&commands)).is_empty());

    panel.update(cx, |p, cx| {
        p.items.push(Item::Todos(TodoCardPayload {
            todos: vec![card("in_progress")],
            ..Default::default()
        }));
        turn_done(p, cx);
    });
    cx.run_until_parked();
    drain(&commands);
    panel.update(cx, |p, cx| p.todo_state_received(None, cx));
    let poke = sent(&drain(&commands));
    assert_eq!(poke.len(), 1);
    assert!(is_auto_poke_message(&poke[0]));
}

#[test]
fn poke_messages_render_as_short_notices_not_prompts() {
    let poke = jcode_sdk::todo::build_auto_poke_message(2);
    assert_eq!(auto_poke_notice_text(&poke), format!("👉 {poke}"));
    assert_eq!(
        auto_poke_notice_text(jcode_sdk::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE),
        "✅ Preparing the final response..."
    );
    assert!(!super::super::prompt::is_pinnable_prompt(&Item::User(poke)));
    assert!(super::super::prompt::is_pinnable_prompt(&Item::User(
        "real prompt".into()
    )));
}

#[test]
fn auto_poke_is_not_restored_from_window_snapshot() {
    let mut queue = PromptQueue::default();
    queue.auto_poke.start(0);
    let restored: PromptQueue =
        serde_json::from_value(serde_json::to_value(queue).unwrap()).unwrap();
    assert_eq!(restored.auto_poke, AutoPoke::default());
}
