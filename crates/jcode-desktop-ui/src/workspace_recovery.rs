//! Short-lived crash checkpoints, separate from in-process hot reload snapshots.
//! A dead owner plus a recent heartbeat is required. Quitting normally disarms
//! recovery, and generation tokens prevent retired UI writers racing a reload.
use super::{Workspace, WorkspaceSnapshot};
use gpui::{App, Entity, Window};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RECOVERY_WINDOW_MS: u64 = 30_000;
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Deserialize, Serialize)]
struct Checkpoint {
    owner: String,
    pid: u32,
    updated_at_ms: u64,
    snapshot: WorkspaceSnapshot,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn filename(mode: jcode_desktop_api::LaunchMode) -> Option<&'static str> {
    // Standalone invocations are always fresh. They must neither consume nor
    // overwrite the main workspace's checkpoint (or another standalone's).
    // In-process Ctrl+R snapshots remain available without a recovery file.
    match mode {
        jcode_desktop_api::LaunchMode::SinglePanel => None,
        jcode_desktop_api::LaunchMode::NoSidebar => Some("crash-recovery-no-sidebar.json"),
        jcode_desktop_api::LaunchMode::Workspace => Some("crash-recovery.json"),
    }
}

fn path() -> Option<PathBuf> {
    let name = filename(jcode_desktop_api::LaunchMode::from_args(std::env::args_os()))?;
    Some(crate::learning::state_path()?.with_file_name(name))
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return true;
    }
    #[cfg(unix)]
    {
        // EPERM is also a live process. Be conservative if process inspection fails.
        (unsafe { libc::kill(pid as i32, 0) == 0 })
            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        // Do not steal another independent Windows instance's checkpoint.
        true
    }
}

fn eligible(checkpoint: &Checkpoint, now: u64, alive: bool) -> bool {
    !alive
        && now
            .checked_sub(checkpoint.updated_at_ms)
            .is_some_and(|age| age <= RECOVERY_WINDOW_MS)
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

// All generations use this same short-lived lock, including across cdylibs.
// Never remove the lock inode, which could let two writers hold different locks.
fn lock(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path.with_extension("lock"))?;
    file.lock()?;
    Ok(file)
}

fn read(path: &Path) -> Option<Checkpoint> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write(path: &Path, checkpoint: &Checkpoint) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = private_file(&temporary)?;
        serde_json::to_writer(&mut file, checkpoint)?;
        file.flush()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn load() -> Option<WorkspaceSnapshot> {
    let path = path()?;
    let _lock = lock(&path).ok()?;
    let checkpoint = read(&path)?;
    let now = now_ms();
    let snapshot = restore(&checkpoint, now, process_alive(checkpoint.pid))?;
    eprintln!(
        "restoring desktop panels from recent crash ({} ms since checkpoint)",
        now.saturating_sub(checkpoint.updated_at_ms)
    );
    Some(snapshot)
}

fn restore(checkpoint: &Checkpoint, now: u64, alive: bool) -> Option<WorkspaceSnapshot> {
    if !eligible(checkpoint, now, alive) {
        return None;
    }
    // Validate the workspace schema as strictly as the hot-reload boundary.
    let mut snapshot = WorkspaceSnapshot::decode(&checkpoint.snapshot.encode().ok()?).ok()?;
    for slot in &mut snapshot.slots {
        // PTY resource handles and output cursors belong to the dead host.
        // Reopen in its directory without suppressing the new PTY's replies.
        if slot.panel.terminal_resource_id.take().is_some() {
            slot.panel.session_id = "terminal".into();
        }
        slot.panel.terminal_output_cursor = None;
    }
    Some(snapshot)
}

#[derive(Clone)]
struct Writer {
    path: PathBuf,
    owner: String,
}

impl Writer {
    fn begin(path: PathBuf, snapshot: WorkspaceSnapshot) -> anyhow::Result<Self> {
        let _lock = lock(&path)?;
        if let Some(previous) = read(&path) {
            anyhow::ensure!(
                previous.pid == std::process::id() || !process_alive(previous.pid),
                "another desktop process owns the crash checkpoint"
            );
        }
        let owner = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let writer = Self { path, owner };
        write(&writer.path, &writer.checkpoint(snapshot, now_ms()))?;
        Ok(writer)
    }

    fn checkpoint(&self, snapshot: WorkspaceSnapshot, updated_at_ms: u64) -> Checkpoint {
        Checkpoint {
            owner: self.owner.clone(),
            pid: std::process::id(),
            updated_at_ms,
            snapshot,
        }
    }

    fn save(&self, snapshot: WorkspaceSnapshot, updated_at_ms: u64) -> anyhow::Result<bool> {
        let _lock = lock(&self.path)?;
        if read(&self.path).is_none_or(|record| record.owner != self.owner) {
            return Ok(false);
        }
        write(&self.path, &self.checkpoint(snapshot, updated_at_ms))?;
        Ok(true)
    }

