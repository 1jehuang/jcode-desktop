//! Harness bridge: SDK connections on background threads, events fanned into
//! GPUI through a channel the workspace polls.
//!
//! The harness API attaches one session per connection (the bridge translates
//! to the daemon's subscribe protocol), so this bridge gives every panel its
//! own connection thread: attach, fetch history, stream events, and serve
//! that panel's commands. Session creation also uses a fresh connection each
//! time, because a connection re-serves its already-attached session.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
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
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    /// A session was created (in reply to `Command::CreateSession`).
    SessionCreated {
        session: SessionInfo,
    },
    /// History fetched for a session after attach.
    History {
        session_id: String,
        messages: Vec<jcode_sdk::HistoryMessage>,
        images: Vec<jcode_sdk::RenderedImage>,
    },
    /// A streaming event for one session. The worker supplies the session id
    /// because some important events (notably errors) do not include one.
    Event {
        session_id: String,
        event: ApiEvent,
    },
    /// Sending a message failed before the harness accepted it.
    SendFailed {
        session_id: String,
        reason: String,
    },
    CommandFailed {
        session_id: String,
        reason: String,
    },
    /// A per-session connection died.
    SessionLost {
        session_id: String,
        reason: String,
    },
    /// A per-session connection was established again.
    SessionConnected {
        session_id: String,
    },
    /// The control connection died; the bridge will retry.
    Disconnected {
        reason: String,
    },
}

/// Commands flowing from the UI into the bridge.
pub enum Command {
    RefreshSessions,
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
    SetModel {
        session_id: String,
        model: String,
    },
    SessionOperation {
        session_id: String,
        operation: SessionOperation,
    },
}

#[derive(Clone, Debug)]
pub enum SessionOperation {
    Clear,
    Compact,
    SetEffort(String),
    Rename(Option<String>),
    Rewind(usize),
    RewindUndo,
}

