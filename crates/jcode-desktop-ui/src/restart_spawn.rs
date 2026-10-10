//! Starting `/restart-all`'s detached worker, without the workspace.
//!
//! The panel's `/restart-all` command only needs to spawn the worker, so this
//! lives outside `workspace::restart` (which owns manifests, snapshots and the
//! worker itself). That keeps `panel` independent of `workspace`.
use std::process::{Command, Stdio};

pub const WORKER_FLAG: &str = "--restart-all-worker";
pub const NO_SERVER_FLAG: &str = "--no-server";
/// Relaunch windows with this executable instead of the running one, so an
/// update restart starts the newly installed build.
pub const RELAUNCH_FLAG: &str = "--relaunch-executable=";

/// Start the detached worker. It outlives this process, which it stops.
pub(crate) fn spawn_worker(restart_server: bool) -> anyhow::Result<()> {
    spawn_worker_with(restart_server, None)
}

/// [`spawn_worker`], relaunching onto `relaunch` when given. The worker runs
/// from the new executable too, so its restart logic is the updated one.
pub(crate) fn spawn_worker_with(
    restart_server: bool,
    relaunch: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let executable = match relaunch {
        Some(executable) => executable.to_path_buf(),
        None => crate::platform::self_executable()?,
    };
    let mut command = Command::new(&executable);
    command.arg(WORKER_FLAG);
    if !restart_server {
        command.arg(NO_SERVER_FLAG);
    }
    if let Some(relaunch) = relaunch {
        let mut flag = std::ffi::OsString::from(RELAUNCH_FLAG);
        flag.push(relaunch);
        command.arg(flag);
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
