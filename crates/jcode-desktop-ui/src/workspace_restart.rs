//! Restart everything, restore every window.
//!
//! Each live window keeps a small manifest (its launch plus a workspace
//! snapshot without transcripts) under the state directory. `/restart-all`
//! spawns a detached worker that stops every Desktop process, gracefully
//! reloads the Jcode server (live sessions are handed off, never killed), and
//! relaunches each window with `--restore-state=<file>`. Transcripts reload
//! from the server, so a restart reclaims memory without losing chats,
//! drafts, queued prompts, or layout.
use super::{Workspace, WorkspaceSnapshot};
use gpui::{App, Entity, Window};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// Shared with the panel's `/restart-all` command, which only spawns the worker.
pub(crate) use crate::restart_spawn::{NO_SERVER_FLAG, WORKER_FLAG, detach};
const RESTORE_FLAG: &str = "--restore-state=";
const MANIFEST_INTERVAL: Duration = Duration::from_secs(1);
/// Launch flags that described how a window was first opened, not what it
/// shows now. A restored window must not adopt a held voice key, reopen a
/// requested session over its snapshot, or insist on a private process.
const TRANSIENT_FLAGS: &[&str] = &[
    "--new-process",
    "--global-voice-hold",
    "--toggle-voice",
    "--voice-press",
    "--voice-release",
    "--reload-ui",
];

