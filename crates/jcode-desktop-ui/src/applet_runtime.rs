//! Applet runtime: one app-wide [`AppletHost`] plus provider transports.
//!
//! The runtime is a GPUI global, so it survives UI hot reload (App globals
//! persist across generations) and every window sees the same instances.
//! Providers are:
//!
//! - Local processes declared in `~/.jcode/applets/<id>/applet.json`, spoken to
//!   over newline-delimited JSON on stdio. Any language works, like MCP.
//! - In-process providers (built-in demos, tests) that push messages directly.
//!
//! A misbehaving provider can only affect its own instances: every message is
//! validated by the host, output lines are size-capped, and a crashed process
//! is reported on its instances rather than taking the UI down.
use crate::applet_host::{AppletHost, AssetCache, Effect};
use gpui::{App, Global};
use jcode_applet_types::{HostMessage, ProviderMessage};
use serde::Deserialize;
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
};

/// A single provider line may carry assets. Beyond this, the line is dropped.
const MAX_LINE_BYTES: usize = 48 * 1024 * 1024;

/// `~/.jcode/applets/<id>/applet.json`: how to launch a local provider.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LocalApplet {
    pub id: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Start with Desktop rather than on first launch.
    #[serde(default)]
    pub autostart: bool,
    #[serde(skip)]
    pub dir: PathBuf,
}

enum Inbound {
    Message(String, ProviderMessage),
    Invalid(String, String),
    Exited(String, String),
}

struct Process {
    child: Child,
    stdin: ChildStdin,
}

pub(crate) struct Runtime {
    pub host: RefCell<AppletHost>,
    pub assets: RefCell<AssetCache>,
    processes: RefCell<HashMap<String, Process>>,
    inbound_tx: mpsc::Sender<Inbound>,
    inbound_rx: RefCell<mpsc::Receiver<Inbound>>,
    /// Toasts waiting to be shown by the workspace.
    pub toasts: RefCell<Vec<(String, jcode_applet_types::view::Tone)>>,
    /// Bumped on every visible change, so views know to re-render.
    pub generation: std::cell::Cell<u64>,
}

impl Global for Runtime {}

impl Default for Runtime {
    fn default() -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        Self {
            host: RefCell::new(AppletHost::new()),
            assets: RefCell::new(AssetCache::default()),
            processes: RefCell::new(HashMap::new()),
            inbound_tx,
            inbound_rx: RefCell::new(inbound_rx),
            toasts: RefCell::new(Vec::new()),
            generation: std::cell::Cell::new(0),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        for (_, mut process) in self.processes.get_mut().drain() {
            let _ = process.child.kill();
        }
    }
}

pub(crate) fn get(cx: &mut App) -> &Runtime {
    if !cx.has_global::<Runtime>() {
        cx.set_global(Runtime::default());
    }
    cx.global::<Runtime>()
}

impl Runtime {
    fn bump(&self) {
        self.generation.set(self.generation.get() + 1);
    }

    /// Apply a message from an in-process provider.
    pub fn apply(&self, applet: &str, message: ProviderMessage) -> Result<(), String> {
        if let ProviderMessage::Toast { text, tone, .. } = &message {
            self.toasts.borrow_mut().push((text.clone(), *tone));
        }
        let result = self.host.borrow_mut().apply(applet, message);
        self.bump();
        self.flush();
        result
    }

    /// Drain provider output and deliver host messages. Returns whether
    /// anything changed, so callers can notify only when needed.
    pub fn pump(&self) -> bool {
        let mut changed = false;
        loop {
            let next = self.inbound_rx.borrow().try_recv();
            let Ok(inbound) = next else { break };
            changed = true;
            match inbound {
                Inbound::Message(applet, message) => {
                    let _ = self.apply(&applet, message);
                }
                Inbound::Invalid(applet, error) => {
                    self.send(
                        &applet,
                        &HostMessage::Rejected {
                            instance: None,
                            reason: error,
                        },
                    );
                }
                Inbound::Exited(applet, reason) => {
                    self.processes.borrow_mut().remove(&applet);
                    let mut host = self.host.borrow_mut();
                    host.provider_disconnected(&applet);
                    let ids: Vec<String> = host
                        .instances()
                        .filter(|mounted| mounted.instance.applet == applet)
                        .map(|mounted| mounted.instance.id.clone())
                        .collect();
                    drop(host);
                    for id in ids {
                        self.host
                            .borrow_mut()
                            .mark_error(&id, format!("Provider stopped: {reason}"));
                    }
                    self.bump();
                }
            }
        }
        changed
    }

    fn flush(&self) {
        for (applet, message) in self.host.borrow_mut().drain_outbox() {
            self.send(&applet, &message);
        }
    }

    fn send(&self, applet: &str, message: &HostMessage) {
        let mut processes = self.processes.borrow_mut();
        let Some(process) = processes.get_mut(applet) else {
            return;
        };
        let Ok(mut line) = serde_json::to_string(message) else {
            return;
        };
        line.push('\n');
        if process
            .stdin
            .write_all(line.as_bytes())
            .and_then(|_| process.stdin.flush())
            .is_err()
        {
            // The reader thread reports the exit.
        }
    }

    /// Route a rendered node's intent. Returns the effect for the caller to
    /// perform with window access (URLs, clipboard, chats).
    pub fn dispatch(
        &self,
        instance: &str,
        action: &jcode_applet_types::view::Action,
        source_key: Option<String>,
    ) -> Option<Effect> {
        let effect = self
            .host
            .borrow_mut()
            .dispatch(instance, action, source_key);
        self.bump();
        self.flush();
        effect
    }

