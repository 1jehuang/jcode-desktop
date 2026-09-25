//! Harness bridge: SDK connections on background threads, events fanned into
//! GPUI through a channel the workspace polls.
//!
//! The harness API attaches one session per connection (the bridge translates
//! to the daemon's subscribe protocol), so this bridge gives every panel its
//! own connection thread: attach, fetch history, stream events, and serve
//! that panel's commands. Session creation also uses a fresh connection each
//! time, because a connection re-serves its already-attached session.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use jcode_sdk::{ApiEvent, ConnectOptions, JcodeClient, LaunchOptions, SessionInfo};

#[path = "harness_spawn.rs"]
mod spawn_profile;

#[path = "remote.rs"]
mod remote;

#[cfg(unix)]
#[path = "harness_transport.rs"]
mod transport;

// Shared OpenSSH control sockets are Unix-only. Other platforms retain the
// SDK's existing isolated transport behavior and security defaults.
#[cfg(not(unix))]
mod transport {
    #[derive(Clone, Default)]
    pub(super) struct RemoteTransports;

    impl RemoteTransports {
        pub(super) fn connect(&self, host: &str) -> jcode_sdk::Result<jcode_sdk::JcodeClient> {
            self.connect_with_progress(host, &mut |_| {})
        }

        pub(super) fn connect_with_progress(
            &self,
            host: &str,
            progress: &mut dyn FnMut(&str),
        ) -> jcode_sdk::Result<jcode_sdk::JcodeClient> {
            if host == crate::managed_cloud::HOST {
                return crate::managed_cloud::connect(
                    progress,
                    format!("jcode-desktop-remote/{}", crate::build_info::VERSION),
                    crate::managed_cloud::request_connect,
                    jcode_sdk::JcodeClient::connect_ssh,
                )
                .map_err(|message| {
                    jcode_sdk::Error::new(jcode_sdk::ErrorKind::ConnectFailed, message)
                });
            }
            jcode_sdk::JcodeClient::connect_ssh(jcode_sdk::SshConnectOptions {
                client_name: format!("jcode-desktop-remote/{}", crate::build_info::VERSION),
                connect_timeout: std::time::Duration::from_secs(20),
                request_timeout: Some(std::time::Duration::from_secs(30)),
                ..jcode_sdk::SshConnectOptions::new(host)
            })
        }
    }
}

#[path = "harness_recovery.rs"]
mod recovery;

/// Decode the host carried by a persisted remote panel ID.
pub fn remote_host(session_id: &str) -> Option<String> {
    remote::SessionAddress::parse(session_id).ok()?.host
}

const SIDEBAR_METADATA_WINDOW: usize = 64 * 1024;

/// Updates flowing from the harness threads into the UI.
#[derive(Debug)]
pub enum Update {
    /// Connection lifecycle status line, shown until connected.
    Status(String),
    /// Remote status persists independently of local runtime startup.
    RemoteStatus {
        host: String,
        message: String,
        request_id: Option<String>,
        failed: bool,
    },
    /// The runtime is up and reachable.
    Connected,
    /// The initial session list, fetched in the background.
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    /// A session was created (in reply to `Command::CreateSession`).
    SessionCreated {
        session: SessionInfo,
        /// Correlates the startup draft without consuming another creation.
        request_id: Option<String>,
    },
    /// A session was forked from an existing panel.
    SessionForked {
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
    /// The runtime accepted a submitted prompt on this session connection.
    MessageSubmitted {
        session_id: String,
    },
    CommandFailed {
        session_id: String,
        reason: String,
    },
    /// A reasoning effort request finished. `error` is the provider's refusal.
    EffortSettled {
        session_id: String,
        effort: String,
        error: Option<String>,
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
    /// Final UI handle dropped. Stop workers, including their SSH transports.
    Shutdown,
    RefreshSessions,
    RefreshRuntime {
        session_id: String,
    },
    CreateSession {
        working_dir: Option<String>,
        request_id: Option<String>,
    },
    /// Create a native session on an SSH host. Paths are sent in SDK JSON,
    /// never interpolated into an SSH command or interpreted locally.
    CreateRemoteSession {
        host: String,
        working_dir: Option<String>,
        request_id: Option<String>,
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
    Fork {
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
    /// Internal handoff from the asynchronous creator back to the bridge loop.
    CreatedInternal {
        session: SessionInfo,
        client: JcodeClient,
        request_id: Option<String>,
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
    RefreshRuntime,
    Send {
        content: String,
        images: Vec<(String, String)>,
    },
    Cancel,
    Fork,
    SetModel(String),
    Operation(SessionOperation),
    Stop,
}

#[derive(Clone)]
pub struct Bridge {
    _lifetime: std::sync::Arc<BridgeLifetime>,
    commands: Sender<Command>,
    updates: async_channel::Receiver<Update>,
}

struct BridgeLifetime(Sender<Command>);

impl Drop for BridgeLifetime {
    fn drop(&mut self) {
        // The coordinator retains an internal command sender for creation
        // handoffs, so channel disconnection alone cannot end its receive loop.
        let _ = self.0.send(Command::Shutdown);
    }
}

impl Bridge {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Drain every pending update without blocking (test helpers only).
    #[cfg(test)]
    pub fn drain(&self) -> Vec<Update> {
        self.drain_up_to(usize::MAX)
    }

    /// Leave excess updates queued so a busy producer cannot monopolize the UI.
    pub fn drain_up_to(&self, limit: usize) -> Vec<Update> {
        let mut out = Vec::new();
        while out.len() < limit {
            let Ok(update) = self.updates.try_recv() else {
                break;
            };
            out.push(update);
        }
        out
    }

    pub async fn recv(&self) -> Option<Update> {
        self.updates.recv().await.ok()
    }
}

#[derive(Clone)]
struct UpdateSender(async_channel::Sender<Update>);

impl UpdateSender {
    fn send(&self, update: Update) -> Result<(), async_channel::SendError<Update>> {
        self.0.send_blocking(update)
    }
}

/// Spawn the bridge. Returns immediately; connection happens on the thread.
pub fn spawn() -> Bridge {
    if screenshot_mode() {
        return spawn_inert();
    }
    let (update_tx, update_rx) = async_channel::unbounded::<Update>();
    let (command_tx, command_rx) = channel::<Command>();

    std::thread::Builder::new()
        .name("jcode-bridge".into())
        .spawn({
            let command_tx = command_tx.clone();
            move || run(UpdateSender(update_tx), command_rx, command_tx)
        })
        .expect("spawn bridge thread");

    Bridge {
        _lifetime: std::sync::Arc::new(BridgeLifetime(command_tx.clone())),
        commands: command_tx,
        updates: update_rx,
    }
}

/// A bridge with no runtime behind it. Commands are accepted and dropped, so a
/// test can drive the UI without a jcode daemon.
pub fn spawn_inert() -> Bridge {
    let (_update_tx, update_rx) = async_channel::unbounded::<Update>();
    let (command_tx, _command_rx) = channel::<Command>();
    // Leak the receiving ends: nothing should observe or service them, and the
    // senders must stay usable for the lifetime of the test.
    std::mem::forget(_command_rx);
    std::mem::forget(_update_tx);
    Bridge {
        _lifetime: std::sync::Arc::new(BridgeLifetime(command_tx.clone())),
        commands: command_tx,
        updates: update_rx,
    }
}

pub fn screenshot_mode() -> bool {
    std::env::var("JCODE_DESKTOP_SCREENSHOT").as_deref() == Ok("1")
}

/// A runtime-free bridge whose commands can be asserted by UI acceptance tests.
#[cfg(test)]
pub fn spawn_recording() -> (Bridge, Receiver<Command>) {
    let (_update_tx, update_rx) = async_channel::unbounded::<Update>();
    let (command_tx, command_rx) = channel::<Command>();
    std::mem::forget(_update_tx);
    (
        Bridge {
            _lifetime: std::sync::Arc::new(BridgeLifetime(command_tx.clone())),
            commands: command_tx,
            updates: update_rx,
        },
        command_rx,
    )
}

fn connect(client_name: &str) -> jcode_sdk::Result<JcodeClient> {
    let attempt = || {
        JcodeClient::connect(ConnectOptions {
            client_name: format!("jcode-desktop-{client_name}/{}", crate::build_info::VERSION),
            ensure_runtime: false,
            ..Default::default()
        })
    };
    match attempt() {
        Err(error) if error.kind == jcode_sdk::ErrorKind::ConnectFailed => {
            // The runtime was started once at bridge startup. If the harness
            // API bridge later dies (a self-dev restart interrupted halfway,
            // a crash), its socket file is left behind with nothing listening
            // and every panel retried the dead socket forever. Respawn the
            // runtime here instead. The bridge holds a single-instance lock,
            // so concurrent respawns from several panels are harmless, and the
            // throttle keeps a genuinely broken install from fork-looping.
            if !revive_local_runtime() {
                return Err(error);
            }
            attempt()
        }
        other => other,
    }
}

/// Minimum spacing between runtime respawn attempts from reconnect paths.
const RUNTIME_REVIVE_INTERVAL: Duration = Duration::from_secs(2);

/// Respawn the harness API bridge when the daemon is up but the bridge is not.
/// Returns whether a revive was attempted and succeeded.
///
/// A missing daemon is left alone on purpose: a self-dev reload takes the
/// daemon socket away for a moment, and spawning a competing `jcode serve`
/// from every reconnecting panel would fight the handoff.
fn revive_local_runtime() -> bool {
    if !jcode_sdk::socket_accepts(&jcode_sdk::api::legacy_socket_path()) {
        return false;
    }
    static LAST_ATTEMPT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    {
        let Ok(mut last) = LAST_ATTEMPT.lock() else {
            return false;
        };
        if !revive_due(*last, std::time::Instant::now()) {
            return false;
        }
        *last = Some(std::time::Instant::now());
    }
    let options = LaunchOptions {
        binary: Some(crate::platform::companion_executable("jcode")),
        ..Default::default()
    };
    match jcode_sdk::ensure_runtime(&options, &|_| {}) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("jcode desktop: could not revive local runtime: {error}");
            false
        }
    }
}

fn revive_due(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= RUNTIME_REVIVE_INTERVAL)
}

/// Gracefully detach idle attachments, including from older runtimes that use
/// the ownership flag for crash detection. Current runtimes classify abrupt
/// disconnects as crashes only when they interrupt an unfinished turn.
struct GracefulDetach<'a> {
    client: &'a JcodeClient,
    session_id: &'a str,
    processing: Cell<bool>,
}

impl GracefulDetach<'_> {
    fn set_processing(&self, processing: bool) {
        self.processing.set(processing);
    }
}

