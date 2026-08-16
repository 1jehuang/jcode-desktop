//! Harness bridge: SDK connections on background threads, events fanned into
//! GPUI through a channel the workspace polls.
//!
//! The harness API attaches one session per connection (the bridge translates
//! to the daemon's subscribe protocol), so this bridge gives every panel its
//! own connection thread: attach, fetch history, stream events, and serve
//! that panel's commands. Session creation also uses a fresh connection each
//! time, because a connection re-serves its already-attached session.

use std::collections::{HashMap, VecDeque};
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
    /// A streaming event for one session. The worker supplies the session id
    /// because some important events (notably errors) do not include one.
    Event { session_id: String, event: ApiEvent },
    /// Sending a message failed before the harness accepted it.
    SendFailed { session_id: String, reason: String },
    /// A per-session connection died.
    SessionLost { session_id: String, reason: String },
    /// The control connection died; the bridge will retry.
    Disconnected { reason: String },
}

/// Commands flowing from the UI into the bridge.
pub enum Command {
    CreateSession {
        working_dir: Option<String>,
    },
    /// Open a dedicated connection for this session (attach + stream).
    Watch {
        session_id: String,
    },
    /// Drop a session's dedicated connection.
    Unwatch {
        session_id: String,
    },
    Send {
        session_id: String,
        content: String,
        images: Vec<(String, String)>,
    },
    Cancel {
        session_id: String,
    },
}

