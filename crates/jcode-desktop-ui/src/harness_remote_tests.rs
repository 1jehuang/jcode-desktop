//! Real SDK socket-pair tests for the native SSH address routing boundary.
use super::*;
use jcode_sdk::api::{ApiRequest, ClientFrame, ServerFrame, read_frame, write_frame};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Transport(UnixStream);
impl jcode_sdk::Transport for Transport {
    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let writer = self.0.try_clone().unwrap();
        Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
    }
}

fn info(id: &str) -> SessionInfo {
    SessionInfo {
        session_id: id.into(),
        working_dir: Some("/remote/project".into()),
        title: Some("Remote test".into()),
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
    }
}

struct Server {
    requests: Receiver<ApiRequest>,
    writer: Arc<Mutex<UnixStream>>,
}
impl Server {
    fn new() -> (JcodeClient, Self) {
        let (socket, server) = UnixStream::pair().unwrap();
        let mut reader = BufReader::new(server.try_clone().unwrap());
        let writer = Arc::new(Mutex::new(server));
        let reply_writer = writer.clone();
        let (tx, requests) = channel();
        std::thread::spawn(move || {
            while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
                let event = match &frame.request {
                    ApiRequest::Hello { .. } => ApiEvent::HelloOk {
                        version: jcode_sdk::api::API_VERSION_MAJOR,
                        server: "remote-test".into(),
                        capabilities: vec![],
                    },
                    ApiRequest::Ping => ApiEvent::Pong,
                    ApiRequest::CreateSession { .. } | ApiRequest::AttachSession { .. } => {
                        ApiEvent::Attached {
                            session: info("same-id"),
                        }
                    }
                    ApiRequest::GetHistory { .. } => ApiEvent::History {
                        session_id: "same-id".into(),
                        messages: vec![],
                        images: vec![],
                    },
                    ApiRequest::GetRuntimeInfo { .. } => ApiEvent::RuntimeInfo {
                        session_id: "same-id".into(),
                        provider: None,
                        model: None,
                        routes: vec![],
                        reasoning_effort: None,
                    },
                    ApiRequest::ForkSession { .. } => ApiEvent::SessionForked {
                        session: info("fork-id"),
                    },
                    ApiRequest::Compact { .. } => ApiEvent::Compacted {
                        session_id: "same-id".into(),
                        message: "scheduled".into(),
                    },
                    _ => ApiEvent::Ok,
                };
                if !matches!(frame.request, ApiRequest::Hello { .. } | ApiRequest::Ping) {
                    let _ = tx.send(frame.request.clone());
                }
                if write_frame(
                    &mut *reply_writer.lock().unwrap(),
                    &ServerFrame::reply(frame.id, event),
                )
                .is_err()
                {
                    break;
                }
                if matches!(frame.request, ApiRequest::SendMessage { .. }) {
                    let _ = write_frame(
                        &mut *reply_writer.lock().unwrap(),
                        &ServerFrame::event(ApiEvent::MessageAccepted {
                            session_id: "same-id".into(),
                        }),
                    );
                }
            }
        });
        let client = JcodeClient::connect_with(
            Box::new(Transport(socket)),
            ConnectOptions {
                ensure_runtime: false,
                request_timeout: Some(Duration::from_secs(2)),
                ..Default::default()
            },
        )
        .unwrap();
        (client, Self { requests, writer })
    }
    fn next(&self) -> ApiRequest {
        self.requests
            .recv_timeout(Duration::from_secs(3))
            .expect("SDK request")
    }
    fn event(&self, event: ApiEvent) {
        write_frame(
            &mut *self.writer.lock().unwrap(),
            &ServerFrame::event(event),
        )
        .unwrap();
    }
    fn disconnect(&self) {
        self.writer
            .lock()
            .unwrap()
            .shutdown(std::net::Shutdown::Both)
            .unwrap();
    }
}