impl Drop for GracefulDetach<'_> {
    fn drop(&mut self) {
        // A desktop window disappearing during a live turn is an interruption,
        // not an idle detach. The runtime persists interrupted work as `crashed`;
        // startup restoration can then surface and resume it.
        if should_detach_on_drop(self.processing.get()) {
            let _ = self.client.detach_session(self.session_id);
        }
    }
}

fn should_detach_on_drop(processing: bool) -> bool {
    !processing
}

fn start_local_runtime(updates: UpdateSender) {
    // A self-dev reload deliberately takes the runtime socket away for a short
    // time. Keep this bridge (and therefore the GPUI/Wayland process) alive
    // while it comes back instead of turning a transient failure into a dead
    // desktop window.
    loop {
        if updates
            .send(Update::Status("starting jcode runtime...".into()))
            .is_err()
        {
            return;
        }
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
    // Neither persisted history scanning nor a busy daemon's list request
    // should delay creating the first interactive session.
    refresh_sessions(
        updates.clone(),
        true,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let _ = updates.send(Update::Connected);
}

fn run(updates: UpdateSender, commands: Receiver<Command>, internal: Sender<Command>) {
    // Local startup must not gate SSH work. In particular a broken/missing
    // local runtime cannot strand restored remote panels or remote creates.
    std::thread::Builder::new()
        .name("jcode-bridge-local-startup".into())
        .spawn({
            let updates = updates.clone();
            move || start_local_runtime(updates)
        })
        .expect("spawn local startup thread");
    // One pool per bridge run, never a process-global/dylib-lifetime singleton.
    run_with_transports(
        updates,
        commands,
        internal,
        transport::RemoteTransports::default(),
    );
}

fn run_with_transports(
    updates: UpdateSender,
    commands: Receiver<Command>,
    internal: Sender<Command>,
    transports: transport::RemoteTransports,
) {
    let session_refresh_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Per-session workers, keyed by session id.
    let mut workers: HashMap<String, Sender<SessionCommand>> = HashMap::new();

    while let Ok(command) = commands.recv() {
        match command {
            Command::Shutdown => break,
            Command::RefreshSessions => {
                refresh_sessions(updates.clone(), false, session_refresh_in_flight.clone())
            }
            Command::CreateSession {
                working_dir,
                request_id,
            } => {
                // A fresh connection per creation: an existing connection
                // returns its already-attached session instead of a new one.
                let updates = updates.clone();
                let internal = internal.clone();
                std::thread::Builder::new()
                    .name("jcode-bridge-create".into())
                    .spawn(move || {
                        loop {
                            let result =
                                spawn_profile::create(|| connect("create"), working_dir.clone());
                            match result {
                                Ok((session, client)) => {
                                    let _ = internal.send(Command::CreatedInternal {
                                        session,
                                        client,
                                        request_id,
                                    });
                                    break;
                                }
                                Err(error) => {
                                    if let Some(session_id) = &request_id
                                        && !spawn_profile::retry_startup(Some(session_id))
                                    {
                                        let _ = updates.send(Update::CommandFailed {
                                            session_id: session_id.clone(),
                                            reason: format!("Could not start session: {error}"),
                                        });
                                    }
                                    if updates
                                        .send(Update::Status(format!(
                                            "create session failed: {error}"
                                        )))
                                        .is_err()
                                        || !spawn_profile::retry_startup(request_id.as_deref())
                                    {
                                        break;
                                    }
                                    // Keep the startup draft usable across a daemon
                                    // restart without requiring a desktop restart.
                                    std::thread::sleep(Duration::from_millis(500));
                                }
                            }
                        }
                    })
                    .expect("spawn create thread");
            }
            Command::CreateRemoteSession {
                host,
                working_dir,
                request_id,
            } => {
                let updates = updates.clone();
                let internal = internal.clone();
                let transports = transports.clone();
                std::thread::Builder::new()
                    .name("jcode-bridge-ssh-create".into())
                    .spawn(move || {
                        let progress_updates = updates.clone();
                        let progress_host = host.clone();
                        let progress_request = request_id.clone();
                        create_remote_session(
                            host,
                            working_dir,
                            request_id,
                            updates,
                            internal,
                            move |host| {
                                transports.connect_with_progress(host, &mut |message| {
                                    let _ = progress_updates.send(Update::RemoteStatus {
                                        host: progress_host.clone(),
                                        message: message.to_owned(),
                                        request_id: progress_request.clone(),
                                        failed: false,
                                    });
                                })
                            },
                        );
                    })
                    .expect("spawn remote create thread");
            }
            Command::CreatedInternal {
                session,
                client,
                request_id,
            } => {
                let session_id = session.session_id.clone();
                eprintln!("jcode desktop: adopting created session {session_id}");
                // The adopted worker can report SessionConnected immediately.
                // Publish the panel first so readiness is not drained before the
                // UI has somewhere to apply it. This loop still installs the
                // worker before it can receive the UI's later Watch command.
                let _ = updates.send(Update::SessionCreated {
                    session,
                    request_id,
                });
                let worker = spawn_attached_session_worker(
                    session_id.clone(),
                    client,
                    &updates,
                    &transports,
                );
                if let Some(old) = workers.insert(session_id, worker) {
                    let _ = old.send(SessionCommand::Stop);
                }
            }
            Command::Watch { session_id } => {
                ensure_session_worker(&mut workers, session_id, &updates, &transports);
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
                    spawn_session_worker(session_id, &updates, &transports)
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
                    spawn_session_worker(session_id, &updates, &transports)
                });
            }
            Command::RefreshRuntime { session_id } => {
                send_to_session_worker(
                    &mut workers,
                    session_id,
                    SessionCommand::RefreshRuntime,
                    |session_id| spawn_session_worker(session_id, &updates, &transports),
                );
            }
            Command::SessionOperation {
                session_id,
                operation,
            } => {
                let command = SessionCommand::Operation(operation);
                send_to_session_worker(&mut workers, session_id, command, |session_id| {
                    spawn_session_worker(session_id, &updates, &transports)
                });
            }
            Command::Fork { session_id } => {
                send_to_session_worker(
                    &mut workers,
                    session_id,
                    SessionCommand::Fork,
                    |session_id| spawn_session_worker(session_id, &updates, &transports),
                );
            }
        }
    }
    stop_session_workers(workers);
}

fn stop_session_workers(workers: HashMap<String, Sender<SessionCommand>>) {
    for worker in workers.into_values() {
        let _ = worker.send(SessionCommand::Stop);
    }
}

/// A create is deliberately single-attempt, even for a startup request. An
/// ambiguous timeout must not produce extra sessions on a recovered host.
fn create_remote_session(
    host: String,
    working_dir: Option<String>,
    request_id: Option<String>,
    updates: UpdateSender,
    internal: Sender<Command>,
    connector: impl FnOnce(&str) -> jcode_sdk::Result<JcodeClient>,
) {
    let result = crate::remote_targets::validate_host(&host).and_then(|host| {
        let managed = host == crate::managed_cloud::HOST;
        let _ = updates.send(Update::RemoteStatus {
            host: host.clone(),
            message: if managed {
                "Checking your Jcode Cloud machine…".into()
            } else {
                format!("Connecting to {host} over SSH...")
            },
            request_id: request_id.clone(),
            failed: false,
        });
        let client = connector(&host).map_err(|error| error.to_string())?;
        let _ = updates.send(Update::RemoteStatus {
            host: host.clone(),
            message: if managed {
                "Connected. Creating Jcode session…".into()
            } else {
                "SSH connected. Creating Jcode session...".into()
            },
            request_id: request_id.clone(),
            failed: false,
        });
        let mut session = client
            .create_session(working_dir)
            .map_err(|error| error.to_string())?;
        session.session_id = remote::namespace(&host, &session.session_id);
        Ok((session, client))
    });
    match result {
        Ok((session, client)) => {
            let _ = updates.send(Update::RemoteStatus {
                host: host.trim().to_owned(),
                message: format!("Connected to {}", host.trim()),
                request_id: request_id.clone(),
                failed: false,
            });
            let _ = internal.send(Command::CreatedInternal {
                session,
                client,
                request_id,
            });
        }
        Err(error) => {
            let _ = updates.send(Update::RemoteStatus {
                host,
                message: format!("Remote session failed: {error}"),
                request_id,
                failed: true,
            });
        }
    }
}