enum SessionCommand {
    Send { content: String, images: Vec<(String, String)> },
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

/// A bridge with no runtime behind it. Commands are accepted and dropped, so a
/// test can drive the UI without a jcode daemon.
#[cfg(test)]
pub fn spawn_inert() -> Bridge {
    let (_update_tx, update_rx) = channel::<Update>();
    let (command_tx, _command_rx) = channel::<Command>();
    // Leak the receiving ends: nothing should observe or service them, and the
    // senders must stay usable for the lifetime of the test.
    std::mem::forget(_command_rx);
    std::mem::forget(_update_tx);
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
    // A self-dev reload deliberately takes the runtime socket away for a short
    // time. Keep this bridge (and therefore the GPUI/Wayland process) alive
    // while it comes back instead of turning a transient failure into a dead
    // desktop window.
    loop {
        let _ = updates.send(Update::Status("starting jcode runtime...".into()));
        match jcode_sdk::ensure_runtime(&LaunchOptions::default(), &|status| {
            let _ = updates.send(Update::Status(status.to_string()));
        }) {
            Ok(()) => break,
            Err(error) => {
                let _ = updates.send(Update::Disconnected {
                    reason: format!("{error}; retrying"),
                });
                std::thread::sleep(Duration::from_millis(500));
            }
        }
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
                            let _ =
                                updates.send(Update::Status(format!("connect failed: {error}")));
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
                images,
            } => {
                if let Some(worker) = workers.get(&session_id) {
                    let _ = worker.send(SessionCommand::Send { content, images });
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
fn session_worker(session_id: String, commands: Receiver<SessionCommand>, updates: Sender<Update>) {
    let lost = |reason: String| {
        let _ = updates.send(Update::SessionLost {
            session_id: session_id.clone(),
            reason,
        });
    };

    let mut pending = VecDeque::new();
    // The harness rejects a second SendMessage while a turn is active. Keep the
    // activity bit in this worker so subsequent composer submissions use the
    // SDK's urgent soft-interrupt queue instead. That is the same "ASAP"
    // steering path used by the TUI.
    let mut turn_active = false;

    // Reconnect in this same worker. The window and workspace stay resident,
    // and history refreshes the panel after the replacement runtime is ready.
    loop {
        let client = match connect("panel") {
            Ok(client) => client,
            Err(error) => {
                lost(format!("{error}; reconnecting"));
                if collect_disconnected_commands(&commands, &mut pending) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
        };
        let events = client.events(None);
        if let Err(error) = client.attach_session(&session_id) {
            lost(format!("{error}; reconnecting"));
            std::thread::sleep(Duration::from_millis(300));
            continue;
        }

        if let Ok(messages) = client.get_history(&session_id) {
            let _ = updates.send(Update::History {
                session_id: session_id.clone(),
                messages,
            });
        }

        loop {
            while let Some(command) = pending.pop_front().or_else(|| commands.try_recv().ok()) {
                match command {
                    SessionCommand::Send { content, images } => {
                        let result = if turn_active && images.is_empty() {
                            client.soft_interrupt(&session_id, &content, true)
                        } else {
                            client.send_message(
                                &session_id,
                                &content,
                                images,
                                Some(Duration::from_secs(5)),
                            )
                        };
                        if let Err(error) = result {
                            let _ = updates.send(Update::SendFailed {
                                session_id: session_id.clone(),
                                reason: error.to_string(),
                            });
                        } else {
                            // Mark active immediately instead of waiting for a
                            // streamed status event, so two rapidly submitted
                            // prompts cannot both take the SendMessage path.
                            turn_active = true;
                        }
                    }
                    SessionCommand::Cancel => {
                        let _ = client.cancel(&session_id);
                    }
                    SessionCommand::Stop => return,
                }
            }

            if let Some(event) = events.next_timeout(Duration::from_millis(100)) {
                update_turn_activity(&event, &mut turn_active);
                let _ = updates.send(Update::Event {
                    session_id: session_id.clone(),
                    event,
                });
            } else if client.is_closed() {
                lost("runtime reloading; reconnecting".into());
                break;
            }
        }
    }
}

fn update_turn_activity(event: &ApiEvent, turn_active: &mut bool) {
    match event {
        ApiEvent::MessageAccepted { .. } => *turn_active = true,
        ApiEvent::TurnDone { .. } => *turn_active = false,
        ApiEvent::SessionStatus { status, .. } => *turn_active = status != "idle",
        _ => {}
    }
}

/// Retain user commands while the self-dev runtime is between processes.
/// Returns true when the panel was closed and the worker should stop.
fn collect_disconnected_commands(
    commands: &Receiver<SessionCommand>,
    pending: &mut VecDeque<SessionCommand>,
) -> bool {
    while let Ok(command) = commands.try_recv() {
        if matches!(command, SessionCommand::Stop) {
            return true;
        }
        pending.push_back(command);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_retained_while_the_runtime_is_disconnected() {
        let (tx, rx) = channel();
        tx.send(SessionCommand::Send {
            content: "hi".into(),
            images: Vec::new(),
        })
        .unwrap();
        tx.send(SessionCommand::Cancel).unwrap();
        let mut pending = VecDeque::new();

        assert!(!collect_disconnected_commands(&rx, &mut pending));
        assert_eq!(pending.len(), 2);
        assert!(
            matches!(pending.pop_front(), Some(SessionCommand::Send { content, .. }) if content == "hi")
        );
        assert!(matches!(pending.pop_front(), Some(SessionCommand::Cancel)));
    }

    #[test]
    fn closing_a_disconnected_panel_stops_its_worker() {
        let (tx, rx) = channel();
        tx.send(SessionCommand::Stop).unwrap();
        let mut pending = VecDeque::new();

        assert!(collect_disconnected_commands(&rx, &mut pending));
        assert!(pending.is_empty());
    }

    #[test]
    fn turn_activity_tracks_acceptance_completion_and_session_status() {
        let mut active = false;
        update_turn_activity(
            &ApiEvent::MessageAccepted {
                session_id: "s1".into(),
            },
            &mut active,
        );
        assert!(active);

        update_turn_activity(
            &ApiEvent::SessionStatus {
                session_id: "s1".into(),
                status: "idle".into(),
            },
            &mut active,
        );
        assert!(!active);
    }
}
