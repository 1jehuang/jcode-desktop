#[test]
fn interrupted_remote_session_recovers_again_after_transport_replacement() {
    let ui_id = remote::namespace("desktop", "same-id");
    let (commands, rx) = channel();
    let (tx, updates) = async_channel::unbounded();
    let (connections_tx, connections) = channel();
    let worker_id = ui_id.clone();
    let worker = std::thread::spawn(move || {
        session_worker_with_connector(worker_id, rx, UpdateSender(tx), None, |address| {
            assert_eq!(address.host.as_deref(), Some("desktop"));
            let (client, server) = Server::new();
            connections_tx.send(server).unwrap();
            Ok(client)
        })
    });
    for attempt in 0..2 {
        let server = connections.recv_timeout(Duration::from_secs(4)).unwrap();
        assert!(
            matches!(server.next(), ApiRequest::AttachSession { session_id } if session_id == "same-id")
        );
        // The daemon can volunteer recovery history before attach's state
        // reply, and before Desktop asks for its displayed transcript.
        server.event(ApiEvent::SessionRecovery {
            session_id: "same-id".into(),
            continuation_message: format!("continue interrupted attempt {attempt}"),
            reconnect_notice: None,
        });
        assert!(matches!(server.next(), ApiRequest::GetHistory { .. }));
        assert!(matches!(server.next(), ApiRequest::GetRuntimeInfo { .. }));
        assert!(
            matches!(server.next(), ApiRequest::SendMessage { session_id, content, system_reminder, .. }
            if session_id == "same-id" && content.is_empty() && system_reminder == Some(format!("continue interrupted attempt {attempt}")))
        );
        wait_update(&updates, |update| {
            matches!(update, Update::Event {
                session_id,
                event: ApiEvent::MessageAccepted { session_id: inner },
            } if session_id == &ui_id && inner == &ui_id)
        });
        if attempt == 0 {
            // No TurnDone: the old worker activity flag is still true here.
            server.disconnect();
            wait_update(&updates, |update| {
                matches!(update, Update::SessionLost { .. })
            });
        } else {
            commands.send(SessionCommand::Stop).unwrap();
            worker.join().unwrap();
            server.disconnect();
            return;
        }
    }
}