    fn clean_quit(&self) -> anyhow::Result<()> {
        let _lock = lock(&self.path)?;
        if read(&self.path).is_some_and(|record| record.owner == self.owner) {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

/// Install at the plugin boundary so Ctrl+R enables recovery in existing hosts.
/// The loop retains only a weak workspace, never an old generation's panels.
pub(crate) fn install(workspace: &Entity<Workspace>, window: &Window, app: &mut App) {
    let Some(path) = path() else { return };
    let writer = workspace
        .read(app)
        .snapshot(window, app)
        .and_then(|snapshot| Writer::begin(path, snapshot));
    let writer = match writer {
        Ok(writer) => writer,
        Err(error) => {
            eprintln!("desktop crash checkpoint unavailable: {error:#}");
            return;
        }
    };
    let quit_writer = writer.clone();
    app.on_app_quit(move |_| {
        // Synchronous disarm also excludes any in-flight background save.
        if let Err(error) = quit_writer.clean_quit() {
            eprintln!("failed to disarm desktop crash recovery: {error:#}");
        }
        std::future::ready(())
    })
    .detach();
    let workspace = workspace.downgrade();
    let window = window.window_handle();
    app.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(CHECKPOINT_INTERVAL).await;
            let snapshot = window.update(cx, |_, window, app| {
                workspace
                    .upgrade()
                    .map(|workspace| workspace.read(app).snapshot(window, app))
            });
            let Ok(Some(Ok(snapshot))) = snapshot else {
                break;
            };
            let captured_at = now_ms();
            let writer = writer.clone();
            match cx
                .background_executor()
                .spawn(async move { writer.save(snapshot, captured_at) })
                .await
            {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => eprintln!("desktop crash checkpoint failed: {error:#}"),
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot::decode(br#"{"format_version":1,"slots":[],"active":0,"active_row":0,"row_focus":[null,null,null,null],"previous":null,"camera_x":[0,0,0,0],"camera_target":[0,0,0,0],"overview":false,"hints_overlay":false,"folder_picker_dir":null,"folder_picker_error":null,"folder_search":null,"focus":"Workspace"}"#).unwrap()
    }

    #[test]
    fn single_panel_never_consumes_or_overwrites_workspace_recovery() {
        use jcode_desktop_api::LaunchMode;
        assert_eq!(filename(LaunchMode::SinglePanel), None);
        assert_eq!(filename(LaunchMode::Workspace), Some("crash-recovery.json"));
        assert_eq!(filename(LaunchMode::NoSidebar), Some("crash-recovery-no-sidebar.json"));
    }

    #[test]
    fn only_recent_dead_owners_are_recoverable() {
        let record = Checkpoint {
            owner: "old".into(),
            pid: 123,
            updated_at_ms: 100_000,
            snapshot: snapshot(),
        };
        assert!(eligible(&record, 100_000, false));
        assert!(eligible(&record, 130_000, false));
        assert!(!eligible(&record, 130_001, false));
        assert!(!eligible(&record, 99_999, false));
        assert!(!eligible(&record, 100_001, true));
        assert!(process_alive(std::process::id()));
        assert!(process_alive(0));
    }

    #[test]
    fn recovery_validates_schema_and_discards_dead_host_resources() {
        let mut state = serde_json::to_value(snapshot()).unwrap();
        state["slots"] = serde_json::json!([{
            "panel": {
                "session_id": "terminal", "title": "shell", "working_dir": "/project",
                "draft": {"content": "draft", "selection_start": 5, "selection_end": 5,
                    "selection_reversed": false, "history": [], "history_index": null,
                    "live_draft": "", "attachments": []},
                "scroll_x": 0, "scroll_y": 0, "stick_to_bottom": true,
                "terminal_resource_id": 42, "terminal_output_cursor": 1234
            },
            "row": 1, "width_fraction": 0.5, "restore_fraction": null
        }]);
        let mut record = Checkpoint {
            owner: "dead".into(),
            pid: 123,
            updated_at_ms: 100_000,
            snapshot: serde_json::from_value(state).unwrap(),
        };
        let restored = restore(&record, 110_000, false).unwrap();
        assert_eq!(restored.slots[0].panel.terminal_resource_id, None);
        assert_eq!(restored.slots[0].panel.terminal_output_cursor, None);
        assert_eq!(restored.slots[0].panel.session_id, "terminal");
        assert_eq!(restored.slots[0].row, 1);
        assert_eq!(
            restored.slots[0].panel.working_dir.as_deref(),
            Some("/project")
        );
        assert_eq!(
            record.snapshot.slots[0].panel.terminal_resource_id,
            Some(42),
            "hot-reload snapshots remain untouched"
        );
        assert_eq!(
            record.snapshot.slots[0].panel.terminal_output_cursor,
            Some(1234)
        );
        record.snapshot.format_version = 999;
        assert!(restore(&record, 110_000, false).is_none());
        record.snapshot.format_version = 1;
        record.snapshot.slots[0].row = super::super::STRIP_COUNT;
        assert!(restore(&record, 110_000, false).is_none());
    }

    #[test]
    fn quit_disarms_and_late_save_cannot_recreate_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("recovery.json");
        let writer = Writer::begin(path.clone(), snapshot()).unwrap();
        assert!(writer.save(snapshot(), 123).unwrap());
        assert_eq!(read(&path).unwrap().updated_at_ms, 123);
        writer.clean_quit().unwrap();
        assert!(!path.exists());
        assert!(!writer.save(snapshot(), 456).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn reload_retires_old_writer_without_disarming_new_generation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("recovery.json");
        let old = Writer::begin(path.clone(), snapshot()).unwrap();
        let new = Writer::begin(path.clone(), snapshot()).unwrap();
        assert_ne!(old.owner, new.owner);
        assert!(!old.save(snapshot(), 123).unwrap());
        old.clean_quit().unwrap();
        assert_eq!(read(&path).unwrap().owner, new.owner);
        assert!(new.save(snapshot(), 456).unwrap());
    }

    #[test]
    fn corrupt_checkpoint_is_replaced_and_files_are_private() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("recovery.json");
        fs::write(&path, b"partial json").unwrap();
        assert!(read(&path).is_none());
        let writer = Writer::begin(path.clone(), snapshot()).unwrap();
        assert_eq!(read(&path).unwrap().owner, writer.owner);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.with_extension("lock"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
