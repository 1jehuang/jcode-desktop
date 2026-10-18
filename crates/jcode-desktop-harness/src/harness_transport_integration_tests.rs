//! Exercise the real bridge command loop and SDK clients with socket pairs.
//! Only SSH authentication is substituted. The pool itself has separate tests.
use super::*;
use jcode_sdk::api::{ApiRequest, ClientFrame, ServerFrame, read_frame, write_frame};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct SocketTransport(UnixStream);
impl jcode_sdk::Transport for SocketTransport {
    fn shutdown_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let socket = self.0.try_clone().ok()?;
        Some(Arc::new(move || {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }))
    }

    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let writer = self.0.try_clone().unwrap();
        Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
    }
}

fn session(id: &str) -> SessionInfo {
    serde_json::from_value(serde_json::json!({
        "session_id": id, "working_dir": "/remote", "title": "test", "status": "idle"
    }))
    .unwrap()
}

struct Fixture {
    pool: transport::RemoteTransports,
    connections: Receiver<(String, usize, UnixStream)>,
    requests: Receiver<(usize, ApiRequest)>,
    closed: Receiver<usize>,
}

impl Fixture {
    fn new() -> Self {
        let (connections_tx, connections) = channel();
        let (requests_tx, requests) = channel();
        let (closed_tx, closed) = channel();
        let count = AtomicUsize::new(0);
        let pool = transport::RemoteTransports::testing(move |host| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            let (socket, server) = UnixStream::pair().unwrap();
            connections_tx
                .send((host.to_owned(), index, server.try_clone().unwrap()))
                .unwrap();
            let requests = requests_tx.clone();
            let closed = closed_tx.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(server.try_clone().unwrap());
                let mut writer = server;
                while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
                    let event = match &frame.request {
                        ApiRequest::Hello { .. } => ApiEvent::HelloOk {
                            version: jcode_sdk::api::API_VERSION_MAJOR,
                            server: "pool-integration-test".into(),
                            capabilities: vec!["runtime_commands".into()],
                        },
                        ApiRequest::Ping => ApiEvent::Pong,
                        ApiRequest::CreateSession { .. } => ApiEvent::Attached {
                            session: session(&format!("created-{index}")),
                        },
                        ApiRequest::AttachSession { session_id } => ApiEvent::Attached {
                            session: session(session_id),
                        },
                        ApiRequest::GetHistory { session_id } => ApiEvent::History {
                            session_id: session_id.clone(),
                            messages: vec![],
                            images: vec![],
                        },
                        ApiRequest::GetRuntimeInfo { session_id } => ApiEvent::RuntimeInfo {
                            session_id: session_id.clone(),
                            provider: None,
                            model: None,
                            routes: vec![],
                            reasoning_effort: None,
                            auth_method: None,
                        },
                        ApiRequest::RuntimeCommand {
                            session_id,
                            command,
                            ..
                        } => ApiEvent::RuntimeCommandResult {
                            session_id: session_id.clone(),
                            command: *command,
                            data: serde_json::json!({"connection": index}),
                            message: "Remote result".into(),
                        },
                        ApiRequest::ForkSession { .. } => ApiEvent::SessionForked {
                            session: session(&format!("fork-{index}")),
                        },
                        _ => ApiEvent::Ok,
                    };
                    if !matches!(frame.request, ApiRequest::Hello { .. } | ApiRequest::Ping) {
                        let _ = requests.send((index, frame.request));
                    }
                    if write_frame(&mut writer, &ServerFrame::reply(frame.id, event)).is_err() {
                        break;
                    }
                }
                let _ = closed.send(index);
            });
            JcodeClient::connect_with(
                Box::new(SocketTransport(socket)),
                ConnectOptions {
                    ensure_runtime: false,
                    request_timeout: Some(Duration::from_secs(1)),
                    ..Default::default()
                },
            )
        });
        Self {
            pool,
            connections,
            requests,
            closed,
        }
    }
}

fn bridge_loop(
    pool: transport::RemoteTransports,
) -> (Sender<Command>, UpdateReceiver, std::thread::JoinHandle<()>) {
    let (commands, rx) = channel();
    let (updates, updates_rx) = update_channel();
    let internal = commands.clone();
    let handle = std::thread::spawn(move || run_with_transports(updates, rx, internal, pool));
    (commands, updates_rx, handle)
}