fn wait_update(
    updates: &async_channel::Receiver<Update>,
    predicate: impl Fn(&Update) -> bool,
) -> Update {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for UI update");
        if let Ok(update) = updates.try_recv() {
            assert!(
                !matches!(
                    update,
                    Update::CommandFailed { .. } | Update::SendFailed { .. }
                ),
                "{update:?}"
            );
            if predicate(&update) {
                return update;
            }
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn remote_worker_routes_every_native_operation_and_namespaces_outputs() {
    let ui_id = remote::namespace("user@desktop", "same-id");
    let (client, server) = Server::new();
    let (commands, rx) = channel();
    let (tx, updates) = async_channel::unbounded();
    let worker_id = ui_id.clone();
    let worker =
        std::thread::spawn(move || session_worker(worker_id, rx, UpdateSender(tx), Some(client)));
    wait_update(
        &updates,
        |u| matches!(u, Update::History { session_id, .. } if session_id == &ui_id),
    );
    assert!(
        matches!(server.next(), ApiRequest::GetHistory { session_id } if session_id == "same-id")
    );
    assert!(
        matches!(server.next(), ApiRequest::GetRuntimeInfo { session_id } if session_id == "same-id")
    );

    commands
        .send(SessionCommand::Send {
            content: "remote prompt".into(),
            images: vec![("image/png".into(), "bytes".into())],
        })
        .unwrap();
    assert!(
        matches!(server.next(), ApiRequest::SendMessage { session_id, content, images, .. }
        if session_id == "same-id" && content == "remote prompt" && images.len() == 1)
    );
    wait_update(&updates, |u| {
        matches!(u, Update::Event { session_id, event: ApiEvent::MessageAccepted { session_id: inner } }
        if session_id == &ui_id && inner == &ui_id)
    });
    commands
        .send(SessionCommand::Send {
            content: "steering".into(),
            images: vec![],
        })
        .unwrap();
    assert!(
        matches!(server.next(), ApiRequest::SoftInterrupt { session_id, urgent: true, .. } if session_id == "same-id")
    );

    commands.send(SessionCommand::Cancel).unwrap();
    commands
        .send(SessionCommand::SetModel("model".into()))
        .unwrap();
    commands
        .send(SessionCommand::Operation(SessionOperation::Clear))
        .unwrap();
    commands
        .send(SessionCommand::Operation(SessionOperation::Compact))
        .unwrap();
    commands
        .send(SessionCommand::Operation(SessionOperation::SetEffort(
            "high".into(),
        )))
        .unwrap();
    commands
        .send(SessionCommand::Operation(SessionOperation::Rename(Some(
            "name".into(),
        ))))
        .unwrap();
    commands
        .send(SessionCommand::Operation(SessionOperation::Rewind(2)))
        .unwrap();
    commands
        .send(SessionCommand::Operation(SessionOperation::RewindUndo))
        .unwrap();
    commands.send(SessionCommand::Fork).unwrap();
    for expected in [
        "cancel",
        "set_model",
        "clear",
        "compact",
        "set_reasoning_effort",
        "rename_session",
        "rewind",
        "rewind_undo",
        "fork_session",
    ] {
        let request = serde_json::to_value(server.next()).unwrap();
        assert_eq!(request["session_id"], "same-id", "{expected}: {request}");
        assert_eq!(request["req"], expected, "{request}");
    }
    wait_update(
        &updates,
        |u| matches!(u, Update::SessionForked { session } if session.session_id == remote::namespace("user@desktop", "fork-id")),
    );
    // A different remote session must not leak into this panel, even for images.
    server.event(ApiEvent::SidePaneImages {
        session_id: "other-id".into(),
        images: vec![],
    });
    server.event(ApiEvent::TextDelta {
        session_id: "same-id".into(),
        text: "answer".into(),
    });
    wait_update(&updates, |u| {
        assert!(!matches!(
            u,
            Update::Event {
                event: ApiEvent::SidePaneImages { .. },
                ..
            }
        ));
        matches!(u, Update::Event { session_id, event: ApiEvent::TextDelta { session_id: inner, .. } } if session_id == &ui_id && inner == &ui_id)
    });
    server.event(ApiEvent::TurnDone {
        session_id: "same-id".into(),
    });
    wait_update(&updates, |u| {
        matches!(
            u,
            Update::Event {
                event: ApiEvent::TurnDone { .. },
                ..
            }
        )
    });
    commands.send(SessionCommand::Stop).unwrap();
    worker.join().unwrap();
    assert!(
        matches!(server.next(), ApiRequest::DetachSession { session_id } if session_id == "same-id")
    );
    server.disconnect();
}

#[test]
fn restored_remote_address_reconnects_to_same_host_without_blocking_local_worker() {
    let ui_id = remote::namespace("desktop", "same-id");
    let (commands, rx) = channel();
    let (tx, updates) = async_channel::unbounded();
    let (connections_tx, connections) = channel();
    let worker_id = ui_id.clone();
    let worker = std::thread::spawn(move || {
        session_worker_with_connector(worker_id, rx, UpdateSender(tx), None, |address| {
            assert_eq!(address.host.as_deref(), Some("desktop"));
            assert_eq!(address.session_id, "same-id");
            let (client, server) = Server::new();
            connections_tx.send(server).unwrap();
            Ok(client)
        })
    });
    let server = connections.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        matches!(server.next(), ApiRequest::AttachSession { session_id } if session_id == "same-id")
    );
    wait_update(
        &updates,
        |u| matches!(u, Update::History { session_id, .. } if session_id == &ui_id),
    );
    server.disconnect();
    wait_update(
        &updates,
        |u| matches!(u, Update::SessionLost { session_id, .. } if session_id == &ui_id),
    );

    // The same raw ID on the local host remains independent and responsive.
    let (local_client, local_server) = Server::new();
    let (local_commands, local_rx) = channel();
    let (local_tx, local_updates) = async_channel::unbounded();
    let local = std::thread::spawn(move || {
        session_worker(
            "same-id".into(),
            local_rx,
            UpdateSender(local_tx),
            Some(local_client),
        )
    });
    wait_update(
        &local_updates,
        |u| matches!(u, Update::History { session_id, .. } if session_id == "same-id"),
    );
    let replacement = connections.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        matches!(replacement.next(), ApiRequest::AttachSession { session_id } if session_id == "same-id")
    );
    wait_update(
        &updates,
        |u| matches!(u, Update::SessionConnected { session_id } if session_id == &ui_id),
    );
    commands.send(SessionCommand::Stop).unwrap();
    local_commands.send(SessionCommand::Stop).unwrap();
    worker.join().unwrap();
    local.join().unwrap();
    replacement.disconnect();
    local_server.disconnect();
}

