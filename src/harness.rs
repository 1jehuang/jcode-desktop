//! Harness bridge: SDK connections on background threads, events fanned into
//! GPUI through a channel the workspace polls.
//!
//! The harness API attaches one session per connection (the bridge translates
//! to the daemon's subscribe protocol), so this bridge gives every panel its
//! own connection thread: attach, fetch history, stream events, and serve
//! that panel's commands. Session creation also uses a fresh connection each
//! time, because a connection re-serves its already-attached session.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jcode_sdk::{ApiEvent, ConnectOptions, JcodeClient, LaunchOptions, SessionInfo};

/// Updates flowing from the harness threads into the UI.
#[derive(Debug)]
pub enum Update {
    /// Connection lifecycle status line, shown until connected.
    Status(String),
    /// The runtime is up and reachable.
    Connected,
    /// The initial session list, fetched in the background.
    Sessions { sessions: Vec<SessionInfo> },
    /// A session was created (in reply to `Command::CreateSession`).
    SessionCreated { session: SessionInfo },
    /// History fetched for a session after attach.
    History {
        session_id: String,
        messages: Vec<jcode_sdk::HistoryMessage>,
    },
    /// A streaming event for one session.
    Event(ApiEvent),
    /// A per-session connection died.
    SessionLost { session_id: String, reason: String },
    /// The control connection died; the bridge will retry.
    Disconnected { reason: String },
}

/// Commands flowing from the UI into the bridge.
pub enum Command {
    CreateSession { working_dir: Option<String> },
    /// Open a dedicated connection for this session (attach + stream).
    Watch { session_id: String },
    /// Drop a session's dedicated connection.
    Unwatch { session_id: String },
    Send { session_id: String, content: String },
    Cancel { session_id: String },
}

enum SessionCommand {
    Send { content: String },
    Cancel,
    Stop,
}

#[derive(Clone)]
pub struct Bridge {
    commands: Sender<Command>,
    updates: Arc<Mutex<Receiver<Update>>>,
}

impl Bridge {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Drain every pending update without blocking.
    pub fn drain(&self) -> Vec<Update> {
        let mut out = Vec::new();
        if let Ok(receiver) = self.updates.lock() {
            while let Ok(update) = receiver.try_recv() {
                out.push(update);
            }
        }
        out
    }
}

/// Spawn the bridge. Returns immediately; connection happens on the thread.
pub fn spawn() -> Bridge {
    let (update_tx, update_rx) = channel::<Update>();
    let (command_tx, command_rx) = channel::<Command>();

    std::thread::Builder::new()
        .name("jcode-bridge".into())
        .spawn(move || run(update_tx, command_rx))
        .expect("spawn bridge thread");

    Bridge {
        commands: command_tx,
        updates: Arc::new(Mutex::new(update_rx)),
    }
}

fn connect(client_name: &str) -> jcode_sdk::Result<JcodeClient> {
    JcodeClient::connect(ConnectOptions {
        client_name: format!("jcode-desktop-{client_name}/{}", env!("CARGO_PKG_VERSION")),
        ensure_runtime: false,
        ..Default::default()
    })
}