fn refresh_sessions(
    updates: UpdateSender,
    include_disk_snapshot: bool,
    in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let home = jcode_home();
    // The workspace asks for this periodically so the sidebar follows sessions
    // created or changed by other Jcode processes. Coalesce requests while the
    // daemon is slow instead of accumulating 30-second list calls.
    if in_flight.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }

    std::thread::Builder::new()
        .name("jcode-bridge-sessions".into())
        .spawn(move || {
            if include_disk_snapshot {
                let started = std::time::Instant::now();
                let mut sessions = merge_persisted_sessions(Vec::new(), home.as_deref());
                jcode_sdk::enrich_sessions_from_local_swarm_state(&mut sessions);
                jcode_sdk::enrich_sessions_from_local_edit_stats(&mut sessions);
                eprintln!(
                    "jcode desktop: local session metadata loaded in {:.1}ms ({} sessions)",
                    started.elapsed().as_secs_f64() * 1_000.0,
                    sessions.len()
                );
                let _ = updates.send(Update::Sessions { sessions });
            }
            let started = std::time::Instant::now();
            let api_sessions =
                match connect("sessions").and_then(|client| client.list_sessions_limited(100)) {
                    Ok(sessions) => sessions,
                    Err(error) => {
                        eprintln!(
                            "jcode desktop: session list failed after {:.1}ms: {error}",
                            started.elapsed().as_secs_f64() * 1_000.0
                        );
                        Vec::new()
                    }
                };
            let mut sessions = merge_persisted_sessions(api_sessions, home.as_deref());
            jcode_sdk::enrich_sessions_from_local_swarm_state(&mut sessions);
            jcode_sdk::enrich_sessions_from_local_edit_stats(&mut sessions);
            eprintln!(
                "jcode desktop: session list completed in {:.1}ms ({} sessions)",
                started.elapsed().as_secs_f64() * 1_000.0,
                sessions.len()
            );
            let _ = updates.send(Update::Sessions { sessions });
            in_flight.store(false, std::sync::atomic::Ordering::Release);
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
    #[serde(default)]
    saved: bool,
    #[serde(default)]
    status: serde_json::Value,
}

fn persisted_session_status(status: &serde_json::Value) -> String {
    let name = status
        .as_str()
        .or_else(|| {
            status
                .as_object()
                .and_then(|status| status.keys().next().map(String::as_str))
        })
        .unwrap_or("idle");
    match name.to_ascii_lowercase().as_str() {
        "active" | "closed" | "crashed" | "reloaded" | "compacted" | "ratelimited"
        | "rate_limited" | "error" => name.to_ascii_lowercase(),
        _ => "idle".into(),
    }
}

#[derive(serde::Deserialize)]
struct PersistedTodoTitleItem {
    content: String,
    status: String,
    #[serde(default)]
    group: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnfinishedTodo {
    pub content: String,
    pub status: String,
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnfinishedSession {
    pub session_id: String,
    pub title: String,
    pub working_dir: Option<String>,
    pub todos: Vec<UnfinishedTodo>,
}

/// Read the durable todo snapshots for sessions which are no longer open.
/// This intentionally avoids transcripts, so opening the dashboard remains cheap.
pub fn unfinished_sessions(sessions: &[SessionInfo]) -> Vec<UnfinishedSession> {
    let Some(home) = jcode_home() else {
        return Vec::new();
    };
    let todos_dir = home.join("todos");
    sessions
        .iter()
        .filter(|session| !remote::is_remote(&session.session_id))
        .filter(|session| !matches!(session.status.as_str(), "active" | "running" | "working"))
        .filter_map(|session| {
            let todos: Vec<PersistedTodoTitleItem> =
                std::fs::read(todos_dir.join(format!("{}.json", session.session_id)))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
            let todos = todos
                .into_iter()
                .filter(|todo| !todo.status.eq_ignore_ascii_case("completed"))
                .map(|todo| UnfinishedTodo {
                    content: todo.content,
                    status: todo.status,
                    group: todo.group,
                })
                .collect::<Vec<_>>();
            (!todos.is_empty()).then(|| UnfinishedSession {
                session_id: session.session_id.clone(),
                title: session
                    .title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
                    .or_else(|| persisted_todo_title(&home, &session.session_id))
                    .unwrap_or_else(|| session.session_id.clone()),
                working_dir: session.working_dir.clone(),
                todos,
            })
        })
        .collect()
}

/// One session a live process owns, for the orchestration panel. Built from
/// presence markers and durable todo snapshots only, never from a transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSession {
    pub session_id: String,
    pub title: String,
    pub working_dir: Option<String>,
    /// A model turn is in flight right now.
    pub running: bool,
    pub todos: Vec<UnfinishedTodo>,
}

impl LiveSession {
    pub fn completed_todos(&self) -> usize {
        self.todos
            .iter()
            .filter(|todo| todo.status.eq_ignore_ascii_case("completed"))
            .count()
    }

    pub fn as_unfinished(&self) -> UnfinishedSession {
        UnfinishedSession {
            session_id: self.session_id.clone(),
            title: self.title.clone(),
            working_dir: self.working_dir.clone(),
            todos: self.todos.clone(),
        }
    }
}

/// Every local session whose owning process is alive. Running sessions sort
/// first, then the most recently created. Cheap enough to poll each second:
/// it reads presence markers, bounded session metadata and todo files.
pub fn live_sessions() -> Vec<LiveSession> {
    let Some(home) = jcode_home() else {
        return Vec::new();
    };
    let presence = jcode_base::session::session_presence()
        .into_iter()
        .map(|presence| (presence.session_id, presence.streaming));
    live_sessions_from(&home, presence)
}

pub(crate) fn live_sessions_from(
    home: &Path,
    presence: impl IntoIterator<Item = (String, bool)>,
) -> Vec<LiveSession> {
    let todos_dir = home.join("todos");
    let mut sessions = presence
        .into_iter()
        .filter(|(session_id, _)| !remote::is_remote(session_id))
        .map(|(session_id, running)| {
            let path = home.join("sessions").join(format!("{session_id}.json"));
            let record = std::fs::metadata(&path)
                .ok()
                .and_then(|metadata| read_persisted_session(&path, metadata.len()));
            let todos: Vec<PersistedTodoTitleItem> =
                std::fs::read(todos_dir.join(format!("{session_id}.json")))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default();
            let non_empty = |value: Option<String>| value.filter(|value| !value.trim().is_empty());
            let title = record
                .as_ref()
                .and_then(|record| {
                    non_empty(record.custom_title.clone())
                        .or_else(|| non_empty(record.title.clone()))
                })
                .or_else(|| persisted_todo_title(home, &session_id))
                .unwrap_or_else(|| session_id.clone());
            LiveSession {
                title,
                working_dir: record.and_then(|record| record.working_dir),
                running,
                todos: todos
                    .into_iter()
                    .map(|todo| UnfinishedTodo {
                        content: todo.content,
                        status: todo.status,
                        group: todo.group,
                    })
                    .collect(),
                session_id,
            }
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|a, b| {
        b.running
            .cmp(&a.running)
            .then_with(|| session_recency_ms(&b.session_id).cmp(&session_recency_ms(&a.session_id)))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    sessions
}

#[derive(serde::Deserialize, Default)]
struct PersistedTodoTitlePlan {
    #[serde(default)]
    user_intention: Option<String>,
}

/// Match the title precedence used by the TUI's `/resume` picker without
/// loading a transcript: current todo group, plan intention, then todo text.
fn persisted_todo_title(home: &Path, session_id: &str) -> Option<String> {
    if remote::is_remote(session_id) {
        return None;
    }
    let todos_dir = home.join("todos");
    let todos: Vec<PersistedTodoTitleItem> =
        std::fs::read(todos_dir.join(format!("{session_id}.json")))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
    let plan: PersistedTodoTitlePlan =
        std::fs::read(todos_dir.join(format!("{session_id}-plan.json")))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
    let current = todos
        .iter()
        .rev()
        .find(|todo| todo.status.eq_ignore_ascii_case("in_progress"))
        .or_else(|| {
            todos
                .iter()
                .rev()
                .find(|todo| !todo.status.eq_ignore_ascii_case("completed"))
        })
        .or_else(|| todos.last());
    let non_empty = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    current
        .and_then(|todo| non_empty(todo.group.as_deref()))
        .or_else(|| non_empty(plan.user_intention.as_deref()))
        .or_else(|| current.and_then(|todo| non_empty(Some(&todo.content))))
}

fn json_string_field(bytes: &[u8], field: &str, last: bool) -> Option<String> {
    let needle = format!("\"{field}\"");
    let mut starts = bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle.as_bytes()).then_some(index));
    let index = if last {
        starts.next_back()?
    } else {
        starts.next()?
    };
    let mut value = &bytes[index + needle.len()..];
    value = value.strip_prefix(b":")?.trim_ascii_start();
    serde_json::Deserializer::from_slice(value)
        .into_iter::<Option<String>>()
        .next()?
        .ok()
        .flatten()
}

/// Session transcripts can be hundreds of megabytes, while the sidebar only
/// needs fields stored before and after the messages array. Read small windows
/// from both ends instead of asking serde to walk every message.
fn read_persisted_session(path: &Path, bytes: u64) -> Option<PersistedSession> {
    use std::io::{Read, Seek, SeekFrom};

    if bytes <= (SIDEBAR_METADATA_WINDOW * 2) as u64 {
        return serde_json::from_reader(std::fs::File::open(path).ok()?).ok();
    }

    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0; SIDEBAR_METADATA_WINDOW];
    file.read_exact(&mut head).ok()?;
    file.seek(SeekFrom::End(-(SIDEBAR_METADATA_WINDOW as i64)))
        .ok()?;
    let mut tail = vec![0; SIDEBAR_METADATA_WINDOW];
    file.read_exact(&mut tail).ok()?;

    Some(PersistedSession {
        working_dir: json_string_field(&tail, "working_dir", true),
        title: json_string_field(&head, "title", false),
        custom_title: json_string_field(&head, "custom_title", false)
            .or_else(|| json_string_field(&tail, "custom_title", true)),
        saved: json_value_field(&tail, "saved", true)
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        status: json_value_field(&tail, "status", true).unwrap_or_default(),
    })
}

fn json_value_field(bytes: &[u8], field: &str, last: bool) -> Option<serde_json::Value> {
    let needle = format!("\"{field}\"");
    let mut starts = bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle.as_bytes()).then_some(index));
    let index = if last {
        starts.next_back()?
    } else {
        starts.next()?
    };
    let mut value = &bytes[index + needle.len()..];
    value = value.strip_prefix(b":")?.trim_ascii_start();
    serde_json::Deserializer::from_slice(value)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()
}

