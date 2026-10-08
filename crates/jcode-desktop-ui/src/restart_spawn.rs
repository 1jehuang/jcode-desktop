//! Starting `/restart-all`'s detached worker, without the workspace.
//!
//! The panel's `/restart-all` command only needs to spawn the worker, so this
//! lives outside `workspace::restart` (which owns manifests, snapshots and the
//! worker itself). That keeps `panel` independent of `workspace`.
use std::process::{Command, Stdio};

pub const WORKER_FLAG: &str = "--restart-all-worker";
pub const NO_SERVER_FLAG: &str = "--no-server";

/// Start the detached worker. It outlives this process, which it stops.
pub(crate) fn spawn_worker(restart_server: bool) -> anyhow::Result<()> {
    let executable = crate::platform::self_executable()?;
    let mut command = Command::new(executable);
    command.arg(WORKER_FLAG);
    if !restart_server {
        command.arg(NO_SERVER_FLAG);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command.spawn()?;
    Ok(())
}

/// Put a child in its own session so it survives this process exiting.
pub(crate) fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and only affects the child.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = command;
}
