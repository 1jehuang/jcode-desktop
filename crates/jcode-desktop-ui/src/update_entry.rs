//! Process entry points for updating: the headless `jcode-desktop --update`,
//! the launch-time redirect to a newer adopted install, and the background
//! updater that downloads new releases while Jcode Desktop runs.
//!
//! All three share `jcode_desktop_updater`, so the command CI runs on every
//! platform is the code users get.
use std::path::Path;

/// The published release this binary was built as.
pub const RELEASE_VERSION: &str = env!("JCODE_DESKTOP_VERSION");

pub fn release_version() -> Option<semver::Version> {
    jcode_desktop_updater::parse_version(RELEASE_VERSION).ok()
}

/// Report a finished update attempt. Content-free: versions, install shape,
/// and a short stage token, never paths or error text.
pub(crate) fn report(
    running: &str,
    result: &Result<jcode_desktop_updater::Outcome, jcode_desktop_updater::Failure>,
) {
    use jcode_desktop_updater::Outcome;
    let (to, kind, outcome, stage) = match result {
        Ok(Outcome::UpToDate { version }) => (version.to_string(), None, "up_to_date", None),
        Ok(Outcome::Installed { version, kind, fresh: true, .. }) => {
            (version.to_string(), Some(*kind), "success", None)
        }
        // Already reported by the attempt that installed it.
        Ok(Outcome::Installed { fresh: false, .. }) => return,
        Err(failure) => (String::new(), failure.kind, "failure", Some(failure.stage)),
    };
    jcode_base::telemetry::record_desktop_update(
        running,
        &to,
        kind.map_or("unknown", |kind| kind.as_str()),
        outcome,
        stage,
    );
}

/// `jcode-desktop --update`: update this installation, print the result, and
/// exit. Returns the process exit code, or `None` for every other launch.
pub fn run_update_command_if_requested() -> Option<i32> {
    if !std::env::args_os().skip(1).any(|arg| arg == "--update") {
        return None;
    }
    let Some(running) = release_version() else {
        eprintln!("Jcode Desktop {RELEASE_VERSION} is not a published release version");
        return Some(2);
    };
    println!("Jcode Desktop {running}: checking for updates");
    let result = jcode_desktop_updater::update(&running, &mut |progress| match progress {
        jcode_desktop_updater::Progress::Downloading { version } => {
            println!("Downloading Jcode Desktop {version}");
        }
    });
    report(RELEASE_VERSION, &result);
    jcode_base::telemetry::flush_pending(std::time::Duration::from_secs(5));
    Some(match result {
        Ok(jcode_desktop_updater::Outcome::UpToDate { version }) => {
            println!("Jcode Desktop {version} is up to date");
            0
        }
        Ok(jcode_desktop_updater::Outcome::Installed { version, relaunch, kind, .. }) => {
            println!(
                "Installed Jcode Desktop {version} ({}) at {}",
                kind.as_str(),
                relaunch.display()
            );
            0
        }
        Err(failure) => {
            eprintln!("Update failed at {}: {failure}", failure.stage);
            1
        }
    })
}

/// Launch a newer adopted install instead of this one. Never returns when it
/// redirects. A system package (for example a .deb in /usr/bin) cannot update
/// itself, so its first update installs a per-user copy, and every later
/// launch of the system copy runs that copy while it is newer.
pub fn redirect_to_newer_install_if_present() {
    let Some(running) = release_version() else {
        return;
    };
    let Some(target) = jcode_desktop_updater::redirect_target(&running) else {
        return;
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Err(error) = exec(&target, &args) {
        eprintln!(
            "could not start the newer Jcode Desktop at {}: {error}",
            target.display()
        );
    }
}

#[cfg(unix)]
fn exec(target: &Path, args: &[std::ffi::OsString]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    // Guard against a redirect loop if version files ever disagree.
    Err(std::process::Command::new(target)
        .args(args)
        .env("JCODE_DESKTOP_NO_REDIRECT", "1")
        .exec())
}

#[cfg(not(unix))]
fn exec(target: &Path, args: &[std::ffi::OsString]) -> std::io::Result<()> {
    let status = std::process::Command::new(target)
        .args(args)
        .env("JCODE_DESKTOP_NO_REDIRECT", "1")
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}