fn until(updates: &UpdateReceiver, predicate: impl Fn(&Update) -> bool) -> Update {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        assert!(Instant::now() < deadline, "bridge update deadline");
        if let Ok(update) = updates.try_recv() {
            if predicate(&update) {
                return update;
            }
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn two_remote_creates_keep_request_correlation_and_adopt_without_extra_connections() {
    let fixture = Fixture::new();
    let (commands, updates, bridge) = bridge_loop(fixture.pool);
    for request in ["draft-one", "draft-two"] {
        commands
            .send(Command::CreateRemoteSession {
                host: "same-host".into(),
                working_dir: None,
                request_id: Some(request.into()),
            })
            .unwrap();
    }
    let mut created = std::collections::HashMap::new();
    let mut connected = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while created.len() < 2 || connected.len() < 2 {
        assert!(Instant::now() < deadline, "both creates must be adopted");
        match updates.try_recv() {
            Ok(Update::SessionCreated {
                session,
                request_id,
            }) => {
                created.insert(request_id.unwrap(), session.session_id);
            }
            Ok(Update::SessionConnected { session_id }) => {
                connected.insert(session_id);
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert!(created.contains_key("draft-one"));
    assert!(created.contains_key("draft-two"));
    assert_ne!(created["draft-one"], created["draft-two"]);
    assert!(created.values().all(|id| connected.contains(id)));
    let mut sockets = Vec::new();
    for _ in 0..2 {
        let (host, index, socket) = fixture
            .connections
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(host, "same-host");
        assert!(connected.contains(&remote::namespace(&host, &format!("created-{index}"))));
        sockets.push(socket);
    }
    assert!(
        fixture.connections.try_recv().is_err(),
        "adoption never opens another API client"
    );
    commands.send(Command::Shutdown).unwrap();
    bridge.join().unwrap();
    let creates = fixture
        .requests
        .try_iter()
        .filter(|(_, request)| matches!(request, ApiRequest::CreateSession { .. }))
        .count();
    assert_eq!(creates, 2, "one create on each independent API connection");
    drop(sockets);
}

#[test]
fn fork_prompt_adopts_same_host_child_and_preserves_parent() {
    let fixture = Fixture::new();
    let (commands, updates, bridge) = bridge_loop(fixture.pool);
    let parent_id = remote::namespace("fork-host", "parent");
    commands
        .send(Command::Watch {
            session_id: parent_id.clone(),
        })
        .unwrap();
    let (host, parent_index, parent_socket) = fixture
        .connections
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert_eq!(host, "fork-host");
    until(
        &updates,
        |update| matches!(update, Update::SessionConnected { session_id } if session_id == &parent_id),
    );
    commands
        .send(Command::ForkWithPrompt {
            session_id: parent_id.clone(),
            prompt: "child-only question".into(),
        })
        .unwrap();
    let (child_host, child_index, child_socket) = fixture
        .connections
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert_eq!(child_host, "fork-host");
    assert_ne!(parent_index, child_index);
    let child_native = format!("fork-{parent_index}");
    let child_id = remote::namespace("fork-host", &child_native);
    until(
        &updates,
        |update| matches!(update, Update::SessionForked { session } if session.session_id == child_id),
    );
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        assert!(Instant::now() < deadline, "child prompt was not submitted");
        let (index, request) = fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        if let ApiRequest::SendMessage {
            session_id,
            content,
            ..
        } = request
        {
            assert_eq!(index, child_index);
            assert_eq!(session_id, child_native);
            assert_eq!(content, "child-only question");
            break;
        }
    }
    commands
        .send(Command::Watch {
            session_id: child_id,
        })
        .unwrap();
    commands
        .send(Command::SetModel {
            session_id: parent_id,
            model: "parent-still-attached".into(),
        })
        .unwrap();
    loop {
        let (index, request) = fixture
            .requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        if let ApiRequest::SetModel { session_id, model } = request {
            assert_eq!(index, parent_index);
            assert_eq!(session_id, "parent");
            assert_eq!(model, "parent-still-attached");
            break;
        }
    }
    assert!(
        fixture.connections.try_recv().is_err(),
        "UI Watch must reuse adopted child"
    );
    commands.send(Command::Shutdown).unwrap();
    bridge.join().unwrap();
    drop((parent_socket, child_socket));
}

#[test]
fn every_remote_worker_spawn_path_and_adopted_reconnect_uses_run_pool() {
    let fixture = Fixture::new();
    let (commands, updates, bridge) = bridge_loop(fixture.pool);
    let id = |suffix: &str| remote::namespace("same-host", suffix);
    let commands_to_spawn = [
        Command::Watch {
            session_id: id("watch"),
        },
        Command::Send {
            session_id: id("send"),
            content: "hello".into(),
            images: vec![],
        },
        Command::SetModel {
            session_id: id("model"),
            model: "model".into(),
        },
        Command::RefreshRuntime {
            session_id: id("runtime"),
        },
        Command::RuntimeCommand {
            session_id: id("runtime-command"),
            command: jcode_sdk::api::RuntimeCommand::Info,
            args: vec![],
        },
        Command::SessionOperation {
            session_id: id("operation"),
            operation: SessionOperation::Clear,
        },
        Command::Fork {
            session_id: id("fork"),
        },
    ];
    let mut sockets = Vec::new();
    let mut live_connections = HashSet::new();
    let mut busy_connection = None;
    for (position, command) in commands_to_spawn.into_iter().enumerate() {
        commands.send(command).unwrap();
        let (host, index, socket) = fixture
            .connections
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(host, "same-host");
        live_connections.insert(index);
        if position == 1 {
            busy_connection = Some(index);
        }
        sockets.push(socket);
    }
    until(&updates, |update| {
        matches!(update, Update::Event { session_id, event: ApiEvent::RuntimeCommandResult { session_id: inner, message, .. } }
        if session_id == &id("runtime-command") && inner == session_id && message == "Remote result")
    });
    commands
        .send(Command::CreateRemoteSession {
            host: "same-host".into(),
            working_dir: None,
            request_id: Some("adopted-draft".into()),
        })
        .unwrap();
    let (_, original_index, original) = fixture
        .connections
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    let Update::SessionCreated {
        session,
        request_id,
    } = until(&updates, |update| {
        matches!(update, Update::SessionCreated { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(request_id.as_deref(), Some("adopted-draft"));
    assert_eq!(session.session_id, id(&format!("created-{original_index}")));
    until(
        &updates,
        |update| matches!(update, Update::SessionConnected { session_id } if session_id == &session.session_id),
    );
    assert!(
        fixture.connections.try_recv().is_err(),
        "adoption must not reconnect"
    );
    original.shutdown(std::net::Shutdown::Both).unwrap();
    let (host, replacement_index, replacement) = fixture
        .connections
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(host, "same-host");
    assert_ne!(original_index, replacement_index);
    live_connections.insert(replacement_index);
    until(
        &updates,
        |update| matches!(update, Update::SessionConnected { session_id } if session_id == &session.session_id),
    );
    commands.send(Command::Shutdown).unwrap();
    bridge.join().unwrap();
    let mut attached_replacement = false;
    let mut created_count = 0;
    // Busy workers intentionally preserve crash recovery instead of detaching.
    // Shutdown must still close all seven API clients, including the busy one.
    let mut detached = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(4);
    while detached.len() < 6 && Instant::now() < deadline {
        if let Ok((index, request)) = fixture.requests.recv_timeout(Duration::from_millis(50)) {
            match request {
                ApiRequest::CreateSession { .. } => created_count += 1,
                ApiRequest::AttachSession { session_id } if index == replacement_index => {
                    assert_eq!(session_id, format!("created-{original_index}"));
                    attached_replacement = true;
                }
                ApiRequest::DetachSession { .. } => {
                    detached.insert(index);
                }
                _ => {}
            }
        }
    }
    assert!(attached_replacement);
    assert_eq!(
        created_count, 1,
        "adopted reconnect must attach, never create"
    );
    assert_eq!(detached.len(), 6, "only idle workers explicitly detach");
    assert!(!detached.contains(&busy_connection.unwrap()));
    let mut closed = HashSet::new();
    while !live_connections.is_subset(&closed) && Instant::now() < deadline {
        if let Ok(index) = fixture.closed.recv_timeout(Duration::from_millis(50)) {
            closed.insert(index);
        }
    }
    assert!(
        live_connections.is_subset(&closed),
        "shutdown closes every worker API connection: live={live_connections:?}, closed={closed:?}"
    );
    drop((sockets, replacement));
}