/// The sidebar is a recency view, not an archive browser. Session files include
/// the complete transcript, so parsing an unbounded store can mean reading many
/// gigabytes before the first row appears.
const MAX_PERSISTED_SIDEBAR_SESSIONS: usize = 100;

fn jcode_home() -> Option<PathBuf> {
    std::env::var_os("JCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".jcode")))
}

fn session_recency_ms(session_id: &str) -> Option<u128> {
    session_id
        .split('_')
        .filter_map(|part| part.parse::<u64>().ok())
        .find(|value| (1_000_000_000_000..10_000_000_000_000).contains(value))
        .map(u128::from)
}

fn file_recency_ms(entry: &std::fs::DirEntry) -> u128 {
    entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
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
    // The harness list API reports attachment state, not the durable lifecycle
    // state used by the TUI picker. Read the same bounded session records so a
    // crashed or errored session does not get flattened to a generic idle row.
    for session in &mut sessions {
        if remote::is_remote(&session.session_id) {
            continue;
        }
        let path = home
            .join("sessions")
            .join(format!("{}.json", session.session_id));
        let bytes = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
        if let Some(record) = read_persisted_session(&path, bytes.unwrap_or_default()) {
            session.status = persisted_session_status(&record.status);
        }
    }

    // A full limited API page is authoritative. Modern bridges source it from
    // the compact metadata index, so rescanning a 100k-file transcript directory
    // here would discard the entire latency win. Keep the disk walk only as a
    // compatibility fallback for old bridges that return a partial/empty list.
    if sessions.len() >= MAX_PERSISTED_SIDEBAR_SESSIONS {
        for session in &mut sessions {
            if session
                .title
                .as_ref()
                .is_none_or(|title| title.trim().is_empty())
            {
                session.title = persisted_todo_title(home, &session.session_id);
            }
        }
        sessions.reverse();
        return sessions;
    }

    let mut known = sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect::<HashSet<_>>();
    let mut modified_by_id = HashMap::new();
    let mut disk_candidates = Vec::new();
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
            // A transcript is rewritten whenever the session receives new
            // activity, so its modification time represents the user's latest
            // interaction. The timestamp embedded in the id is only a fallback
            // for stores where file metadata is unavailable.
            let modified = file_recency_ms(&entry);
            let recency = if modified == 0 {
                session_recency_ms(id).unwrap_or_default()
            } else {
                modified
            };
            modified_by_id.insert(id.to_string(), recency);
            if known.contains(id) {
                continue;
            }
            disk_candidates.push((recency, id.to_string(), path));
        }
    }

    // Select by cheap filesystem metadata first. Deserializing a session also
    // walks its `messages` array even though PersistedSession ignores that field.
    disk_candidates.sort_by_key(|(recency, ..)| std::cmp::Reverse(*recency));
    disk_candidates.truncate(MAX_PERSISTED_SIDEBAR_SESSIONS);

    let mut disk_sessions = Vec::new();
    for (recency, id, path) in disk_candidates {
        if !known.insert(id.clone()) {
            continue;
        }
        let transcript_bytes = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
        let Some(record) = read_persisted_session(&path, transcript_bytes.unwrap_or_default())
        else {
            continue;
        };
        let title = record
            .custom_title
            .filter(|title| !title.trim().is_empty())
            .or_else(|| persisted_todo_title(home, &id))
            .or_else(|| record.title.filter(|title| !title.trim().is_empty()));
        let status = persisted_session_status(&record.status);
        disk_sessions.push((
            recency,
            SessionInfo {
                session_id: id,
                working_dir: record.working_dir,
                title,
                status,
                transcript_bytes,
                saved: record.saved,
                updated_at_ms: i64::try_from(recency).ok(),
                last_active_at_ms: None,
                archived: false,
                archived_at_ms: None,
                save_label: None,
                parent_session_id: None,
                agent_label: None,
                swarm_status: None,
                edit_stats: None,
            },
        ));
    }
    sessions.extend(disk_sessions.into_iter().map(|(_, session)| session));
    for session in &mut sessions {
        if session
            .title
            .as_ref()
            .is_none_or(|title| title.trim().is_empty())
        {
            session.title = persisted_todo_title(home, &session.session_id);
        }
    }
    sessions.sort_by_key(|session| {
        modified_by_id
            .get(&session.session_id)
            .copied()
            .unwrap_or_else(|| session_recency_ms(&session.session_id).unwrap_or_default())
    });
    sessions
}

fn ensure_session_worker(
    workers: &mut HashMap<String, Sender<SessionCommand>>,
    session_id: String,
    updates: &UpdateSender,
    transports: &transport::RemoteTransports,
) -> Sender<SessionCommand> {
    if let Some(worker) = workers.get(&session_id) {
        return worker.clone();
    }
    let tx = spawn_session_worker(session_id.clone(), updates, transports);
    workers.insert(session_id, tx.clone());
    tx
}

fn spawn_session_worker(
    session_id: String,
    updates: &UpdateSender,
    transports: &transport::RemoteTransports,
) -> Sender<SessionCommand> {
    let (tx, rx) = channel::<SessionCommand>();
    let updates = updates.clone();
    let transports = transports.clone();
    std::thread::Builder::new()
        .name(format!("jcode-session-{session_id}"))
        .spawn(move || session_worker_with_transports(session_id, rx, updates, None, transports))
        .expect("spawn session worker");
    tx
}