enum SessionCommand {
    Send {
        content: String,
        images: Vec<(String, String)>,
    },
    Cancel,
    SetModel(String),
    Operation(SessionOperation),
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

/// A runtime-free bridge whose commands can be asserted by UI acceptance tests.
#[cfg(test)]
pub fn spawn_recording() -> (Bridge, Receiver<Command>) {
    let (_update_tx, update_rx) = channel::<Update>();
    let (command_tx, command_rx) = channel::<Command>();
    std::mem::forget(_update_tx);
    (
        Bridge {
            commands: command_tx,
            updates: Arc::new(Mutex::new(update_rx)),
        },
        command_rx,
    )
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
        let options = LaunchOptions {
            binary: Some(crate::platform::companion_executable("jcode")),
            ..Default::default()
        };
        match jcode_sdk::ensure_runtime(&options, &|status| {
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
    refresh_sessions(updates.clone());

    // Per-session workers, keyed by session id.
    let mut workers: HashMap<String, Sender<SessionCommand>> = HashMap::new();

    while let Ok(command) = commands.recv() {
        match command {
            Command::RefreshSessions => refresh_sessions(updates.clone()),
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
                ensure_session_worker(&mut workers, session_id, &updates);
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
                let command = SessionCommand::Send { content, images };
                send_to_session_worker(&mut workers, session_id, command, |session_id| {
                    spawn_session_worker(session_id, &updates)
                });
            }
            Command::Cancel { session_id } => {
                if let Some(worker) = workers.get(&session_id) {
                    let _ = worker.send(SessionCommand::Cancel);
                }
            }
            Command::SetModel { session_id, model } => {
                let command = SessionCommand::SetModel(model);
                send_to_session_worker(&mut workers, session_id, command, |session_id| {
                    spawn_session_worker(session_id, &updates)
                });
            }
            Command::SessionOperation {
                session_id,
                operation,
            } => {
                let command = SessionCommand::Operation(operation);
                send_to_session_worker(&mut workers, session_id, command, |session_id| {
                    spawn_session_worker(session_id, &updates)
                });
            }
        }
    }
}

fn refresh_sessions(updates: Sender<Update>) {
    std::thread::Builder::new()
        .name("jcode-bridge-sessions".into())
        .spawn(move || {
            let api_sessions = connect("sessions")
                .and_then(|client| client.list_sessions())
                .unwrap_or_default();
            let sessions = merge_persisted_sessions(api_sessions, jcode_home().as_deref());
            let _ = updates.send(Update::Sessions { sessions });
        })
        .expect("spawn session list thread");
}

#[derive(serde::Deserialize)]
struct PersistedSession {
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    custom_title: Option<String>,
}

fn jcode_home() -> Option<PathBuf> {
    std::env::var_os("JCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".jcode")))
}

/// Merge the API's live view with records on disk. This deliberately makes the
/// desktop resilient to an older already-running bridge that only reports
/// sessions created during its lifetime.
pub(crate) fn merge_persisted_sessions(
    mut sessions: Vec<SessionInfo>,
    home: Option<&Path>,
) -> Vec<SessionInfo> {
    sessions.retain(|session| !session.archived);
    let Some(home) = home else {
        return sessions;
    };

    let archived = std::fs::read_to_string(home.join("sdk-archive.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.get("sessions").and_then(|v| v.as_object()).cloned())
        .map(|entries| {
            entries
                .into_iter()
                .map(|(id, _)| id)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    sessions.retain(|session| !archived.contains(&session.session_id));

    let mut known = sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect::<HashSet<_>>();
    let mut modified_by_id = HashMap::new();
    let mut disk_sessions = Vec::new();
    if let Ok(entries) = std::fs::read_dir(home.join("sessions")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if archived.contains(id) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            modified_by_id.insert(id.to_string(), modified);
            if !known.insert(id.to_string()) {
                continue;
            }
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let Ok(record) = serde_json::from_reader::<_, PersistedSession>(file) else {
                continue;
            };
            let title = record
                .custom_title
                .filter(|title| !title.trim().is_empty())
                .or_else(|| record.title.filter(|title| !title.trim().is_empty()));
            disk_sessions.push((
                modified,
                SessionInfo {
                    session_id: id.to_string(),
                    working_dir: record.working_dir,
                    title,
                    status: "idle".into(),
                    transcript_bytes: entry.metadata().ok().map(|metadata| metadata.len()),
                    archived: false,
                    archived_at_ms: None,
                },
            ));
        }
    }
    sessions.extend(disk_sessions.into_iter().map(|(_, session)| session));
    sessions.sort_by_key(|session| {
        modified_by_id
            .get(&session.session_id)
            .copied()
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    sessions
}

fn ensure_session_worker(
    workers: &mut HashMap<String, Sender<SessionCommand>>,
    session_id: String,
    updates: &Sender<Update>,
) -> Sender<SessionCommand> {
    if let Some(worker) = workers.get(&session_id) {
        return worker.clone();
    }
    let tx = spawn_session_worker(session_id.clone(), updates);
    workers.insert(session_id, tx.clone());
    tx
}

fn spawn_session_worker(session_id: String, updates: &Sender<Update>) -> Sender<SessionCommand> {
    let (tx, rx) = channel::<SessionCommand>();
    let updates = updates.clone();
    std::thread::Builder::new()
        .name(format!("jcode-session-{session_id}"))
        .spawn(move || session_worker(session_id, rx, updates))
        .expect("spawn session worker");
    tx
}

fn send_to_session_worker<F>(
    workers: &mut HashMap<String, Sender<SessionCommand>>,
    session_id: String,
    command: SessionCommand,
    mut spawn: F,
) where
    F: FnMut(String) -> Sender<SessionCommand>,
{
    let worker = workers
        .entry(session_id.clone())
        .or_insert_with(|| spawn(session_id.clone()))
        .clone();
    if let Err(error) = worker.send(command) {
        // A worker can disconnect between the map lookup and send. Replace it
        // and retain the user's message instead of silently dropping it.
        let worker = spawn(session_id.clone());
        workers.insert(session_id, worker.clone());
        let _ = worker.send(error.0);
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
        let _ = updates.send(Update::SessionConnected {
            session_id: session_id.clone(),
        });

        if let Ok((messages, images)) = client.get_history_with_images(&session_id) {
            let _ = updates.send(Update::History {
                session_id: session_id.clone(),
                messages,
                images,
            });
        }

        // Identity for the panel's status footer: which model and provider are
        // serving this session, and through which credential route. Delivered
        // as a normal event so the panel has one place that absorbs identity,
        // whether it arrives by request (here) or unsolicited (model switches).
        if let Ok(info) = client.get_runtime_info(&session_id) {
            let _ = updates.send(Update::Event {
                session_id: session_id.clone(),
                event: ApiEvent::RuntimeInfo {
                    session_id: session_id.clone(),
                    provider: info.provider,
                    model: info.model,
                    routes: info.routes,
                    reasoning_effort: info.reasoning_effort,
                },
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
                    SessionCommand::SetModel(model) => {
                        if let Err(error) = client.set_model(&session_id, &model) {
                            let _ = updates.send(Update::CommandFailed {
                                session_id: session_id.clone(),
                                reason: format!("Failed to switch model: {error}"),
                            });
                        }
                    }
                    SessionCommand::Operation(operation) => {
                        let result = match operation {
                            SessionOperation::Clear => client.clear(&session_id),
                            SessionOperation::Compact => client.compact(&session_id).map(|_| ()),
                            SessionOperation::SetEffort(effort) => {
                                client.set_reasoning_effort(&session_id, &effort)
                            }
                            SessionOperation::Rename(title) => {
                                client.rename_session(&session_id, title)
                            }
                            SessionOperation::Rewind(index) => client.rewind(&session_id, index),
                            SessionOperation::RewindUndo => client.rewind_undo(&session_id),
                        };
                        if let Err(error) = result {
                            let _ = updates.send(Update::CommandFailed {
                                session_id: session_id.clone(),
                                reason: format!("Command failed: {error}"),
                            });
                        }
                    }
                    SessionCommand::Stop => return,
                }
            }

            if let Some(event) = events.next_timeout(Duration::from_millis(100)) {
                // The API bridge emits this event immediately before closing its
                // stream when the legacy daemon connection disappears. It is a
                // transport lifecycle notification, not a failed model turn.
                // Reconnect now rather than rendering a scary transcript error
                // and waiting for the socket reader to notice EOF separately.
                if is_daemon_connection_closed(&event) {
                    lost("runtime connection closed; reconnecting".into());
                    break;
                }
                // The API socket broadcasts streaming events for every live
                // session. A busy TUI session must not make a newly-created
                // desktop panel look busy: doing so routes its first prompt
                // through soft_interrupt, where it waits forever because that
                // new session has no active turn to interrupt.
                if event_session_id(&event).is_some_and(|id| id != session_id) {
                    continue;
                }
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

fn event_session_id(event: &ApiEvent) -> Option<&str> {
    match event {
        ApiEvent::TextDelta { session_id, .. }
        | ApiEvent::ReasoningDelta { session_id, .. }
        | ApiEvent::ReasoningDone { session_id, .. }
        | ApiEvent::ToolStart { session_id, .. }
        | ApiEvent::ToolInputDelta { session_id, .. }
        | ApiEvent::ToolExec { session_id, .. }
        | ApiEvent::ToolDone { session_id, .. }
        | ApiEvent::TokenUsage { session_id, .. }
        | ApiEvent::TurnDone { session_id }
        | ApiEvent::BackgroundProgress { session_id, .. }
        | ApiEvent::MessageAccepted { session_id }
        | ApiEvent::PermissionRequest { session_id, .. }
        | ApiEvent::SessionStatus { session_id, .. }
        | ApiEvent::ConnectionPhase { session_id, .. }
        | ApiEvent::ModelInfo { session_id, .. }
        | ApiEvent::Models { session_id, .. }
        | ApiEvent::RuntimeInfo { session_id, .. }
        | ApiEvent::FileContent { session_id, .. }
        | ApiEvent::Files { session_id, .. }
        | ApiEvent::TextMatches { session_id, .. }
        | ApiEvent::FileStatus { session_id, .. }
        | ApiEvent::Compacted { session_id, .. }
        | ApiEvent::SessionRenamed { session_id, .. }
        | ApiEvent::History { session_id, .. } => Some(session_id),
        _ => None,
    }
}

fn is_daemon_connection_closed(event: &ApiEvent) -> bool {
    matches!(
        event,
        ApiEvent::Error { message, .. }
            if message.eq_ignore_ascii_case("daemon connection closed")
    )
}

fn update_turn_activity(event: &ApiEvent, turn_active: &mut bool) {
    match event {
        ApiEvent::MessageAccepted { .. } => *turn_active = true,
        ApiEvent::TurnDone { .. } => *turn_active = false,
        // `attached` describes the transport, not a model turn. Treating every
        // non-idle status as active routed the first prompt in a fresh desktop
        // panel through `soft_interrupt`; with no turn to interrupt, the prompt
        // stayed queued forever and the panel showed only its local echo.
        ApiEvent::SessionStatus { status, .. } if status == "idle" => *turn_active = false,
        ApiEvent::SessionStatus { status, .. }
            if matches!(status.as_str(), "generating" | "running") =>
        {
            *turn_active = true;
        }
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
    use std::time::Instant;

    fn session_info(id: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            working_dir: None,
            title: None,
            status: "idle".into(),
            transcript_bytes: None,
            archived: false,
            archived_at_ms: None,
        }
    }

    #[test]
    fn persisted_sessions_fill_sidebar_in_recency_order_without_duplicates_or_archives() {
        let home = std::env::temp_dir().join(format!(
            "jcode-desktop-sessions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sessions_dir = home.join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("older.json"),
            r#"{"working_dir":"/old","title":"Old title"}"#,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(
            sessions_dir.join("newer.json"),
            r#"{"working_dir":"/new","title":"Generated","custom_title":"Latest title"}"#,
        )
        .unwrap();
        std::fs::write(sessions_dir.join("archived.json"), r#"{"title":"Hidden"}"#).unwrap();
        std::fs::write(sessions_dir.join("malformed.json"), "not json").unwrap();
        std::fs::write(
            home.join("sdk-archive.json"),
            r#"{"sessions":{"archived":123}}"#,
        )
        .unwrap();

        let merged = merge_persisted_sessions(vec![session_info("older")], Some(&home));
        let ids = merged
            .iter()
            .map(|session| session.session_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["older", "newer"]);
        assert_eq!(merged[1].title.as_deref(), Some("Latest title"));
        assert_eq!(merged[1].working_dir.as_deref(), Some("/new"));
        assert!(merged[1].transcript_bytes.is_some());

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn daemon_connection_closed_is_a_transport_event() {
        let event = ApiEvent::Error {
            code: jcode_sdk::api::ErrorCode::Internal,
            message: "daemon connection closed".into(),
        };
        assert!(is_daemon_connection_closed(&event));
    }

    #[test]
    fn session_event_identity_prevents_cross_session_activity() {
        let event = ApiEvent::SessionStatus {
            session_id: "other-session".into(),
            status: "generating".into(),
        };
        assert_eq!(event_session_id(&event), Some("other-session"));
        assert_ne!(event_session_id(&event), Some("this-session"));
    }

    /// Opt-in acceptance check against the real local runtime and configured
    /// model. Run with `cargo test live_prompt_round_trip -- --ignored`.
    #[test]
    #[ignore = "requires a configured model and makes a real model request"]
    fn live_prompt_round_trip() {
        let bridge = spawn();
        bridge.send(Command::CreateSession { working_dir: None });

        let deadline = Instant::now() + Duration::from_secs(120);
        let session_id = loop {
            assert!(
                Instant::now() < deadline,
                "runtime did not create a session"
            );
            if let Some(session_id) = bridge.drain().into_iter().find_map(|update| match update {
                Update::SessionCreated { session } => Some(session.session_id),
                _ => None,
            }) {
                break session_id;
            }
            std::thread::sleep(Duration::from_millis(50));
        };

        bridge.send(Command::Watch {
            session_id: session_id.clone(),
        });

        // Match the real UI: the user types after the panel says "attached".
        // Sending immediately after Watch used to race ahead of that status and
        // accidentally hide the first-prompt soft-interrupt bug.
        loop {
            assert!(
                Instant::now() < deadline,
                "panel never reached attached status"
            );
            let attached = bridge.drain().into_iter().any(|update| {
                matches!(
                    update,
                    Update::Event {
                        session_id: ref event_session,
                        event: ApiEvent::SessionStatus { ref status, .. },
                    } if event_session == &session_id && status == "attached"
                )
            });
            if attached {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        bridge.send(Command::Send {
            session_id: session_id.clone(),
            content: "Reply with exactly JCODE_DESKTOP_OK and nothing else.".into(),
            images: Vec::new(),
        });

        let mut response = String::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "model response timed out; received {response:?}"
            );
            for update in bridge.drain() {
                match update {
                    Update::Event {
                        session_id: event_session,
                        event: ApiEvent::TextDelta { text, .. },
                    } if event_session == session_id => response.push_str(&text),
                    Update::SendFailed {
                        session_id: event_session,
                        reason,
                    } if event_session == session_id => panic!("send failed: {reason}"),
                    _ => {}
                }
            }
            if response.contains("JCODE_DESKTOP_OK") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn sending_without_a_watched_worker_starts_one_and_delivers() {
        let mut workers = HashMap::new();
        let (tx, rx) = channel();
        send_to_session_worker(
            &mut workers,
            "new-session".into(),
            SessionCommand::Send {
                content: "hello".into(),
                images: vec![],
            },
            |_| tx.clone(),
        );

        assert!(workers.contains_key("new-session"));
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(100)),
            Ok(SessionCommand::Send { content, .. }) if content == "hello"
        ));
    }

    #[test]
    fn sending_to_a_disconnected_worker_restarts_it_without_losing_the_message() {
        let mut workers = HashMap::new();
        let (stale_tx, stale_rx) = channel();
        drop(stale_rx);
        workers.insert("stale-session".into(), stale_tx);
        let (replacement_tx, replacement_rx) = channel();
        let mut starts = 0;

        send_to_session_worker(
            &mut workers,
            "stale-session".into(),
            SessionCommand::Send {
                content: "do not drop me".into(),
                images: vec![],
            },
            |_| {
                starts += 1;
                replacement_tx.clone()
            },
        );

        assert_eq!(starts, 1);
        assert!(matches!(
            replacement_rx.recv_timeout(Duration::from_millis(100)),
            Ok(SessionCommand::Send { content, .. }) if content == "do not drop me"
        ));
    }

    #[test]
    fn messages_are_retained_while_the_runtime_is_disconnected() {
        let (tx, rx) = channel();
        tx.send(SessionCommand::Send {
            content: "hi".into(),
            images: vec![("image/png".into(), "cG5n".into())],
        })
        .unwrap();
        tx.send(SessionCommand::Cancel).unwrap();
        let mut pending = VecDeque::new();

        assert!(!collect_disconnected_commands(&rx, &mut pending));
        assert_eq!(pending.len(), 2);
        assert!(matches!(
            pending.pop_front(),
            Some(SessionCommand::Send { content, images })
                if content == "hi" && images == [("image/png".into(), "cG5n".into())]
        ));
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
            &ApiEvent::SessionStatus {
                session_id: "s1".into(),
                status: "attached".into(),
            },
            &mut active,
        );
        assert!(!active, "attaching an idle session is not an active turn");

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
