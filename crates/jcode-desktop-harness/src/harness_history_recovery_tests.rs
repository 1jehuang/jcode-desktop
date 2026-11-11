#[test]
fn failed_bootstrap_history_recovers_without_reconnecting_or_replaying_input() {
    let (client, server) = Server::with_history_failures(2);
    let (commands, rx) = channel();
    let (tx, updates) = update_channel();
    let worker = std::thread::spawn(move || {
        session_worker_with_connector("same-id".into(), rx, tx, Some(client), |_| {
            panic!("history errors must not reconnect a healthy transport")
        });
    });
    assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
    assert!(matches!(server.next(), ApiRequest::GetRuntimeInfo { .. }));
    assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
    assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
    wait_update(
        &updates,
        |update| matches!(update, Update::History { session_id, .. } if session_id == "same-id"),
    );
    commands.send(SessionCommand::Stop).unwrap();
    worker.join().unwrap();
    server.disconnect();
}

#[test]
fn history_failure_budget_reports_actionable_error_and_keeps_commands_working() {
    let (client, server) = Server::with_history_failures(100);
    let (commands, rx) = channel();
    let (tx, updates) = update_channel();
    let worker = std::thread::spawn(move || {
        session_worker_with_connector("same-id".into(), rx, tx, Some(client), |_| {
            panic!("history errors must not reconnect a healthy transport")
        });
    });
    assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
    assert!(matches!(server.next(), ApiRequest::GetRuntimeInfo { .. }));
    for _ in 0..4 {
        assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        assert!(Instant::now() < deadline, "history failure notice missing");
        if let Ok(Update::CommandFailed { reason, .. }) = updates.try_recv() {
            assert!(reason.contains("history") && reason.contains("reopen"));
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    commands.send(SessionCommand::Cancel).unwrap();
    assert!(matches!(server.next(), ApiRequest::Cancel { .. }));
    commands.send(SessionCommand::Stop).unwrap();
    worker.join().unwrap();
    server.disconnect();
}

#[test]
fn remote_eof_reconnect_wait_is_interruptible_by_stop() {
    let ui_id = remote::namespace("desktop", "same-id");
    let (commands, rx) = channel();
    let (tx, updates) = update_channel();
    let (client, server) = Server::new();
    let (done_tx, done) = channel();
    let worker = std::thread::spawn(move || {
        session_worker_with_connector(ui_id, rx, tx, Some(client), |_| {
            panic!("Stop must interrupt the remote reconnect delay")
        });
        done_tx.send(()).unwrap();
    });
    assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
    assert!(matches!(server.next(), ApiRequest::GetRuntimeInfo { .. }));
    server.disconnect();
    wait_update(&updates, |update| {
        matches!(update, Update::SessionLost { .. })
    });
    commands.send(SessionCommand::Stop).unwrap();
    done.recv_timeout(Duration::from_millis(500))
        .expect("remote Stop must not wait for the two-second reconnect timer");
    worker.join().unwrap();
}