#[test]
fn remote_metadata_never_reads_matching_local_files() {
    let home = tempfile::tempdir().unwrap();
    let id = remote::namespace("desktop", "same-id");
    // Deliberately populate the exact path a naive home.join(id) would read.
    let path = home.path().join("sessions").join(format!("{id}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, r#"{"status":"crashed"}"#).unwrap();
    let todos = home.path().join("todos").join(format!("{id}.json"));
    std::fs::create_dir_all(todos.parent().unwrap()).unwrap();
    std::fs::write(
        todos,
        r#"[{"content":"LOCAL SECRET","status":"in_progress"}]"#,
    )
    .unwrap();
    assert_eq!(persisted_todo_title(home.path(), &id), None);
    let merged = merge_persisted_sessions(vec![info(&id)], Some(home.path()));
    let remote = merged
        .iter()
        .find(|session| session.session_id == id)
        .unwrap();
    assert_eq!(remote.status, "idle");
    assert_ne!(remote.title.as_deref(), Some("LOCAL SECRET"));
}

#[test]
fn remote_create_preserves_request_id_and_sends_working_dir_only_as_json() {
    let (client, server) = Server::new();
    let (internal, created) = channel();
    let (tx, updates) = async_channel::unbounded();
    let cwd = "/remote/space dir/$(touch never);'quoted'";
    create_remote_session(
        " user@desktop ".into(),
        Some(cwd.into()),
        Some("draft-id".into()),
        UpdateSender(tx),
        internal,
        |host| {
            assert_eq!(host, "user@desktop");
            Ok(client)
        },
    );
    assert!(
        matches!(server.next(), ApiRequest::CreateSession { working_dir, .. }
        if working_dir.as_deref() == Some(cwd))
    );
    match created.recv_timeout(Duration::from_secs(2)).unwrap() {
        Command::CreatedInternal {
            session,
            request_id,
            ..
        } => {
            assert_eq!(
                session.session_id,
                remote::namespace("user@desktop", "same-id")
            );
            assert_eq!(request_id.as_deref(), Some("draft-id"));
        }
        _ => panic!("wrong create handoff"),
    }
    wait_update(&updates, |update| {
        matches!(update, Update::RemoteStatus { host, message, .. }
        if host == "user@desktop" && message.starts_with("Connected"))
    });
    server.disconnect();
}

#[test]
fn remote_create_failure_is_visible_and_never_retries_a_startup_draft() {
    let calls = Cell::new(0);
    let (internal, created) = channel();
    let (tx, updates) = async_channel::unbounded();
    create_remote_session(
        "desktop".into(),
        None,
        Some("draft-id".into()),
        UpdateSender(tx),
        internal,
        |_| {
            calls.set(calls.get() + 1);
            Err(jcode_sdk::Error::new(
                jcode_sdk::ErrorKind::Timeout,
                "SSH handshake timed out",
            ))
        },
    );
    assert_eq!(calls.get(), 1);
    assert!(created.try_recv().is_err());
    wait_update(&updates, |update| {
        matches!(update, Update::RemoteStatus { host, message, .. }
        if host == "desktop" && message.contains("SSH handshake timed out"))
    });
}

#[test]
fn invalid_remote_create_host_never_starts_a_transport() {
    let (internal, created) = channel();
    let (tx, updates) = async_channel::unbounded();
    create_remote_session(
        "-oProxyCommand=evil".into(),
        None,
        None,
        UpdateSender(tx),
        internal,
        |_| panic!("invalid host reached transport"),
    );
    assert!(created.try_recv().is_err());
    wait_update(&updates, |update| {
        matches!(update, Update::RemoteStatus { message, .. }
        if message.starts_with("Remote session failed:"))
    });
}

#[test]
fn last_bridge_handle_stops_its_coordinator_for_ssh_cleanup() {
    let (bridge, commands) = spawn_recording();
    let second = bridge.clone();
    drop(bridge);
    assert!(commands.try_recv().is_err());
    drop(second);
    assert!(matches!(
        commands.recv_timeout(Duration::from_secs(1)),
        Ok(Command::Shutdown)
    ));
}