fn run(updates: Sender<Update>, commands: Receiver<Command>) {
    // Ensure the runtime once; after that, per-session workers just dial.
    let _ = updates.send(Update::Status("starting jcode runtime...".into()));
    if let Err(error) = jcode_sdk::ensure_runtime(&LaunchOptions::default(), &|status| {
        let _ = updates.send(Update::Status(status.to_string()));
    }) {
        let _ = updates.send(Update::Disconnected {
            reason: error.to_string(),
        });
        return;
    }
    let _ = updates.send(Update::Connected);

    // The session list can be slow (the daemon serializes it behind live
    // sessions), so it must not gate startup: fetch it on its own connection
    // and deliver whenever it lands.
    {
        let updates = updates.clone();
        std::thread::Builder::new()
            .name("jcode-bridge-sessions".into())
            .spawn(move || {
                let Ok(client) = connect("sessions") else {
                    return;
                };
                if let Ok(sessions) = client.list_sessions() {
                    let sessions = sessions
                        .into_iter()
                        .filter(|s| !s.archived)
                        .collect::<Vec<_>>();
                    let _ = updates.send(Update::Sessions { sessions });
                }
            })
            .expect("spawn session list thread");
    }

    // Per-session workers, keyed by session id.
    let mut workers: HashMap<String, Sender<SessionCommand>> = HashMap::new();

    while let Ok(command) = commands.recv() {
        match command {
            Command::CreateSession { working_dir } => {
                // A fresh connection per creation: an existing connection
                // returns its already-attached session instead of a new one.
                let updates = updates.clone();
                std::thread::Builder::new()
                    .name("jcode-bridge-create".into())
                    .spawn(move || match connect("create") {
                        Ok(client) => match client.create_session(working_dir) {
                            Ok(session) => {
                                let _ = updates.send(Update::SessionCreated { session });
                            }
                            Err(error) => {
                                let _ = updates.send(Update::Status(format!(
                                    "create session failed: {error}"
                                )));
                            }
                        },
                        Err(error) => {
                            let _ = updates.send(Update::Status(format!(
                                "connect failed: {error}"
                            )));
                        }
                    })
                    .expect("spawn create thread");
            }
            Command::Watch { session_id } => {
                if workers.contains_key(&session_id) {
                    continue;
                }
                let (tx, rx) = channel::<SessionCommand>();
                workers.insert(session_id.clone(), tx);
                let updates = updates.clone();
                std::thread::Builder::new()
                    .name(format!("jcode-session-{session_id}"))
                    .spawn(move || session_worker(session_id, rx, updates))
                    .expect("spawn session worker");
            }
            Command::Unwatch { session_id } => {
                if let Some(worker) = workers.remove(&session_id) {
                    let _ = worker.send(SessionCommand::Stop);
                }
            }
            Command::Send {
                session_id,
                content,
            } => {
                if let Some(worker) = workers.get(&session_id) {
                    let _ = worker.send(SessionCommand::Send { content });
                }
            }
            Command::Cancel { session_id } => {
                if let Some(worker) = workers.get(&session_id) {
                    let _ = worker.send(SessionCommand::Cancel);
                }
            }
        }
    }
}

/// One session's dedicated connection: attach, history, events, commands.
fn session_worker(
    session_id: String,
    commands: Receiver<SessionCommand>,
    updates: Sender<Update>,
) {
    let lost = |reason: String| {
        let _ = updates.send(Update::SessionLost {
            session_id: session_id.clone(),
            reason,
        });
    };

    let client = match connect("panel") {
        Ok(client) => client,
        Err(error) => return lost(error.to_string()),
    };

    // Subscribe before attaching so events pushed during attach are kept.
    let events = client.events(None);

    if let Err(error) = client.attach_session(&session_id) {
        return lost(error.to_string());
    }

    // History on the same connection: cheap, and the panel needs it once.
    match client.get_history(&session_id) {
        Ok(messages) => {
            let _ = updates.send(Update::History {
                session_id: session_id.clone(),
                messages,
            });
        }
        Err(error) => {
            let _ = updates.send(Update::Status(format!("history failed: {error}")));
        }
    }

    // Command half on its own thread so a blocked send never stalls events.
    let command_client = client.clone();
    let command_session = session_id.clone();
    std::thread::Builder::new()
        .name(format!("jcode-session-cmd-{command_session}"))
        .spawn(move || {
            while let Ok(command) = commands.recv() {
                match command {
                    SessionCommand::Send { content } => {
                        let _ = command_client.send_message(
                            &command_session,
                            &content,
                            Vec::new(),
                            None,
                        );
                    }
                    SessionCommand::Cancel => {
                        let _ = command_client.cancel(&command_session);
                    }
                    SessionCommand::Stop => break,
                }
            }
            // Dropping the client here closes the shared connection and ends
            // the event loop below.
        })
        .expect("spawn session command thread");

    // Event loop: forward this session's stream to the UI.
    loop {
        match events.next_timeout(Duration::from_millis(200)) {
            Some(event) => {
                let _ = updates.send(Update::Event(event));
            }
            None => {
                if client.is_closed() {
                    return lost("connection closed".into());
                }
            }
        }
    }
}