#[derive(Deserialize, Serialize)]
struct Manifest {
    pid: u32,
    window: u64,
    updated_at_ms: u64,
    args: Vec<String>,
    env: Vec<(String, String)>,
    snapshot: serde_json::Value,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("JCODE_DESKTOP_RESTART_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(crate::learning::state_path()?.with_file_name("restart"))
}

fn manifests_dir() -> Option<PathBuf> {
    Some(root()?.join("windows"))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn strip_transient(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|arg| {
            !TRANSIENT_FLAGS.contains(&arg.as_str())
                && !arg.starts_with("--session=")
                && !arg.starts_with(RESTORE_FLAG)
        })
        .cloned()
        .collect()
}

/// Keep this window's manifest current. The loop holds only weak handles, so
/// a hot reload retires it and the next generation's loop takes over the same
/// file. A closed window removes its manifest.
pub(crate) fn install(workspace: &Entity<Workspace>, window: &Window, app: &mut App) {
    if cfg!(test) || crate::harness::screenshot_mode() {
        return;
    }
    let Some(dir) = manifests_dir() else { return };
    let window_id = window.window_handle().window_id().as_u64();
    let path = dir.join(format!("{}-{window_id}.json", std::process::id()));
    let workspace = workspace.downgrade();
    let handle = window.window_handle();
    app.spawn(async move |cx| {
        let mut last: Option<Vec<u8>> = None;
        loop {
            let captured = handle.update(cx, |_, window, app| {
                let workspace = workspace.upgrade()?;
                let workspace = workspace.read(app);
                let snapshot = workspace.snapshot(window, app).ok()?;
                let launch = super::LaunchSnapshot::from(&workspace.launch);
                Some((snapshot, launch))
            });
            let (snapshot, launch) = match captured {
                Ok(Some(captured)) => captured,
                // A newer UI generation replaced this workspace.
                Ok(None) => break,
                Err(_) => {
                    let _ = fs::remove_file(&path);
                    break;
                }
            };
            let manifest = Manifest {
                pid: std::process::id(),
                window: window_id,
                updated_at_ms: now_ms(),
                args: launch.args,
                env: launch.env,
                snapshot: serde_json::to_value(&snapshot).unwrap_or_default(),
            };
            // Compare without the timestamp so idle windows never touch disk.
            let key = serde_json::to_vec(&(&manifest.args, &manifest.env, &manifest.snapshot))
                .unwrap_or_default();
            if last.as_ref() != Some(&key) {
                let path = path.clone();
                let written = cx
                    .background_executor()
                    .spawn(async move {
                        let bytes = serde_json::to_vec(&manifest)?;
                        write_atomic(&path, &bytes)?;
                        anyhow::Ok(())
                    })
                    .await;
                match written {
                    Ok(()) => last = Some(key),
                    Err(error) => eprintln!("restart manifest write failed: {error:#}"),
                }
            }
            cx.background_executor().timer(MANIFEST_INTERVAL).await;
        }
    })
    .detach();
}

/// The snapshot a `/restart-all` relaunch left for this window, consumed once.
pub(crate) fn take_restore_state(window: &Window, app: &App) -> Option<WorkspaceSnapshot> {
    let launch =
        jcode_desktop_api::WindowLaunches::get(app, window.window_handle().window_id().as_u64());
    let path = launch
        .args
        .iter()
        .find_map(|arg| arg.to_str()?.strip_prefix(RESTORE_FLAG).map(PathBuf::from))?;
    let bytes = fs::read(&path).ok();
    let _ = fs::remove_file(&path);
    let snapshot = decode_restore(&bytes?);
    if snapshot.is_none() {
        eprintln!("ignoring invalid restart state {}", path.display());
    }
    snapshot
}

fn decode_restore(bytes: &[u8]) -> Option<WorkspaceSnapshot> {
    let mut snapshot = WorkspaceSnapshot::decode(bytes).ok()?;
    for slot in &mut snapshot.slots {
        // PTYs belonged to the stopped process. Reopen in the same directory.
        if slot.panel.terminal_resource_id.take().is_some() {
            slot.panel.session_id = "terminal".into();
        }
        slot.panel.terminal_output_cursor = None;
    }
    if let Some(launch) = snapshot.launch.as_mut() {
        launch.args = strip_transient(&launch.args);
    }
    Some(snapshot)
}

// ---------------------------------------------------------------------------
// Worker process
// ---------------------------------------------------------------------------

struct Log(Option<fs::File>);

impl Log {
    fn line(&mut self, message: impl AsRef<str>) {
        let message = message.as_ref();
        eprintln!("restart-all: {message}");
        if let Some(file) = self.0.as_mut() {
            let _ = writeln!(file, "[{}] {message}", now_ms());
        }
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        // Without procfs (macOS), fall back to signal 0.
        return unsafe { libc::kill(pid as i32, 0) == 0 };
    };
    // Field 3 follows the parenthesised command name. Zombies are dead.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .is_some_and(|state| state != "Z" && state != "X")
}

#[cfg(not(unix))]
fn process_alive(_: u32) -> bool {
    false
}

fn is_desktop_process(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        fs::read_link(format!("/proc/{pid}/exe"))
            .map(|exe| exe.to_string_lossy().contains("jcode-desktop"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

/// Every Desktop UI process on this machine, for reporting stragglers.
fn desktop_pids() -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != std::process::id() && is_desktop_process(*pid) && process_alive(*pid))
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/cmdline"))
                .map(|cmdline| {
                    !cmdline
                        .split(|byte| *byte == 0)
                        .any(|arg| arg == WORKER_FLAG.as_bytes())
                })
                .unwrap_or(false)
        })
        .collect()
}

fn read_manifests(dir: &Path, log: &mut Log) -> Vec<Manifest> {
    let mut manifests = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return manifests;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let manifest = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok());
        match manifest {
            Some(manifest) if process_alive(manifest.pid) && is_desktop_process(manifest.pid) => {
                manifests.push(manifest)
            }
            _ => {
                log.line(format!("discarding stale manifest {}", path.display()));
                let _ = fs::remove_file(&path);
            }
        }
    }
    manifests
}

fn instance_socket(name: Option<&str>) -> PathBuf {
    let socket = match name {
        Some(name) => format!("jcode-desktop-{name}.sock"),
        None => "jcode-desktop.sock".to_owned(),
    };
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join(socket);
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("{user}-{socket}"))
}