    pub fn set_state(&self, instance: &str, key: &str, value: serde_json::Value) {
        if self.host.borrow_mut().set_state(instance, key, value) {
            self.bump();
        }
    }

    pub fn close(&self, instance: &str) {
        self.host.borrow_mut().close(instance);
        self.bump();
        self.flush();
    }

    /// Start a local provider. Idempotent while it is running.
    pub fn start(&self, applet: &LocalApplet) -> Result<(), String> {
        if self.processes.borrow().contains_key(&applet.id) {
            return Ok(());
        }
        let (program, args) = applet
            .command
            .split_first()
            .ok_or_else(|| format!("applet {} has an empty command", applet.id))?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&applet.dir)
            .envs(&applet.env)
            .env("JCODE_APPLET_ID", &applet.id)
            .env("JCODE_APPLET_SCHEMA", jcode_applet_types::SCHEMA)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not start applet {}: {error}", applet.id))?;
        let stdin = child.stdin.take().ok_or("applet stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("applet stdout unavailable")?;
        let tx = self.inbound_tx.clone();
        let id = applet.id.clone();
        std::thread::Builder::new()
            .name(format!("applet-{id}"))
            .spawn(move || read_provider(id, stdout, tx))
            .map_err(|error| error.to_string())?;
        self.processes
            .borrow_mut()
            .insert(applet.id.clone(), Process { child, stdin });
        Ok(())
    }
}

fn read_provider(applet: String, stdout: std::process::ChildStdout, tx: mpsc::Sender<Inbound>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_line(&mut line)
        {
            Ok(0) => {
                let _ = tx.send(Inbound::Exited(applet, "exited".into()));
                return;
            }
            Ok(_) if line.len() > MAX_LINE_BYTES => {
                let _ = tx.send(Inbound::Invalid(applet.clone(), "message too large".into()));
                // Discard the rest of the oversized line without buffering it.
                loop {
                    let (done, used) = match reader.fill_buf() {
                        Ok([]) => (true, 0),
                        Ok(buf) => match buf.iter().position(|&b| b == b'\n') {
                            Some(i) => (true, i + 1),
                            None => (false, buf.len()),
                        },
                        Err(_) => (true, 0),
                    };
                    reader.consume(used);
                    if done {
                        break;
                    }
                }
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let inbound = match serde_json::from_str::<ProviderMessage>(trimmed) {
                    Ok(message) => Inbound::Message(applet.clone(), message),
                    Err(error) => Inbound::Invalid(applet.clone(), error.to_string()),
                };
                if tx.send(inbound).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = tx.send(Inbound::Exited(applet, error.to_string()));
                return;
            }
        }
    }
}

use std::io::Read as _;

/// Discover local applets. Invalid entries are skipped with a log line.
pub(crate) fn discover(root: &Path) -> Vec<LocalApplet> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut applets = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let manifest = dir.join("applet.json");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        match serde_json::from_str::<LocalApplet>(&text) {
            Ok(mut applet) if !applet.command.is_empty() => {
                applet.dir = dir;
                applets.push(applet);
            }
            Ok(_) => eprintln!("jcode desktop: {} has an empty command", manifest.display()),
            Err(error) => eprintln!("jcode desktop: invalid {}: {error}", manifest.display()),
        }
    }
    applets.sort_by(|a, b| a.id.cmp(&b.id));
    applets
}

pub(crate) fn applets_dir() -> Option<PathBuf> {
    std::env::var_os("JCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".jcode")))
        .map(|home| home.join("applets"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn local_process_provider_round_trips_over_stdio() {
        let dir = std::env::temp_dir().join(format!("jcode-applet-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("echo")).unwrap();
        // A tiny provider: registers, mounts, then patches the title for every action.
        let script = r#"
import json, sys
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
send({"type": "register", "manifest": {"schema": "jcode.applet/1", "id": "echo", "title": "Echo"}})
send({"type": "mount", "instance": "e1", "placement": {"kind": "panel"},
      "document": {"revision": 1, "title": "Echo", "view": {"type": "button", "label": "Hi", "on_press": {"action": "hi"}}}})
rev = 1
for line in sys.stdin:
    m = json.loads(line)
    if m["type"] == "action":
        send({"type": "patch", "instance": "e1", "base_revision": rev,
              "ops": [{"op": "replace", "path": "/title", "value": "Got " + m["action"]["action"]}]})
        rev += 1
"#;
        std::fs::write(dir.join("echo/provider.py"), script).unwrap();
        std::fs::write(
            dir.join("echo/applet.json"),
            json!({"id": "echo", "command": ["python3", "provider.py"]}).to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("broken")).unwrap();
        std::fs::write(dir.join("broken/applet.json"), "{not json").unwrap();

        let applets = discover(&dir);
        assert_eq!(applets.len(), 1);
        let runtime = Runtime::default();
        runtime.start(&applets[0]).unwrap();

        let wait = |check: &dyn Fn(&Runtime) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                runtime.pump();
                if check(&runtime) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        };
        assert!(
            wait(&|r| r.host.borrow().instance("e1").is_some()),
            "mounted"
        );
        runtime.dispatch("e1", &jcode_applet_types::view::Action::new("hi"), None);
        assert!(wait(&|r| r.host.borrow().instance("e1").is_some_and(|m| m
            .instance
            .document
            .title
            == "Got hi")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
