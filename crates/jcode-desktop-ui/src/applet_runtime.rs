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
    /// Host effects that need workspace access (new chats, prompts).
    pub effects: RefCell<Vec<Effect>>,
    /// Toasts waiting to be shown by the workspace.
    pub toasts: RefCell<Vec<(String, jcode_applet_types::view::Tone)>>,
    /// Bumped on every visible change, so views know to re-render.
    pub generation: std::cell::Cell<u64>,
    /// Actions on agent instances, for the workspace to forward to the SDK:
    /// `(session_id, local instance id, message)`.
    pub agent_outbox: RefCell<Vec<AgentOutbound>>,
    /// Persistent instances were restored from disk once per process.
    restored: std::cell::Cell<bool>,
    /// Last persisted bytes, so unchanged state is not rewritten.
    persisted: RefCell<Vec<u8>>,
}

/// A user intent on an agent-mounted instance, bound for the SDK.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AgentOutbound {
    Action {
        session_id: String,
        instance: String,
        action: jcode_applet_types::Action,
        state: serde_json::Value,
        source_key: Option<String>,
    },
    Close {
        session_id: String,
        instance: String,
    },
}

/// Host-wide id of an agent instance: session ids are globally unique and
/// agent instance ids are only unique within their session.
pub(crate) fn agent_instance_id(session_id: &str, local: &str) -> String {
    format!(
        "{}@{session_id}/{local}",
        jcode_applet_types::agent::APPLET_ID
    )
}

/// Split a host-wide agent instance id into `(session_id, local id)`.
pub(crate) fn split_agent_instance(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(jcode_applet_types::agent::APPLET_ID)?
        .strip_prefix('@')?
        .rsplit_once('/')
}

impl Global for Runtime {}

impl Default for Runtime {
    fn default() -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let mut host = AppletHost::new();
        host.trust(crate::workspace::applets::SHOWCASE_ID);
        host.trust(jcode_applet_types::agent::APPLET_ID);
        let _ = host.apply(
            jcode_applet_types::agent::APPLET_ID,
            ProviderMessage::Register {
                manifest: jcode_applet_types::agent::manifest(),
            },
        );
        if !cfg!(test) {
            host.set_grants(load_grants());
        }
        Self {
            host: RefCell::new(host),
            assets: RefCell::new(AssetCache::default()),
            processes: RefCell::new(HashMap::new()),
            inbound_tx,
            inbound_rx: RefCell::new(inbound_rx),
            toasts: RefCell::new(Vec::new()),
            generation: std::cell::Cell::new(0),
            agent_outbox: RefCell::new(Vec::new()),
            effects: RefCell::new(Vec::new()),
            restored: std::cell::Cell::new(false),
            persisted: RefCell::new(Vec::new()),
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
        let registering = matches!(message, ProviderMessage::Register { .. });
        let result = self.host.borrow_mut().apply(applet, message);
        if registering && result.is_ok() {
            let startup = self.host.borrow().startup_launchers(applet);
            for index in startup {
                let _ = self.host.borrow_mut().launch(applet, index);
            }
        }
        self.bump();
        self.flush();
        result
    }

