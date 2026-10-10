//! Automatic updates for packaged Linux, FreeBSD and Windows builds.
//!
//! Mirrors what Sparkle does on macOS: check at launch and periodically,
//! download and install the new release in the background, then offer a
//! restart. Restarting relaunches every open window onto the new build with
//! its workspace restored. Quitting and reopening also runs the new build.
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use super::{UpdateState, set};

/// How often a long-running Desktop looks for a new release.
const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
/// Let startup finish before touching the network.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);

static RUNNING: AtomicBool = AtomicBool::new(false);
static SCHEDULED: AtomicBool = AtomicBool::new(false);
/// The installed build a restart should launch.
static RELAUNCH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

pub(crate) fn supported() -> bool {
    jcode_desktop_updater::Target::current().is_ok()
        && crate::update_entry::release_version().is_some()
}

/// Start the background update loop once per process.
pub(super) fn schedule() {
    if SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("desktop-auto-update".into())
        .spawn(|| {
            let delay = if std::env::var_os("JCODE_DESKTOP_LIVE_UPDATE_TEST").is_some() {
                Duration::from_secs(1)
            } else {
                FIRST_CHECK_DELAY
            };
            std::thread::sleep(delay);
            if let Some(running) = crate::update_entry::release_version() {
                jcode_desktop_updater::cleanup_after_update(&running);
            }
            loop {
                run(false);
                std::thread::sleep(CHECK_INTERVAL);
            }
        });
}

/// A user-requested check, as for `/update` or the version pill.
pub(super) fn start() {
    let _ = std::thread::Builder::new()
        .name("desktop-update".into())
        .spawn(|| run(true));
}

fn run(requested: bool) {
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(running) = crate::update_entry::release_version() else {
        RUNNING.store(false, Ordering::Release);
        return;
    };
    // A finished download stays offered until the user restarts.
    if matches!(super::current(), UpdateState::ReadyToRestart { .. }) && !requested {
        RUNNING.store(false, Ordering::Release);
        return;
    }
    if requested {
        set(UpdateState::Checking);
    }
    let result = std::panic::catch_unwind(|| {
        jcode_desktop_updater::update(&running, &mut |progress| match progress {
            jcode_desktop_updater::Progress::Downloading { version } => {
                set(UpdateState::Available {
                    version: version.to_string(),
                })
            }
        })
    })
    .unwrap_or_else(|_| {
        Err(jcode_desktop_updater::Failure {
            stage: "panic",
            kind: None,
            error: anyhow::anyhow!("the updater unexpectedly stopped"),
        })
    });
    crate::update_entry::report(crate::update_entry::RELEASE_VERSION, &result);
    set(match result {
        Ok(jcode_desktop_updater::Outcome::UpToDate { version }) => {
            if requested {
                UpdateState::Finished {
                    message: format!("Jcode Desktop {version} is up to date."),
                }
            } else {
                UpdateState::Idle
            }
        }
        Ok(jcode_desktop_updater::Outcome::Installed {
            version, relaunch, ..
        }) => {
            *RELAUNCH.lock().unwrap_or_else(|error| error.into_inner()) = Some(relaunch);
            UpdateState::ReadyToRestart {
                version: version.to_string(),
            }
        }
        // Background failures stay quiet and retry later. A requested update
        // explains what went wrong.
        Err(failure) if requested => UpdateState::Failed {
            message: format!("Jcode Desktop update failed: {failure}. Run /update to retry."),
        },
        Err(failure) => {
            eprintln!("background desktop update failed at {}: {failure}", failure.stage);
            UpdateState::Idle
        }
    });
    RUNNING.store(false, Ordering::Release);
}

/// Relaunch every window onto the installed update. Returns whether a restart
/// was started.
pub(super) fn install_now() -> bool {
    let Some(executable) = RELAUNCH
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
    else {
        return false;
    };
    match crate::restart_spawn::spawn_worker_with(false, Some(&executable)) {
        Ok(()) => true,
        Err(error) => {
            set(UpdateState::Failed {
                message: format!(
                    "Could not restart into the update ({error}). Quit and reopen Jcode Desktop to use it."
                ),
            });
            false
        }
    }
}