fn spawn_attached_session_worker(
    session_id: String,
    client: JcodeClient,
    updates: &UpdateSender,
    transports: &transport::RemoteTransports,
) -> Sender<SessionCommand> {
    let (tx, rx) = channel::<SessionCommand>();
    let updates = updates.clone();
    let transports = transports.clone();
    std::thread::Builder::new()
        .name(format!("jcode-session-{session_id}"))
        .spawn(move || {
            session_worker_with_transports(session_id, rx, updates, Some(client), transports)
        })
        .expect("spawn attached session worker");
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

enum WorkerCommand {
    Command(SessionCommand),
    Idle,
    Disconnected,
}

fn next_worker_command(
    commands: &Receiver<SessionCommand>,
    pending: &mut VecDeque<SessionCommand>,
) -> WorkerCommand {
    if let Some(command) = pending.pop_front() {
        return WorkerCommand::Command(command);
    }
    match commands.try_recv() {
        Ok(command) => WorkerCommand::Command(command),
        Err(std::sync::mpsc::TryRecvError::Empty) => WorkerCommand::Idle,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => WorkerCommand::Disconnected,
    }
}

// Existing socket-pair tests need no live transport pool.
#[cfg(test)]
fn session_worker(
    session_id: String,
    commands: Receiver<SessionCommand>,
    updates: UpdateSender,
    initial_client: Option<JcodeClient>,
) {
    session_worker_with_transports(
        session_id,
        commands,
        updates,
        initial_client,
        transport::RemoteTransports::default(),
    );
}

/// One session's dedicated API connection, including an adopted client's reconnect.
fn session_worker_with_transports(
    session_id: String,
    commands: Receiver<SessionCommand>,
    updates: UpdateSender,
    initial_client: Option<JcodeClient>,
    transports: transport::RemoteTransports,
) {
    // Keep the shared master alive across this worker's API reconnect, but
    // never after the worker stops. Adoption preserves the creator's owner.
    #[cfg(unix)]
    let mut transport_lease = initial_client
        .as_ref()
        .and_then(JcodeClient::shared_ssh_transport);
    session_worker_with_connector(session_id, commands, updates, initial_client, |address| {
        match &address.host {
            Some(host) => {
                let result = transports.connect(host);
                #[cfg(unix)]
                {
                    transport_lease = result
                        .as_ref()
                        .ok()
                        .and_then(JcodeClient::shared_ssh_transport);
                }
                result
            }
            None => connect("panel"),
        }
    });
}

/// Longest wait between reconnect attempts for a session that keeps failing.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// Exponential backoff for a failing session reconnect.
///
/// A panel whose session the daemon no longer knows (`unknown_session`) used to
/// retry every 300 ms forever. Four such panels made ~40k attach attempts in an
/// hour, each one a fresh bridge connection with its own translation state and
/// daemon subscribe. Transport failures still retry promptly at first, so a
/// restarting runtime reconnects fast, but everything backs off to 30 s.
fn next_reconnect_delay(current: Duration, error: &jcode_sdk::Error) -> Duration {
    let unknown_session = matches!(
        error.kind,
        jcode_sdk::ErrorKind::Harness(jcode_sdk::api::ErrorCode::UnknownSession)
    );
    let next = if unknown_session {
        current.saturating_mul(4)
    } else {
        current.saturating_mul(2)
    };
    next.min(MAX_RECONNECT_DELAY)
}

/// Log the first few failures, then only occasionally, so a permanently
/// missing session cannot fill the diagnostics log.
fn should_log_reconnect(failed_attempts: u32) -> bool {
    failed_attempts < 3 || failed_attempts.is_power_of_two()
}

fn session_worker_with_connector(
    session_id: String,
    commands: Receiver<SessionCommand>,
    updates: UpdateSender,
    mut initial_client: Option<JcodeClient>,
    mut connector: impl FnMut(&remote::SessionAddress) -> jcode_sdk::Result<JcodeClient>,
) {
    let lost = |reason: String| {
        eprintln!("jcode desktop: session {session_id} lost: {reason}");
        let _ = updates.send(Update::SessionLost {
            session_id: session_id.clone(),
            reason,
        });
    };

    let address = match remote::SessionAddress::parse(&session_id) {
        Ok(address) => address,
        Err(error) => {
            let _ = updates.send(Update::Status(error.clone()));
            lost(error);
            return;
        }
    };
    let real_id = address.session_id.as_str();
    let base_reconnect_delay = if address.host.is_some() {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(300)
    };
    let mut reconnect_delay = base_reconnect_delay;
    let mut failed_attempts: u32 = 0;

    let mut pending = VecDeque::new();
    let mut reported_disconnected_events = false;

    // Reconnect in this same worker. The window and workspace stay resident,
    // and history refreshes the panel after the replacement runtime is ready.
    'reconnect: loop {
        if collect_disconnected_commands(&commands, &mut pending) {
            return;
        }
        let already_attached = initial_client.is_some();
        let client = match initial_client
            .take()
            .map(Ok)
            .unwrap_or_else(|| connector(&address))
        {
            Ok(client) => client,
            Err(error) => {
                if should_log_reconnect(failed_attempts) {
                    lost(format!("{error}; reconnecting"));
                }
                if collect_disconnected_commands(&commands, &mut pending) {
                    return;
                }
                failed_attempts = failed_attempts.saturating_add(1);
                match wait_for_reconnect(&commands, &mut pending, reconnect_delay) {
                    ReconnectWait::Stop => return,
                    ReconnectWait::Woken => reconnect_delay = base_reconnect_delay,
                    ReconnectWait::Elapsed => {
                        reconnect_delay = next_reconnect_delay(reconnect_delay, &error);
                    }
                }
                continue;
            }
        };
        let events = client.events(None);
        if !already_attached && let Err(error) = client.attach_session(real_id) {
            if should_log_reconnect(failed_attempts) {
                lost(format!("{error}; reconnecting"));
            }
            if collect_disconnected_commands(&commands, &mut pending) {
                return;
            }
            failed_attempts = failed_attempts.saturating_add(1);
            match wait_for_reconnect(&commands, &mut pending, reconnect_delay) {
                ReconnectWait::Stop => return,
                ReconnectWait::Woken => reconnect_delay = base_reconnect_delay,
                ReconnectWait::Elapsed => {
                    reconnect_delay = next_reconnect_delay(reconnect_delay, &error);
                }
            }
            continue;
        }
        reconnect_delay = base_reconnect_delay;
        failed_attempts = 0;
        let _detach = GracefulDetach {
            client: &client,
            session_id: real_id,
            processing: Cell::new(false),
        };
        // Activity from the dead transport must not suppress recovery on its
        // replacement. The new attachment supplies its own live state.
        let mut turn_active = false;
        let mut recovery = recovery::Recovery::default();
        // Acceptance and rejection belong to this transport. A lost ack on
        // the previous connection must not block server-directed recovery.
        // Explicit attachment-mismatch retries already live in `pending`.
        let mut unaccepted_sends = VecDeque::new();
        eprintln!("jcode desktop: session {session_id} connected");
        let _ = updates.send(Update::SessionConnected {
            session_id: session_id.clone(),
        });

        if let Ok((messages, images)) = client.get_history_with_images(real_id) {
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
        if let Ok(info) = client.get_runtime_info(real_id) {
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
            loop {
                let command = match next_worker_command(&commands, &mut pending) {
                    WorkerCommand::Command(command) => command,
                    WorkerCommand::Idle => break,
                    // Hot reload drops the old bridge and all of its command
                    // senders. Do not leave one busy event loop behind for
                    // every panel from every retained UI generation.
                    WorkerCommand::Disconnected => return,
                };
                match command {
                    SessionCommand::RefreshRuntime => {
                        if let Ok(info) = client.get_runtime_info(real_id) {
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
                    }
                    SessionCommand::Send { content, images } => {
                        recovery.supersede();
                        let retry_content = content.clone();
                        let retry_images = images.clone();
                        // Only one ordinary send can await acceptance. An old
                        // idle event must not let another send overwrite it.
                        let steering = turn_active || !unaccepted_sends.is_empty();
                        let result = if steering {
                            client.soft_interrupt_with_images(real_id, &content, images, true)
                        } else {
                            client.send_message(real_id, &content, images, None)
                        };
                        if let Err(error) = result {
                            // A daemon reload can briefly hand this SDK socket
                            // the state of another legacy subscription. Never
                            // surface that transport mix-up as a failed user
                            // message: replace the connection and retry the
                            // original submission on the correctly attached
                            // session worker.
                            if queue_wrong_session_retry(
                                &error.to_string(),
                                &mut pending,
                                retry_content,
                                retry_images,
                            ) {
                                lost("session connection changed; reconnecting".into());
                                continue 'reconnect;
                            }
                            let _ = updates.send(Update::SendFailed {
                                session_id: session_id.clone(),
                                reason: error.to_string(),
                            });
                        } else {
                            // Steering is acknowledged by the synchronous SDK
                            // reply, not a MessageAccepted stream event. Keeping
                            // it here would replay an already-delivered message
                            // when a later ordinary send gets rejected.
                            if !steering {
                                unaccepted_sends.push_back(SessionCommand::Send {
                                    content: retry_content,
                                    images: retry_images,
                                });
                            }
                            let _ = updates.send(Update::MessageSubmitted {
                                session_id: session_id.clone(),
                            });
                            // Mark active immediately instead of waiting for a
                            // streamed status event, so two rapidly submitted
                            // prompts cannot both take the SendMessage path.
                            turn_active = true;
                            _detach.set_processing(true);
                        }
                    }
                    SessionCommand::Cancel => {
                        recovery.supersede();
                        let _ = client.cancel(real_id);
                    }
                    SessionCommand::Fork => match client.fork_session(real_id) {
                        Ok(session) => {
                            let session = address.session_info(session);
                            let _ = updates.send(Update::SessionForked { session });
                        }
                        Err(error) => {
                            let _ = updates.send(Update::CommandFailed {
                                session_id: session_id.clone(),
                                reason: format!("Failed to fork session: {error}"),
                            });
                        }
                    },
                    SessionCommand::SetModel(model) => {
                        if let Err(error) = client.set_model(real_id, &model) {
                            let _ = updates.send(Update::CommandFailed {
                                session_id: session_id.clone(),
                                reason: format!("Failed to switch model: {error}"),
                            });
                        }
                    }
                    SessionCommand::Operation(operation) => {
                        if matches!(
                            operation,
                            SessionOperation::Clear
                                | SessionOperation::Rewind(_)
                                | SessionOperation::RewindUndo
                        ) {
                            recovery.supersede();
                        }
                        // Effort settles the panel's optimistic label either
                        // way, so its outcome is reported rather than only a
                        // failure. The identity broadcast that follows carries
                        // the level the provider actually applied.
                        if let SessionOperation::SetEffort(effort) = &operation {
                            let error = client
                                .set_reasoning_effort(real_id, effort)
                                .err()
                                .map(|error| error.to_string());
                            let _ = updates.send(Update::EffortSettled {
                                session_id: session_id.clone(),
                                effort: effort.clone(),
                                error,
                            });
                            continue;
                        }
                        let result = match operation {
                            SessionOperation::Clear => client.clear(real_id),
                            SessionOperation::Compact => client.compact(real_id).map(|_| ()),
                            SessionOperation::SetEffort(_) => Ok(()),
                            SessionOperation::Rename(title) => {
                                client.rename_session(real_id, title)
                            }
                            SessionOperation::Rewind(index) => client.rewind(real_id, index),
                            SessionOperation::RewindUndo => client.rewind_undo(real_id),
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

            let event_wait_started = std::time::Instant::now();
            if let Some(event) = events.next_timeout(Duration::from_millis(100)) {
                reported_disconnected_events = false;
                if recover_async_attachment_mismatch(&event, &mut unaccepted_sends, &mut pending) {
                    lost("session connection changed; reconnecting".into());
                    continue 'reconnect;
                }
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
                if event_session_id(&event).is_some_and(|id| id != real_id) {
                    continue;
                }
                if let ApiEvent::SessionRecovery {
                    continuation_message,
                    ..
                } = &event
                {
                    // This is the same server-owned directive used by the TUI,
                    // not a guess based on a sidebar's cached crash badge.
                    if recovery.claim(
                        continuation_message,
                        turn_active || !unaccepted_sends.is_empty(),
                    ) {
                        if let Err(error) =
                            client.send_system_reminder(real_id, continuation_message)
                        {
                            recovery.finish_submission();
                            let _ = updates.send(Update::CommandFailed {
                                session_id: session_id.clone(),
                                reason: format!("Failed to continue interrupted work: {error}"),
                            });
                        } else {
                            turn_active = true;
                            _detach.set_processing(true);
                            let _ = updates.send(Update::Event {
                                session_id: session_id.clone(),
                                event: ApiEvent::SessionStatus {
                                    session_id: session_id.clone(),
                                    status: "running".into(),
                                },
                            });
                        }
                    }
                    continue;
                }
                if recovery.observe(&event) {
                    // Another client already continued the session. Never
                    // steer a second automatic continuation into that turn.
                    turn_active = true;
                    _detach.set_processing(true);
                    continue;
                }
                if recover_async_busy(&event, &mut unaccepted_sends, &mut pending) {
                    // send_message is fire-and-forget, so its busy rejection
                    // arrives here, not in its Result. Replay the intact prompt
                    // through steering before accepting another composer send.
                    turn_active = true;
                    _detach.set_processing(true);
                    continue;
                }
                if matches!(event, ApiEvent::MessageAccepted { .. }) {
                    unaccepted_sends.pop_front();
                } else if matches!(event, ApiEvent::Error { .. }) {
                    // An unrelated terminal failure must not leave an old
                    // payload eligible for recovery on a later busy error.
                    unaccepted_sends.clear();
                }
                update_turn_activity(&event, &mut turn_active);
                _detach.set_processing(turn_active);
                let _ = updates.send(Update::Event {
                    session_id: session_id.clone(),
                    event: namespace_event(event, &address),
                });
            } else if client.is_closed() {
                lost("runtime connection closed; reconnecting".into());
                break;
            } else if event_wait_started.elapsed() < Duration::from_millis(10) {
                // `EventStream::next_timeout` also returns `None` when its
                // receiver disconnects. In that case it returns immediately,
                // and an otherwise healthy client can turn this loop into a
                // full-core spin. Keep command latency low while placing a
                // firm ceiling on CPU use until the transport reconnects.
                if !reported_disconnected_events {
                    eprintln!(
                        "jcode desktop: event stream for {session_id} returned immediately; backing off"
                    );
                    reported_disconnected_events = true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if address.host.is_some() {
            // A reachable SSH server whose API immediately closes should not
            // produce an unbounded rapid reconnect storm either.
            std::thread::sleep(reconnect_delay);
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
        | ApiEvent::SidePaneImages { session_id, .. }
        | ApiEvent::SidePanelState { session_id, .. }
        | ApiEvent::WakeRequested { session_id, .. }
        | ApiEvent::SessionRecovery { session_id, .. }
        | ApiEvent::TokenUsage { session_id, .. }
        | ApiEvent::TurnDone { session_id }
        | ApiEvent::TurnStopped { session_id, .. }
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

fn namespace_event(mut event: ApiEvent, address: &remote::SessionAddress) -> ApiEvent {
    if address.host.is_none() {
        return event;
    }
    match &mut event {
        ApiEvent::TextDelta { session_id, .. }
        | ApiEvent::ReasoningDelta { session_id, .. }
        | ApiEvent::ReasoningDone { session_id, .. }
        | ApiEvent::ToolStart { session_id, .. }
        | ApiEvent::ToolInputDelta { session_id, .. }
        | ApiEvent::ToolExec { session_id, .. }
        | ApiEvent::ToolDone { session_id, .. }
        | ApiEvent::SidePaneImages { session_id, .. }
        | ApiEvent::SidePanelState { session_id, .. }
        | ApiEvent::WakeRequested { session_id, .. }
        | ApiEvent::SessionRecovery { session_id, .. }
        | ApiEvent::TokenUsage { session_id, .. }
        | ApiEvent::TurnDone { session_id }
        | ApiEvent::TurnStopped { session_id, .. }
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
        | ApiEvent::History { session_id, .. } => *session_id = address.ui_id(session_id),
        ApiEvent::Attached { session } | ApiEvent::SessionForked { session } => {
            session.session_id = address.ui_id(&session.session_id);
        }
        ApiEvent::Sessions { sessions } => {
            for session in sessions {
                session.session_id = address.ui_id(&session.session_id);
            }
        }
        _ => {}
    }
    event
}

fn is_daemon_connection_closed(event: &ApiEvent) -> bool {
    matches!(
        event,
        ApiEvent::Error { message, .. }
            if message.eq_ignore_ascii_case("daemon connection closed")
    )
}

fn is_wrong_session_attachment_error(message: &str) -> bool {
    message.contains("this connection is attached to `")
        && message.contains("; attach to it first or use another connection")
}

fn recover_async_attachment_mismatch(
    event: &ApiEvent,
    unaccepted: &mut VecDeque<SessionCommand>,
    pending: &mut VecDeque<SessionCommand>,
) -> bool {
    let ApiEvent::Error { message, .. } = event else {
        return false;
    };
    if !is_wrong_session_attachment_error(message) {
        return false;
    }
    let Some(submission) = unaccepted.pop_front() else {
        return false;
    };
    pending.push_front(submission);
    true
}

fn queue_wrong_session_retry(
    error: &str,
    pending: &mut VecDeque<SessionCommand>,
    content: String,
    images: Vec<(String, String)>,
) -> bool {
    if !is_wrong_session_attachment_error(error) {
        return false;
    }
    pending.push_front(SessionCommand::Send { content, images });
    true
}

fn is_already_processing_error(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("already processing a message")
}

fn recover_async_busy(
    event: &ApiEvent,
    unaccepted: &mut VecDeque<SessionCommand>,
    pending: &mut VecDeque<SessionCommand>,
) -> bool {
    if !matches!(event, ApiEvent::Error { message, .. } if is_already_processing_error(message)) {
        return false;
    }
    let Some(submission) = unaccepted.pop_front() else {
        return false;
    };
    pending.push_front(submission);
    true
}

fn update_turn_activity(event: &ApiEvent, turn_active: &mut bool) {
    match event {
        ApiEvent::MessageAccepted { .. }
        | ApiEvent::TextDelta { .. }
        | ApiEvent::ReasoningDelta { .. }
        | ApiEvent::ToolStart { .. } => *turn_active = true,
        ApiEvent::TurnDone { .. } | ApiEvent::TurnStopped { .. } | ApiEvent::Error { .. } => {
            *turn_active = false
        }
        // `attached` describes the transport, not a model turn. Treating every
        // non-idle status as active routed the first prompt in a fresh desktop
        // panel through `soft_interrupt`; with no turn to interrupt, the prompt
        // stayed queued forever and the panel showed only its local echo.
        ApiEvent::SessionStatus { status, .. }
            if matches!(
                status.as_str(),
                "idle" | "cancelled" | "canceled" | "interrupted" | "crashed" | "error" | "failed"
            ) =>
        {
            *turn_active = false;
        }
        ApiEvent::SessionStatus { status, .. }
            if matches!(
                status.as_str(),
                "generating"
                    | "running"
                    | "processing"
                    | "busy"
                    | "thinking"
                    | "streaming"
                    | "running_tools"
            ) =>
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

/// Outcome of waiting between reconnect attempts.
#[derive(Debug, PartialEq, Eq)]
enum ReconnectWait {
    /// The panel closed; end the worker.
    Stop,
    /// The user acted; retry now with the base delay.
    Woken,
    /// The delay elapsed with nothing to do.
    Elapsed,
}

/// Wait out a reconnect delay, but wake at once on any command.
///
/// Backoff can reach 30 s. A Stop must still end the worker promptly, and a
/// user action (sending a message) is a reason to retry now rather than later.
fn wait_for_reconnect(
    commands: &Receiver<SessionCommand>,
    pending: &mut VecDeque<SessionCommand>,
    delay: Duration,
) -> ReconnectWait {
    match commands.recv_timeout(delay) {
        Ok(SessionCommand::Stop) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            ReconnectWait::Stop
        }
        Ok(command) => {
            pending.push_back(command);
            if collect_disconnected_commands(commands, pending) {
                ReconnectWait::Stop
            } else {
                ReconnectWait::Woken
            }
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => ReconnectWait::Elapsed,
    }
}

#[cfg(test)]
#[path = "harness_submission_tests.rs"]
mod submission_tests;

#[cfg(all(test, unix))]
#[path = "harness_remote_tests.rs"]
mod remote_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn unknown_session_reconnects_back_off_to_the_cap() {
        let unknown = jcode_sdk::Error::new(
            jcode_sdk::ErrorKind::Harness(jcode_sdk::api::ErrorCode::UnknownSession),
            "gone",
        );
        let transport = jcode_sdk::Error::new(jcode_sdk::ErrorKind::ConnectFailed, "down");
        let base = Duration::from_millis(300);
        assert_eq!(
            next_reconnect_delay(base, &unknown),
            Duration::from_millis(1200)
        );
        assert_eq!(
            next_reconnect_delay(base, &transport),
            Duration::from_millis(600)
        );
        let mut delay = base;
        for _ in 0..20 {
            delay = next_reconnect_delay(delay, &unknown);
        }
        assert_eq!(delay, MAX_RECONNECT_DELAY);
    }

    #[test]
    fn runtime_revive_is_throttled() {
        let now = Instant::now();
        assert!(revive_due(None, now));
        assert!(!revive_due(Some(now), now + Duration::from_millis(500)));
        assert!(revive_due(Some(now), now + RUNTIME_REVIVE_INTERVAL));
    }

    #[test]
    fn reconnect_logging_thins_out() {
        let logged = (0..1000)
            .filter(|&attempt| should_log_reconnect(attempt))
            .count();
        assert!(logged <= 13, "{logged}");
        assert!(should_log_reconnect(0));
    }

    #[test]
    fn reconnect_wait_wakes_on_commands_and_stops() {
        let (tx, rx) = channel();
        let mut pending = VecDeque::new();
        let started = Instant::now();
        assert_eq!(
            wait_for_reconnect(&rx, &mut pending, Duration::from_millis(20)),
            ReconnectWait::Elapsed
        );
        tx.send(SessionCommand::Cancel).unwrap();
        assert_eq!(
            wait_for_reconnect(&rx, &mut pending, Duration::from_secs(30)),
            ReconnectWait::Woken
        );
        assert_eq!(pending.len(), 1);
        tx.send(SessionCommand::Stop).unwrap();
        assert_eq!(
            wait_for_reconnect(&rx, &mut pending, Duration::from_secs(30)),
            ReconnectWait::Stop
        );
        drop(tx);
        assert_eq!(
            wait_for_reconnect(&rx, &mut pending, Duration::from_secs(30)),
            ReconnectWait::Stop
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn bounded_bridge_batches_preserve_order_and_leave_backlog_queued() {
        let (updates, receiver) = async_channel::unbounded();
        let (commands, _command_rx) = channel();
        let bridge = Bridge {
            _lifetime: std::sync::Arc::new(BridgeLifetime(commands.clone())),
            commands,
            updates: receiver,
        };
        for index in 0..1000 {
            updates.try_send(Update::Status(index.to_string())).unwrap();
        }
        assert!(bridge.drain_up_to(0).is_empty());
        assert_eq!(bridge.updates.len(), 1000);
        let mut received = Vec::new();
        while !bridge.updates.is_empty() {
            let batch = bridge.drain_up_to(128);
            assert!(!batch.is_empty() && batch.len() <= 128);
            received.extend(batch.into_iter().map(|update| match update {
                Update::Status(value) => value,
                _ => panic!("unexpected event"),
            }));
        }
        assert_eq!(
            received,
            (0..1000).map(|i| i.to_string()).collect::<Vec<_>>()
        );
        assert!(bridge.drain_up_to(128).is_empty());
        drop(updates);
        assert!(bridge.drain_up_to(128).is_empty());
    }

    fn session_info(id: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            working_dir: None,
            title: None,
            status: "idle".into(),
            transcript_bytes: None,
            saved: false,
            updated_at_ms: None,
            last_active_at_ms: None,
            archived: false,
            archived_at_ms: None,
            save_label: None,
            parent_session_id: None,
            agent_label: None,
            swarm_status: None,
            edit_stats: None,
        }
    }

    #[test]
    fn persisted_status_handles_unit_and_detail_variants() {
        assert_eq!(
            persisted_session_status(&serde_json::json!("Closed")),
            "closed"
        );
        assert_eq!(
            persisted_session_status(&serde_json::json!({
                "Crashed": { "message": "process exited" }
            })),
            "crashed"
        );
        assert_eq!(persisted_session_status(&serde_json::json!(null)), "idle");
    }

    #[test]
    fn session_recency_uses_timestamp_from_modern_and_legacy_ids() {
        assert_eq!(
            session_recency_ms("session_wolf_1787082160300_cab86a9cb334fa3f"),
            Some(1_787_082_160_300)
        );
        assert_eq!(
            session_recency_ms("session_1768160401354_2233921634250370970"),
            Some(1_768_160_401_354)
        );
        assert_eq!(session_recency_ms("legacy-name"), None);
    }

    #[test]
    fn persisted_sessions_fill_sidebar_in_last_interaction_order_without_duplicates_or_archives() {
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

        // Interacting with an older-created session rewrites its transcript and
        // should move it ahead of sessions created later.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(
            sessions_dir.join("older.json"),
            r#"{"working_dir":"/old","title":"Old title","messages":[]}"#,
        )
        .unwrap();
        let merged = merge_persisted_sessions(Vec::new(), Some(&home));
        let ids = merged
            .iter()
            .map(|session| session.session_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["newer", "older"]);

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn persisted_sidebar_bounds_transcript_parsing_to_recent_sessions() {
        let home = std::env::temp_dir().join(format!(
            "jcode-desktop-bounded-sessions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sessions_dir = home.join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        for index in 0..MAX_PERSISTED_SIDEBAR_SESSIONS + 20 {
            std::fs::write(
                sessions_dir.join(format!("session-{index:03}.json")),
                format!(r#"{{"title":"Session {index}"}}"#),
            )
            .unwrap();
        }

        let merged = merge_persisted_sessions(Vec::new(), Some(&home));
        assert_eq!(merged.len(), MAX_PERSISTED_SIDEBAR_SESSIONS);

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn persisted_sidebar_reads_large_transcript_metadata_from_file_edges() {
        let path = std::env::temp_dir().join(format!(
            "jcode-desktop-large-session-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let padding = "x".repeat(SIDEBAR_METADATA_WINDOW * 3);
        let document = format!(
            r#"{{"title":"Quick title","messages":[{{"text":"{padding}"}}],"working_dir":"/large/project"}}"#
        );
        std::fs::write(&path, &document).unwrap();

        let record = read_persisted_session(&path, document.len() as u64).unwrap();
        assert_eq!(record.title.as_deref(), Some("Quick title"));
        assert_eq!(record.working_dir.as_deref(), Some("/large/project"));

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn persisted_sidebar_uses_tui_todo_title_for_live_sessions() {
        let home = std::env::temp_dir().join(format!(
            "jcode-desktop-todo-title-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(home.join("todos")).unwrap();
        std::fs::write(
            home.join("todos/live.json"),
            r#"[{"content":"Fallback item","status":"in_progress","group":"Sidebar performance"}]"#,
        )
        .unwrap();
        let mut live = session_info("live");
        live.title = None;

        let merged = merge_persisted_sessions(vec![live], Some(&home));
        assert_eq!(merged[0].title.as_deref(), Some("Sidebar performance"));

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn full_api_page_is_ready_without_scanning_persisted_transcripts() {
        let home = std::env::temp_dir().join(format!(
            "jcode-desktop-api-page-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sessions_dir = home.join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::write(
            sessions_dir.join("disk-only.json"),
            r#"{"title":"Must not delay the API page"}"#,
        )
        .unwrap();

        let api_sessions = (0..MAX_PERSISTED_SIDEBAR_SESSIONS)
            .map(|index| session_info(&format!("api-{index:03}")))
            .collect::<Vec<_>>();
        let merged = merge_persisted_sessions(api_sessions, Some(&home));

        assert_eq!(merged.len(), MAX_PERSISTED_SIDEBAR_SESSIONS);
        assert_eq!(merged[0].session_id, "api-099");
        assert_eq!(merged.last().unwrap().session_id, "api-000");
        assert!(
            merged
                .iter()
                .all(|session| session.session_id != "disk-only")
        );

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
    fn wrong_session_attachment_is_retried_as_a_transport_failure() {
        let mismatch = "this connection is attached to `session_old`, not `session_new`; attach to it first or use another connection";
        let images = vec![("image/png".into(), "payload".into())];
        let mut pending = VecDeque::new();
        assert!(queue_wrong_session_retry(
            mismatch,
            &mut pending,
            "original prompt".into(),
            images.clone(),
        ));
        let Some(SessionCommand::Send {
            content,
            images: queued_images,
        }) = pending.pop_front()
        else {
            panic!("original send was not queued for retry");
        };
        assert_eq!(content, "original prompt");
        assert_eq!(queued_images, images);

        assert!(!queue_wrong_session_retry(
            "ordinary provider error",
            &mut pending,
            "must not retry".into(),
            Vec::new(),
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn asynchronous_attachment_error_replays_the_unaccepted_prompt() {
        let event = ApiEvent::Error {
            code: jcode_sdk::api::ErrorCode::UnknownSession,
            message: "this connection is attached to `old`, not `wanted`; attach to it first or use another connection".into(),
        };
        let images = vec![("image/png".into(), "payload".into())];
        let mut unaccepted = VecDeque::from([SessionCommand::Send {
            content: "hi".into(),
            images: images.clone(),
        }]);
        let mut pending = VecDeque::new();

        assert!(recover_async_attachment_mismatch(
            &event,
            &mut unaccepted,
            &mut pending
        ));
        assert!(unaccepted.is_empty());
        assert!(matches!(
            pending.pop_front(),
            Some(SessionCommand::Send { content, images: queued })
                if content == "hi" && queued == images
        ));
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
        bridge.send(Command::CreateSession {
            working_dir: None,
            request_id: None,
        });

        let deadline = Instant::now() + Duration::from_secs(120);
        let (session_id, mut attached) = loop {
            assert!(
                Instant::now() < deadline,
                "runtime did not create a session"
            );
            let updates = bridge.drain();
            if let Some(session_id) = updates.iter().find_map(|update| match update {
                Update::SessionCreated { session, .. } => Some(session.session_id.clone()),
                _ => None,
            }) {
                let attached = updates.iter().any(|update| {
                    matches!(update, Update::SessionConnected { session_id: connected } if connected == &session_id)
                });
                break (session_id, attached);
            }
            std::thread::sleep(Duration::from_millis(50));
        };

        bridge.send(Command::Watch {
            session_id: session_id.clone(),
        });

        // Match the real UI: the panel is ready when its dedicated worker has
        // attached. SessionStatus wording belongs to the daemon and has changed
        // over time, while SessionConnected is the bridge's public readiness
        // signal.
        while !attached {
            assert!(
                Instant::now() < deadline,
                "panel never reached attached status"
            );
            attached = bridge.drain().into_iter().any(
                |update| matches!(update, Update::SessionConnected { session_id: ref connected } if connected == &session_id),
            );
            if !attached {
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        bridge.send(Command::Send {
            session_id: session_id.clone(),
            content: "Reply with exactly JCODE_DESKTOP_OK and nothing else.".into(),
            images: Vec::new(),
        });

        let mut accepted = false;
        loop {
            assert!(
                Instant::now() < deadline,
                "runtime never accepted the submitted prompt"
            );
            for update in bridge.drain() {
                match update {
                    Update::MessageSubmitted {
                        session_id: event_session,
                    } if event_session == session_id => accepted = true,
                    Update::SendFailed {
                        session_id: event_session,
                        reason,
                    } if event_session == session_id => panic!("send failed: {reason}"),
                    _ => {}
                }
            }
            if accepted {
                bridge.send(Command::Cancel {
                    session_id: session_id.clone(),
                });
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
    fn active_worker_observes_command_channel_disconnect_after_pending_work() {
        let (tx, rx) = channel();
        let mut pending = VecDeque::from([SessionCommand::Cancel]);
        drop(tx);

        assert!(matches!(
            next_worker_command(&rx, &mut pending),
            WorkerCommand::Command(SessionCommand::Cancel)
        ));
        assert!(matches!(
            next_worker_command(&rx, &mut pending),
            WorkerCommand::Disconnected
        ));
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

    #[test]
    fn completed_worker_can_detach_without_waiting_for_idle_status() {
        let mut active = true;
        update_turn_activity(
            &ApiEvent::TurnDone {
                session_id: "s1".into(),
            },
            &mut active,
        );
        assert!(!active);
        assert!(should_detach_on_drop(active));

        // A later turn must restore interruption detection.
        update_turn_activity(
            &ApiEvent::MessageAccepted {
                session_id: "s1".into(),
            },
            &mut active,
        );
        assert!(!should_detach_on_drop(active));
    }

    #[test]
    fn streaming_worker_disconnect_keeps_crash_detection_armed() {
        assert!(!should_detach_on_drop(true));
        assert!(should_detach_on_drop(false));
    }

    #[test]
    fn active_turn_race_error_is_recognized_for_interrupt_retry() {
        assert!(is_already_processing_error("Already processing a message"));
        assert!(is_already_processing_error(
            "request failed: ALREADY PROCESSING A MESSAGE"
        ));
        assert!(!is_already_processing_error("daemon connection closed"));
    }

    #[test]
    fn observer_activity_and_reconnect_status_control_worker_steering() {
        let mut active = false;
        update_turn_activity(
            &ApiEvent::TextDelta {
                message_id: None,
                session_id: "s1".into(),
                text: "observed".into(),
            },
            &mut active,
        );
        assert!(active);
        for status in ["idle", "cancelled", "canceled"] {
            update_turn_activity(
                &ApiEvent::SessionStatus {
                    session_id: "s1".into(),
                    status: "processing".into(),
                },
                &mut active,
            );
            assert!(active);
            update_turn_activity(
                &ApiEvent::SessionStatus {
                    session_id: "s1".into(),
                    status: "attached".into(),
                },
                &mut active,
            );
            assert!(active, "transport notification must not change steering");
            update_turn_activity(
                &ApiEvent::SessionStatus {
                    session_id: "s1".into(),
                    status: status.into(),
                },
                &mut active,
            );
            assert!(!active);
        }
    }
}

#[cfg(test)]
mod side_panel_routing_tests {
    use super::*;

    #[test]
    fn side_panel_events_keep_remote_session_namespace_and_content() {
        let address = remote::SessionAddress::parse("ssh://example/session_one").unwrap();
        let snapshot = jcode_sdk::SidePanelSnapshot {
            focus_revision: 7,
            focused_page_id: Some("notes".into()),
            pages: vec![jcode_sdk::SidePanelPage {
                id: "notes".into(),
                content: "# Remote PDF".into(),
                format: jcode_sdk::SidePanelPageFormat::Pdf,
                pdf_data: Some("JVBERi0xLjcK".into()),
                file_path: "/remote-only/report.pdf".into(),
                ..Default::default()
            }],
        };
        let event = namespace_event(
            ApiEvent::SidePanelState {
                session_id: "session_one".into(),
                snapshot: snapshot.clone(),
            },
            &address,
        );
        assert_eq!(event_session_id(&event), Some("ssh://example/session_one"));
        match event {
            ApiEvent::SidePanelState {
                snapshot: actual, ..
            } => assert_eq!(actual, snapshot),
            _ => panic!("wrong event"),
        }
    }
}

#[cfg(test)]
mod stop_reason_routing_tests {
    use super::*;

    #[test]
    fn legacy_abnormal_statuses_settle_worker_activity() {
        for status in [
            "cancelled",
            "canceled",
            "interrupted",
            "crashed",
            "error",
            "failed",
        ] {
            let mut active = true;
            update_turn_activity(
                &ApiEvent::SessionStatus {
                    session_id: "legacy".into(),
                    status: status.into(),
                },
                &mut active,
            );
            assert!(!active, "{status}");
        }
    }

    fn stop(reason: jcode_sdk::TurnStopReason) -> ApiEvent {
        ApiEvent::TurnStopped {
            session_id: "session_one".into(),
            reason,
            message: "Provider stopped the response".into(),
            provider_stop_reason: Some("content_filter".into()),
        }
    }

    #[test]
    fn stopped_events_preserve_details_and_route_to_local_or_remote_session() {
        for (address, expected_session) in [
            ("session_one", "session_one"),
            ("ssh://example/session_one", "ssh://example/session_one"),
        ] {
            let address = remote::SessionAddress::parse(address).unwrap();
            for reason in [
                jcode_sdk::TurnStopReason::Interrupted,
                jcode_sdk::TurnStopReason::Failure,
                jcode_sdk::TurnStopReason::Crash,
                jcode_sdk::TurnStopReason::ProviderGuardrail,
                jcode_sdk::TurnStopReason::LimitReached,
                jcode_sdk::TurnStopReason::Unknown,
            ] {
                let original = stop(reason);
                assert_eq!(event_session_id(&original), Some("session_one"));
                let routed = namespace_event(original, &address);
                assert_eq!(event_session_id(&routed), Some(expected_session));
                match routed {
                    ApiEvent::TurnStopped {
                        reason: actual_reason,
                        message,
                        provider_stop_reason,
                        ..
                    } => {
                        assert_eq!(actual_reason, reason);
                        assert_eq!(message, "Provider stopped the response");
                        assert_eq!(provider_stop_reason.as_deref(), Some("content_filter"));
                    }
                    _ => panic!("routing changed the stop event type"),
                }
            }
        }
    }

    #[test]
    fn every_stop_reason_settles_worker_activity_without_waiting_for_done_or_idle() {
        for reason in [
            jcode_sdk::TurnStopReason::Interrupted,
            jcode_sdk::TurnStopReason::Failure,
            jcode_sdk::TurnStopReason::Crash,
            jcode_sdk::TurnStopReason::ProviderGuardrail,
            jcode_sdk::TurnStopReason::LimitReached,
            jcode_sdk::TurnStopReason::Unknown,
        ] {
            for initially_active in [false, true] {
                let mut active = initially_active;
                update_turn_activity(&stop(reason), &mut active);
                assert!(!active, "{reason:?}");
                assert!(should_detach_on_drop(active));
                update_turn_activity(&stop(reason), &mut active);
                update_turn_activity(
                    &ApiEvent::SessionStatus {
                        session_id: "session_one".into(),
                        status: "attached".into(),
                    },
                    &mut active,
                );
                assert!(
                    !active,
                    "a replay or transport attach cannot revive the turn"
                );
                update_turn_activity(
                    &ApiEvent::MessageAccepted {
                        session_id: "session_one".into(),
                    },
                    &mut active,
                );
                assert!(active, "a new accepted turn must restore steering");
                assert!(!should_detach_on_drop(active));
            }
        }
    }
}

#[cfg(all(test, unix))]
#[path = "harness_transport_integration_tests.rs"]
mod transport_integration_tests;
