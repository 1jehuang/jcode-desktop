fn recovery_event(session: &str) -> ApiEvent {
    ApiEvent::SessionRecovery {
        session_id: session.into(),
        continuation_message: "Continue interrupted work.".into(),
        reconnect_notice: None,
    }
}

fn recovery_fence(worker: &Worker) {
    worker.event(ApiEvent::RuntimeInfo {
        session_id: "s1".into(),
        provider: None,
        model: None,
        routes: vec![],
        reasoning_effort: None,
    });
    worker.wait_for(|update| {
        matches!(
            update,
            Update::Event {
                event: ApiEvent::RuntimeInfo { .. },
                ..
            }
        )
    });
}

#[test]
fn interrupted_session_continues_once_without_composer_input() {
    let worker = Worker::new(|_| {
        vec![ApiEvent::MessageAccepted {
            session_id: "s1".into(),
        }]
    });
    worker.event(recovery_event("s1"));
    assert!(
        matches!(worker.request(), ApiRequest::SendMessage { session_id, content, images, system_reminder, .. }
        if session_id == "s1" && content.is_empty() && system_reminder.as_deref() == Some("Continue interrupted work.") && images.is_empty())
    );
    worker.wait_for(|update| matches!(update, Update::Event { event: ApiEvent::SessionStatus { status, .. }, .. } if status == "running"));
    worker.event(ApiEvent::TurnDone {
        session_id: "s1".into(),
    });
    worker.event(recovery_event("s1"));
    recovery_fence(&worker);
    assert!(
        worker.requests.try_recv().is_err(),
        "duplicate history must not continue again"
    );
}

#[test]
fn completed_session_and_other_sessions_recovery_remain_idle() {
    let worker = Worker::new(|_| panic!("idle session must not send"));
    worker.event(ApiEvent::SessionStatus {
        session_id: "s1".into(),
        status: "idle".into(),
    });
    worker.event(recovery_event("other-session"));
    recovery_fence(&worker);
    assert!(worker.requests.try_recv().is_err());
}

#[test]
fn already_running_or_completed_turn_suppresses_stale_recovery() {
    for event in [
        ApiEvent::SessionStatus {
            session_id: "s1".into(),
            status: "running".into(),
        },
        ApiEvent::TurnDone {
            session_id: "s1".into(),
        },
        ApiEvent::SessionStatus {
            session_id: "s1".into(),
            status: "cancelled".into(),
        },
    ] {
        let worker = Worker::new(|_| panic!("stale recovery must not send"));
        worker.event(event);
        worker.event(recovery_event("s1"));
        recovery_fence(&worker);
        assert!(worker.requests.try_recv().is_err());
    }
}

#[test]
fn busy_recovery_is_not_replayed_as_steering() {
    let worker = Worker::new(|_| vec![busy_error()]);
    worker.event(recovery_event("s1"));
    assert!(matches!(worker.request(), ApiRequest::SendMessage { .. }));
    recovery_fence(&worker);
    assert!(
        worker.requests.try_recv().is_err(),
        "another client's continuation is sufficient"
    );
}

#[test]
fn user_submission_supersedes_automatic_continuation() {
    let worker = Worker::new(|_| {
        vec![ApiEvent::MessageAccepted {
            session_id: "s1".into(),
        }]
    });
    worker.send("new instructions", vec![]);
    assert!(
        matches!(worker.request(), ApiRequest::SendMessage { content, .. } if content == "new instructions")
    );
    worker.event(ApiEvent::TurnDone {
        session_id: "s1".into(),
    });
    worker.event(recovery_event("s1"));
    recovery_fence(&worker);
    assert!(worker.requests.try_recv().is_err());
}

#[test]
fn recovery_state_resets_for_a_new_attachment_and_rejects_empty_directives() {
    let mut recovery = recovery::Recovery::default();
    assert!(!recovery.claim("  ", false));
    assert!(recovery.claim("continue", false));
    assert!(!recovery.claim("continue", false));
    let mut reconnected = recovery::Recovery::default();
    assert!(reconnected.claim("continue after another crash", false));
}

#[test]
fn explicit_cancel_or_clear_suppresses_late_recovery() {
    for command in [
        SessionCommand::Cancel,
        SessionCommand::Operation(SessionOperation::Clear),
    ] {
        let worker = Worker::new(|_| panic!("explicit user stop must win"));
        worker.commands.send(command).unwrap();
        worker
            .commands
            .send(SessionCommand::RefreshRuntime)
            .unwrap();
        // The synchronous runtime reply fences the preceding user command.
        worker.wait_for(|update| {
            matches!(
                update,
                Update::Event {
                    event: ApiEvent::RuntimeInfo { .. },
                    ..
                }
            )
        });
        worker.event(recovery_event("s1"));
        recovery_fence(&worker);
        assert!(worker.requests.try_recv().is_err());
    }
}
