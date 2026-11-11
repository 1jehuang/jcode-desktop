#[test]
fn malformed_protocol_frame_stops_worker_without_reconnect_or_prompt_replay() {
    for ui_id in [
        "same-id".to_owned(),
        remote::namespace("desktop", "same-id"),
    ] {
        let (client, server) = Server::new();
        let (_commands, rx) = channel();
        let (tx, updates) = update_channel();
        let (done_tx, done) = channel();
        let worker_id = ui_id.clone();
        let worker = std::thread::spawn(move || {
            session_worker_with_connector(worker_id, rx, tx, Some(client), |_| {
                panic!("corrupt protocol must not enter a reconnect loop")
            });
            done_tx.send(()).unwrap();
        });
        assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
        assert!(matches!(server.next(), ApiRequest::GetRuntimeInfo { .. }));
        {
            let mut writer = server.writer.lock().unwrap();
            writer.write_all(b"corrupt frame\n").unwrap();
            writer.flush().unwrap();
        }
        done.recv_timeout(Duration::from_secs(2))
            .expect("fatal protocol error must stop promptly");
        worker.join().unwrap();
        let mut reported = false;
        while let Ok(update) = updates.try_recv() {
            if let Update::CommandFailed { session_id, reason } = update {
                assert_eq!(session_id, ui_id);
                assert!(reason.contains("protocol") && reason.contains("reconnect stopped"));
                reported = true;
            }
        }
        assert!(reported, "fatal protocol error must be actionable");
        server.disconnect();
    }
}

#[test]
fn protocol_failure_during_connect_stops_after_one_attempt() {
    let (_commands, rx) = channel();
    let (tx, updates) = update_channel();
    let mut attempts = 0;
    session_worker_with_connector("same-id".into(), rx, tx, None, |_| {
        attempts += 1;
        assert_eq!(
            attempts, 1,
            "permanent handshake corruption must not reconnect"
        );
        Err(jcode_sdk::Error::new(
            jcode_sdk::ErrorKind::Protocol,
            "invalid handshake frame",
        ))
    });
    assert_eq!(attempts, 1);
    assert!(matches!(updates.try_recv(), Ok(Update::SessionLost { .. })));
    assert!(matches!(
        updates.try_recv(),
        Ok(Update::CommandFailed { .. })
    ));
}