#[cfg(unix)]
fn socket_accepts(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_accepts(_: &Path) -> bool {
    true
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}

#[cfg(unix)]
fn stop_processes(pids: &[u32], log: &mut Log) {
    for pid in pids {
        unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
    }
    if !wait_until(Duration::from_secs(6), || {
        pids.iter().all(|pid| !process_alive(*pid))
    }) {
        for pid in pids.iter().filter(|pid| process_alive(**pid)) {
            log.line(format!("pid {pid} ignored SIGTERM; killing"));
            unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
        }
        wait_until(Duration::from_secs(3), || {
            pids.iter().all(|pid| !process_alive(*pid))
        });
    }
}

#[cfg(not(unix))]
fn stop_processes(_: &[u32], _: &mut Log) {}

fn reload_server(log: &mut Log) {
    let jcode = crate::platform::companion_executable("jcode");
    log.line(format!("reloading server with {}", jcode.display()));
    let child = Command::new(&jcode)
        .args(["server", "reload", "--force"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            log.line(format!("server reload could not start: {error}"));
            return;
        }
    };
    let finished = wait_until(Duration::from_secs(90), || {
        matches!(child.try_wait(), Ok(Some(_)))
    });
    if !finished {
        let _ = child.kill();
        log.line("server reload timed out after 90s; continuing");
        return;
    }
    match child.wait_with_output() {
        Ok(output) => log.line(format!(
            "server reload {}: {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => log.line(format!("server reload failed: {error}")),
    }
}

/// Order restored launches: main workspace, then sidebar-free, then chats.
fn launch_rank(args: &[String]) -> u8 {
    match jcode_desktop_api::LaunchMode::from_args(args) {
        jcode_desktop_api::LaunchMode::Workspace => 0,
        jcode_desktop_api::LaunchMode::NoSidebar => 1,
        jcode_desktop_api::LaunchMode::SinglePanel => 2,
    }
}

fn relaunch(executable: &Path, restore_dir: &Path, manifests: &[Manifest], log: &mut Log) {
    let mut ordered: Vec<&Manifest> = manifests.iter().collect();
    ordered.sort_by_key(|manifest| (launch_rank(&manifest.args), manifest.updated_at_ms));
    let shared_socket = instance_socket(Some(jcode_desktop_api::SHARED_SINGLE_PANEL));
    let mut shared_host_started = false;
    for (index, manifest) in ordered.iter().enumerate() {
        let state = restore_dir.join(format!("window-{index}.json"));
        let bytes = serde_json::to_vec(&manifest.snapshot).unwrap_or_default();
        if let Err(error) = write_atomic(&state, &bytes) {
            log.line(format!("could not stage window {index}: {error}"));
            continue;
        }
        let mut args = strip_transient(&manifest.args);
        args.push(format!("{RESTORE_FLAG}{}", state.display()));
        let single_panel = jcode_desktop_api::LaunchMode::shared_single_panel(&args);
        let mut command = Command::new(executable);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in &manifest.env {
            command.env(key, value);
        }
        detach(&mut command);
        match command.spawn() {
            Ok(child) => log.line(format!(
                "relaunched window {index} as pid {}: {args:?}",
                child.id()
            )),
            Err(error) => {
                log.line(format!("could not relaunch window {index}: {error}"));
                continue;
            }
        }
        if single_panel && !shared_host_started {
            // Later chats forward to this host. Racing them would start a
            // second host that steals the socket.
            shared_host_started = true;
            if !wait_until(Duration::from_secs(20), || socket_accepts(&shared_socket)) {
                log.line("shared single-panel host did not come up within 20s");
            }
        } else if single_panel {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// Entry point for `jcode-desktop --restart-all-worker`.
pub fn run_worker(args: impl IntoIterator<Item = OsString>) -> i32 {
    let restart_server = !args.into_iter().any(|arg| arg == NO_SERVER_FLAG);
    let Some(root) = root() else {
        eprintln!("restart-all: no state directory");
        return 1;
    };
    let _ = fs::create_dir_all(&root);
    let mut log = Log(fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("restart.log"))
        .ok());
    log.line(format!("worker pid {} starting", std::process::id()));
    // Let every window flush its latest manifest (they write once a second).
    std::thread::sleep(Duration::from_millis(1600));
    let Some(dir) = manifests_dir() else { return 1 };
    let manifests = read_manifests(&dir, &mut log);
    let mut pids: Vec<u32> = manifests.iter().map(|manifest| manifest.pid).collect();
    pids.sort_unstable();
    pids.dedup();
    let stragglers: Vec<u32> = desktop_pids()
        .into_iter()
        .filter(|pid| !pids.contains(pid))
        .collect();
    if !stragglers.is_empty() {
        log.line(format!(
            "leaving {} process(es) running an older UI without restart support: {stragglers:?}",
            stragglers.len()
        ));
    }
    log.line(format!(
        "restarting {} window(s) across {} process(es): {pids:?}",
        manifests.len(),
        pids.len()
    ));
    let executable = match crate::platform::self_executable() {
        Ok(executable) => executable,
        Err(error) => {
            log.line(format!("cannot locate desktop executable: {error}"));
            return 1;
        }
    };
    let restore_dir = root.join(format!("restore-{}", now_ms()));
    // Stage every snapshot before anything stops.
    if let Err(error) = fs::create_dir_all(&restore_dir) {
        log.line(format!("cannot create {}: {error}", restore_dir.display()));
        return 1;
    }
    stop_processes(&pids, &mut log);
    for manifest in &manifests {
        let _ = fs::remove_file(dir.join(format!("{}-{}.json", manifest.pid, manifest.window)));
    }
    if restart_server {
        reload_server(&mut log);
    }
    relaunch(&executable, &restore_dir, &manifests, &mut log);
    // Relaunched windows consume their files. Leave the directory briefly for
    // slow starts, then remove whatever is left from older restarts.
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(3600));
            if path != restore_dir
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("restore-"))
                && old
            {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
    log.line("done");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_launch_drops_transient_flags() {
        let args: Vec<String> = [
            "--single-panel",
            "--new-process",
            "--global-voice-hold",
            "--hot-reload",
            "--session=abc",
            "--restore-state=/tmp/x.json",
            "--resume",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            strip_transient(&args),
            vec!["--single-panel", "--hot-reload", "--resume"]
        );
        assert!(jcode_desktop_api::LaunchMode::shared_single_panel(
            &strip_transient(&args)
        ));
    }

    #[test]
    fn restore_state_discards_dead_terminal_handles_and_transient_launch() {
        let bytes = br#"{"format_version":1,"slots":[{"panel":{"session_id":"x","title":"t","working_dir":"/tmp","draft":{"content":"hello","selection_start":0,"selection_end":0,"selection_reversed":false,"history":[],"history_index":null,"live_draft":"","attachments":[]},"scroll_x":0,"scroll_y":0,"stick_to_bottom":true,"terminal_resource_id":7,"terminal_output_cursor":9},"row":0,"width_fraction":0.5,"restore_fraction":null}],"active":0,"active_row":0,"row_focus":[null,null,null,null],"previous":null,"camera_x":[0,0,0,0],"camera_target":[0,0,0,0],"overview":false,"hints_overlay":false,"folder_picker_dir":null,"folder_picker_error":null,"folder_search":null,"focus":"Workspace","launch":{"args":["--single-panel","--new-process","--session=x"],"env":[]}}"#;
        let snapshot = decode_restore(bytes).unwrap();
        assert_eq!(snapshot.slots[0].panel.session_id, "terminal");
        assert_eq!(snapshot.slots[0].panel.terminal_resource_id, None);
        assert_eq!(snapshot.slots[0].panel.terminal_output_cursor, None);
        assert_eq!(snapshot.slots[0].panel.draft.content, "hello");
        assert_eq!(snapshot.launch.unwrap().args, vec!["--single-panel"]);
    }

    #[test]
    fn workspace_windows_relaunch_before_chats() {
        let rank =
            |args: &[&str]| launch_rank(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert!(rank(&["--hot-reload"]) < rank(&["--no-sidebar"]));
        assert!(rank(&["--no-sidebar"]) < rank(&["--single-panel"]));
    }
}