    /// Replace one session's agent instances with the server's snapshot.
    /// Unchanged revisions are left alone, so typed-but-unsent input and
    /// scroll positions survive. Instances the snapshot omits are removed.
    pub fn sync_agent(
        &self,
        session_id: &str,
        snapshot: &jcode_applet_types::AgentApplets,
    ) -> bool {
        use jcode_applet_types::Placement;
        let mut host = self.host.borrow_mut();
        let prefix = agent_instance_id(session_id, "");
        let mut keep = std::collections::HashSet::new();
        let mut changed = false;
        for instance in &snapshot.instances {
            let id = agent_instance_id(session_id, &instance.id);
            keep.insert(id.clone());
            let mut next = instance.clone();
            next.id = id.clone();
            next.applet = jcode_applet_types::agent::APPLET_ID.to_owned();
            // Placement session ids come from the server's view of the
            // session. The Desktop panel may know it under a namespaced id
            // (remote hosts), so the snapshot's session is authoritative.
            next.placement = match next.placement {
                Placement::Inline { anchor, .. } => Placement::Inline {
                    session_id: session_id.to_owned(),
                    anchor,
                },
                Placement::Composer { .. } => Placement::Composer {
                    session_id: session_id.to_owned(),
                },
                other => other,
            };
            next.scope = jcode_applet_types::Scope::Session {
                session_id: session_id.to_owned(),
            };
            let same = host.instance(&id).is_some_and(|mounted| {
                mounted.instance.document.revision == next.document.revision
                    && mounted.instance.placement == next.placement
            });
            if same {
                continue;
            }
            match host.upsert(next) {
                Ok(()) => changed = true,
                Err(error) => host.mark_error(&id, error),
            }
        }
        let stale: Vec<String> = host
            .instances()
            .filter(|mounted| mounted.instance.id.starts_with(&prefix))
            .map(|mounted| mounted.instance.id.clone())
            .filter(|id| !keep.contains(id))
            .collect();
        for id in stale {
            changed |= host.remove_silently(&id);
        }
        drop(host);
        if changed {
            self.bump();
        }
        changed
    }

    /// Restore persistent instances from the last run, once per process.
    pub fn restore_persistent(&self) {
        if self.restored.replace(true) || cfg!(test) {
            return;
        }
        let Some(path) = applets_dir().map(|dir| dir.join("instances.json")) else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(snapshot) = serde_json::from_str(&text) else {
            return;
        };
        let dropped = self.host.borrow_mut().restore(snapshot);
        if dropped > 0 {
            eprintln!("jcode desktop: dropped {dropped} invalid persisted applet instances");
        }
        self.bump();
    }

    /// Write persistent instances for the next run. Agent instances are
    /// owned by the server and restored through session attachment.
    pub fn persist(&self) {
        if cfg!(test) || crate::harness::screenshot_mode() {
            return;
        }
        let Some(dir) = applets_dir() else { return };
        let mut snapshot = self.host.borrow().snapshot(true);
        snapshot
            .instances
            .retain(|mounted| mounted.instance.applet != jcode_applet_types::agent::APPLET_ID);
        snapshot
            .manifests
            .retain(|manifest| manifest.id != jcode_applet_types::agent::APPLET_ID);
        let Ok(json) = serde_json::to_vec_pretty(&snapshot) else {
            return;
        };
        if *self.persisted.borrow() == json {
            return;
        }
        let _ = std::fs::create_dir_all(&dir);
        if std::fs::write(dir.join("instances.json"), &json).is_ok() {
            *self.persisted.borrow_mut() = json;
        }
    }

    /// Record a capability decision and persist every decision.
    pub fn decide(
        &self,
        applet: &str,
        granted: std::collections::BTreeSet<jcode_applet_types::Capability>,
    ) {
        self.host.borrow_mut().decide(applet, granted);
        self.save_grants();
        self.bump();
    }

    pub fn revoke(&self, applet: &str) {
        self.host.borrow_mut().revoke(applet);
        self.save_grants();
        self.bump();
    }

    fn save_grants(&self) {
        if cfg!(test) {
            return;
        }
        let Some(dir) = applets_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_vec_pretty(self.host.borrow().grants()) {
            let _ = std::fs::write(dir.join("grants.json"), json);
        }
    }

    /// Fire a launcher, starting its local provider first if needed.
    /// Returns an already-mounted singleton instance to focus.
    pub fn launch(&self, applet: &str, index: usize) -> Option<String> {
        if let Some(dir) = applets_dir()
            && let Some(local) = discover(&dir).into_iter().find(|a| a.id == applet)
            && let Err(error) = self.start(&local)
        {
            eprintln!("jcode desktop: {error}");
        }
        let focus = self.host.borrow_mut().launch(applet, index);
        self.flush();
        focus
    }

