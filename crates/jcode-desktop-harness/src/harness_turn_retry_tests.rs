fn fast_retry_worker(
    connections_tx: std::sync::mpsc::Sender<Server>,
    rx: Receiver<SessionCommand>,
    tx: async_channel::Sender<Update>,
    base_delay: Duration,
) -> std::thread::JoinHandle<()> {
    let worker_id = remote::namespace("desktop", "same-id");
    std::thread::spawn(move || {
        TEST_RETRY_POLICY.with(|policy| {
            *policy.borrow_mut() = Some(jcode_sdk::RetryPolicy {
                max_attempts: 2,
                base_delay,
                max_delay: base_delay * 2,
            })
        });
        session_worker_with_connector(worker_id, rx, UpdateSender(tx), None, |_| {
            let (client, server) = Server::new();
            connections_tx.send(server).unwrap();
            Ok(client)
        })
    })
}

fn attach(server: &Server) {
    assert!(matches!(server.next(), ApiRequest::AttachSession { .. }));
    assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
    assert!(matches!(server.next(), ApiRequest::GetRuntimeInfo { .. }));
}

/// The bridge's terminal sequence for a failed model turn.
fn fail_turn(server: &Server, message: &str) {
    server.event(ApiEvent::TurnStopped {
        session_id: "same-id".into(),
        reason: jcode_sdk::TurnStopReason::Failure,
        message: message.into(),
        provider_stop_reason: None,
    });
    server.event(ApiEvent::Error {
        code: jcode_sdk::api::ErrorCode::Internal,
        message: message.into(),
    });
    server.event(ApiEvent::TurnDone {
        session_id: "same-id".into(),
        pending_soft_interrupts: None,
    });
}

fn retry_phase(update: &Update) -> Option<String> {
    match update {
        Update::Event {
            event: ApiEvent::ConnectionPhase { phase, .. },
            ..
        } => Some(phase.clone()),
        _ => None,
    }
}

/// Wait until the worker has seen the server accept the last send, as the
/// real runtime always accepts a message before its turn can fail.
fn wait_accepted(updates: &async_channel::Receiver<Update>) {
    wait_update(updates, |update| {
        matches!(
            update,
            Update::Event {
                event: ApiEvent::MessageAccepted { .. },
                ..
            }
        )
    });
}

#[test]
fn transient_turn_failure_is_continued_automatically_until_the_budget_runs_out() {
    let (commands, rx) = channel();
    let (tx, updates) = async_channel::unbounded();
    let (connections_tx, connections) = channel();
    let worker = fast_retry_worker(connections_tx, rx, tx, Duration::from_millis(20));
    let server = connections.recv_timeout(Duration::from_secs(4)).unwrap();
    attach(&server);

    commands
        .send(SessionCommand::Send {
            content: "build it".into(),
            images: vec![],
        })
        .unwrap();
    assert!(
        matches!(server.next(), ApiRequest::SendMessage { content, system_reminder: None, .. } if content == "build it")
    );
    wait_accepted(&updates);

    for attempt in 1..=2 {
        fail_turn(&server, "stream error: connection reset by peer");
        let phase = wait_update(&updates, |update| {
            retry_phase(update).is_some_and(|phase| {
                phase.starts_with("retrying in") && phase.ends_with(&format!("({attempt}/2)"))
            })
        });
        assert!(retry_phase(&phase).is_some());
        // The prompt is already in the transcript: the retry continues the
        // session with a hidden reminder instead of resending "build it".
        assert!(
            matches!(server.next(), ApiRequest::SendMessage { content, system_reminder: Some(reminder), .. }
                if content.is_empty() && reminder == jcode_sdk::turn_retry::CONTINUATION_REMINDER),
            "attempt {attempt}"
        );
        wait_accepted(&updates);
    }

    // Budget exhausted: the error stays for the user and nothing is resent.
    fail_turn(&server, "stream error: connection reset by peer");
    std::thread::sleep(Duration::from_millis(150));
    assert!(server.requests.try_recv().is_err());

    commands.send(SessionCommand::Stop).unwrap();
    worker.join().unwrap();
    server.disconnect();
}

#[test]
fn permanent_failures_and_cancellation_never_retry() {
    let (commands, rx) = channel();
    let (tx, updates) = async_channel::unbounded();
    let (connections_tx, connections) = channel();
    // Long enough that Stop always lands before the retry fires.
    let worker = fast_retry_worker(connections_tx, rx, tx, Duration::from_secs(2));
    let server = connections.recv_timeout(Duration::from_secs(4)).unwrap();
    attach(&server);

    fail_turn(&server, "401 Unauthorized: invalid x-api-key");
    std::thread::sleep(Duration::from_millis(150));
    assert!(server.requests.try_recv().is_err(), "credential failures are not retried");

    // A scheduled retry is dropped when the user presses Stop before it fires.
    fail_turn(&server, "503 Service Unavailable");
    wait_update(&updates, |update| {
        retry_phase(update).is_some_and(|phase| phase.starts_with("retrying in"))
    });
    commands.send(SessionCommand::Cancel).unwrap();
    assert!(matches!(server.next(), ApiRequest::Cancel { .. }));
    wait_update(&updates, |update| retry_phase(update).is_some_and(|p| p.is_empty()));
    std::thread::sleep(Duration::from_millis(2300));
    assert!(server.requests.try_recv().is_err(), "cancel drops the retry");

    commands.send(SessionCommand::Stop).unwrap();
    worker.join().unwrap();
    server.disconnect();
}