    /// Forward a transcript tool call to providers whose manifest claims it.
    pub fn notify_tool_call(&self, message: HostMessage) {
        if self.host.borrow_mut().notify_tool_call(message) > 0 {
            self.flush();
        }
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
            if applet == jcode_applet_types::agent::APPLET_ID {
                self.route_agent(message);
            } else {
                self.send(&applet, &message);
            }
        }
    }

    /// The agent is not a process: its intents travel to the server through
    /// the workspace's SDK bridge.
    fn route_agent(&self, message: HostMessage) {
        let outbound = match message {
            HostMessage::Action {
                instance,
                action,
                state,
                source_key,
                ..
            } => split_agent_instance(&instance).map(|(session_id, local)| AgentOutbound::Action {
                session_id: session_id.to_owned(),
                instance: local.to_owned(),
                action,
                state,
                source_key,
            }),
            HostMessage::Closed { instance } => {
                split_agent_instance(&instance).map(|(session_id, local)| AgentOutbound::Close {
                    session_id: session_id.to_owned(),
                    instance: local.to_owned(),
                })
            }
            _ => None,
        };
        if let Some(outbound) = outbound {
            self.agent_outbox.borrow_mut().push(outbound);
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

fn load_grants() -> crate::applet_host::Grants {
    applets_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("grants.json")).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
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

    /// The bundled GitHub applet: launch, list, open a PR, run a write action.
    /// A fake `gh` on PATH stands in for GitHub, so this runs offline.
    #[test]
    fn bundled_github_applet_renders_valid_documents() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("jcode-gh-applet-{}", std::process::id()));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = root.join("gh.log");
        let fake_gh = format!(
            r#"#!/usr/bin/env python3
import json, sys
open({log:?}, "a").write(" ".join(sys.argv[1:]) + "\n")
args = sys.argv[1:]
pr = "https://github.com/acme/app/pull/7"
if args[:2] == ["search", "prs"]:
    print(json.dumps([{{"number": 7, "title": "Add pills", "url": pr, "isDraft": True,
        "repository": {{"nameWithOwner": "acme/app"}}, "author": {{"login": "ana"}},
        "updatedAt": "2026-01-01T00:00:00Z", "labels": [{{"name": "ui"}}], "commentsCount": 2}}]))
elif args[:2] == ["search", "issues"]:
    print("[]")
elif args[:2] == ["pr", "view"]:
    print(json.dumps({{"number": 7, "title": "Add pills", "url": pr, "state": "OPEN",
        "isDraft": False, "author": {{"login": "ana"}}, "body": "**Hi**",
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
        "baseRefName": "main", "headRefName": "pills", "additions": 3, "deletions": 1,
        "changedFiles": 1, "reviewDecision": "REVIEW_REQUIRED", "mergeable": "MERGEABLE",
        "statusCheckRollup": [{{"conclusion": "FAILURE"}}, {{"conclusion": "SUCCESS"}}],
        "labels": [], "comments": [{{"author": {{"login": "bo"}}, "body": "lgtm",
        "createdAt": "2026-01-02T00:00:00Z"}}], "reviewRequests": [], "assignees": []}}))
elif args[:2] == ["pr", "comment"]:
    pass
elif args[:2] == ["api", "user"]:
    print("me")
else:
    sys.exit("unexpected gh call")
"#
        );
        std::fs::write(bin.join("gh"), fake_gh).unwrap();
        std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();

        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../applets/github");
        let mut applet = discover(dir.parent().unwrap())
            .into_iter()
            .find(|a| a.id == "github")
            .expect("bundled github applet");
        assert!(applet.autostart);
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        applet.env.insert("PATH".into(), path);

        let runtime = Runtime::default();
        runtime.start(&applet).unwrap();
        let wait = |what: &str, check: &dyn Fn(&Runtime) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while std::time::Instant::now() < deadline {
                runtime.pump();
                if check(&runtime) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("timed out waiting for {what}");
        };
        wait("register", &|r| {
            r.host.borrow().manifest("github").is_some()
        });
        let sidebar = runtime
            .host
            .borrow()
            .launchers()
            .into_iter()
            .position(|(applet, _, l)| {
                applet == "github"
                    && matches!(l.trigger, jcode_applet_types::manifest::Trigger::Sidebar)
            })
            .expect("sidebar launcher");
        assert_eq!(runtime.launch("github", sidebar), None);
        let instance = format!("github#launch{sidebar}");
        let doc = |r: &Runtime| {
            r.host
                .borrow()
                .instance(&instance)
                .map(|m| serde_json::to_string(&m.instance.document).unwrap())
                .unwrap_or_default()
        };
        wait("inbox rows", &|r| doc(r).contains("acme/app#7"));
        assert!(doc(&runtime).contains("Review requested 1"));

        let open = jcode_applet_types::view::Action {
            action: "open".into(),
            args: json!({"url": "https://github.com/acme/app/pull/7"}),
        };
        runtime.dispatch(&instance, &open, None);
        wait("pr detail", &|r| doc(r).contains("1 failing"));
        let detail = doc(&runtime);
        for expected in [
            "pills → main",
            "lgtm",
            "Ask Jcode",
            "host.start_chat",
            "review required",
            "\"Approve\"",
            "ask_close",
        ] {
            assert!(
                detail.contains(expected),
                "detail lacks {expected}: {detail}"
            );
        }

        runtime.set_state(&instance, "comment", json!("Looks good"));
        let comment = jcode_applet_types::view::Action {
            action: "comment".into(),
            args: json!({"url": "https://github.com/acme/app/pull/7"}),
        };
        runtime.dispatch(&instance, &comment, None);
        wait("comment posted", &|_| {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("pr comment https://github.com/acme/app/pull/7 --body Looks good")
        });
        assert!(
            runtime
                .host
                .borrow()
                .instance(&instance)
                .is_some_and(|m| m.last_error.is_none()),
            "provider documents must validate"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use serde_json::json;

    fn snapshot(revision: u64, ids: &[&str]) -> jcode_applet_types::AgentApplets {
        serde_json::from_value(json!({"instances": ids.iter().map(|id| json!({
            "id": id,
            "applet": "jcode.agent",
            "placement": {"kind": "inline", "session_id": "server-side", "anchor": {"kind": "tool_call", "call_id": "c1"}},
            "document": {"revision": revision, "title": "Pick", "state": {"q": ""},
                "view": {"type": "button", "label": "Go", "on_press": {"action": "go"}}}
        })).collect::<Vec<_>>()}))
        .unwrap()
    }

    #[test]
    fn agent_snapshots_sync_namespaced_and_actions_route_to_the_sdk() {
        let runtime = Runtime::default();
        assert!(runtime.sync_agent("sess", &snapshot(1, &["a", "b"])));
        let id = agent_instance_id("sess", "a");
        assert_eq!(split_agent_instance(&id), Some(("sess", "a")));
        {
            let host = runtime.host.borrow();
            let mounted = host.instance(&id).unwrap();
            // The panel's session id is authoritative for placement.
            assert!(host.tool_card("sess", "c1").is_some());
            assert!(matches!(&mounted.instance.scope,
                jcode_applet_types::Scope::Session { session_id } if session_id == "sess"));
        }
        // Local state survives an unchanged revision.
        runtime.set_state(&id, "q", json!("typed"));
        assert!(!runtime.sync_agent("sess", &snapshot(1, &["a", "b"])));
        assert_eq!(runtime.host.borrow().state(&id, "q"), Some(&json!("typed")));

        runtime.dispatch(&id, &jcode_applet_types::Action::new("go"), None);
        let outbound = std::mem::take(&mut *runtime.agent_outbox.borrow_mut());
        assert!(
            matches!(&outbound[..], [AgentOutbound::Action { session_id, instance, state, .. }]
            if session_id == "sess" && instance == "a" && state["q"] == "typed")
        );

        // Omitted instances disappear without echoing a close to the server.
        assert!(runtime.sync_agent("sess", &snapshot(2, &["a"])));
        assert!(
            runtime
                .host
                .borrow()
                .instance(&agent_instance_id("sess", "b"))
                .is_none()
        );
        assert!(runtime.agent_outbox.borrow().is_empty());

        // A user close is forwarded.
        runtime.close(&id);
        assert!(matches!(&runtime.agent_outbox.borrow()[..],
            [AgentOutbound::Close { session_id, instance }] if session_id == "sess" && instance == "a"));
    }
}
