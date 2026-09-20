//! Shared desktop updater state. macOS registers Sparkle callbacks. Linux
//! rebuilds source checkouts through the host or updates managed user bundles.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_package;
mod release;

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

/// Public release availability, independent of installer/download state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ReleaseStatus {
    #[default]
    Unknown,
    Checking,
    /// Running release is equal to or newer than the latest public release.
    Current {
        latest: String,
    },
    Newer {
        version: String,
    },
    Error {
        message: String,
    },
    /// A checkout build cannot claim to be a published release.
    Source,
}

fn release_state() -> &'static Mutex<ReleaseStatus> {
    static STATE: OnceLock<Mutex<ReleaseStatus>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ReleaseStatus::Unknown))
}

pub fn release_status() -> ReleaseStatus {
    release_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn store_release_status(next: ReleaseStatus) {
    *release_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = next;
}

#[cfg(test)]
pub fn set_release_status(next: ReleaseStatus) {
    store_release_status(next);
}

fn source_build() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(checkout) = manifest.parent().and_then(std::path::Path::parent) else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        linux::source_executable(&executable, checkout)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(relative) = executable.strip_prefix(checkout.join("target")) else {
            return false;
        };
        let components: Vec<_> = relative.iter().collect();
        checkout.join("Cargo.toml").is_file()
            && match components.as_slice() {
                [profile, binary] | [_, profile, binary] => {
                    (*profile == "debug" || *profile == "release")
                        && (*binary == "jcode-desktop" || *binary == "jcode-desktop.exe")
                }
                _ => false,
            }
    }
}

fn compare_release(running: &str, latest: semver::Version) -> anyhow::Result<ReleaseStatus> {
    let running = semver::Version::parse(running)?;
    // Build metadata does not affect SemVer precedence.
    Ok(if latest.cmp_precedence(&running).is_gt() {
        ReleaseStatus::Newer {
            version: latest.to_string(),
        }
    } else {
        ReleaseStatus::Current {
            latest: latest.to_string(),
        }
    })
}

fn fixture_release_status(value: &str) -> ReleaseStatus {
    match value {
        "current" => ReleaseStatus::Current {
            latest: crate::build_info::VERSION.into(),
        },
        "newer" => ReleaseStatus::Newer {
            version: "0.2.0-beta.1".into(),
        },
        "checking" => ReleaseStatus::Checking,
        "error" => ReleaseStatus::Error {
            message: "Could not reach the release server".into(),
        },
        "source" => ReleaseStatus::Source,
        _ => ReleaseStatus::Unknown,
    }
}

/// Safe to call from every render. Only the first call starts a read-only worker.
/// This never invokes request_now, Sparkle, an installer, or a source rebuild.
/// UI rendering observes completion via release_status, with no host ABI changes.
pub fn ensure_release_check() {
    let mut state = release_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if *state != ReleaseStatus::Unknown {
        return;
    }
    if crate::harness::screenshot_mode() {
        // Offline screenshots must never contact the network.
        *state = fixture_release_status(
            &std::env::var("JCODE_DESKTOP_SCREENSHOT_RELEASE_STATUS")
                .unwrap_or_else(|_| "source".into()),
        );
        return;
    }
    // Render tests run from Cargo test executables, not a packaged release.
    // Never let a unit test start external network work.
    if cfg!(test) || source_build() {
        *state = ReleaseStatus::Source;
        return;
    }
    *state = ReleaseStatus::Checking;
    drop(state);
    if let Err(error) = std::thread::Builder::new()
        .name("desktop-release-check".into())
        .spawn(|| {
            let result = std::panic::catch_unwind(|| {
                compare_release(env!("JCODE_DESKTOP_VERSION"), release::latest()?)
            })
            .unwrap_or_else(|_| Err(anyhow::anyhow!("Release checker unexpectedly stopped")));
            store_release_status(match result {
                Ok(status) => status,
                Err(error) => ReleaseStatus::Error {
                    message: format!("{error:#}"),
                },
            });
        })
    {
        store_release_status(ReleaseStatus::Error {
            message: format!("Could not start release checker: {error}"),
        });
    }
}

/// What the updater is doing right now, in the user's terms.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum UpdateState {
    /// Nothing to say: the app is current, or has not looked yet.
    #[default]
    Idle,
    /// A Linux request completed without forcibly restarting the user's work.
    Finished { message: String },
    /// A recoverable error. A subsequent `/update` retries the operation.
    Failed { message: String },
    /// A scheduled or user-requested check is in flight.
    Checking,
    /// A newer build exists and Sparkle is fetching it.
    Available { version: String },
    /// The download is staged; the next launch runs the new build.
    ReadyToRestart { version: String },
}

impl UpdateState {
    /// The chip's text. `None` means the chip should not paint at all, which
    /// keeps the quiet case genuinely quiet.
    pub fn label(&self) -> Option<String> {
        match self {
            Self::Idle => None,
            Self::Finished { .. } => Some("desktop update status · see result".into()),
            Self::Failed { .. } => Some("desktop update failed · see /update result".into()),
            Self::Checking => Some("checking for updates".to_owned()),
            Self::Available { version } => Some(format!("downloading {version}")),
            Self::ReadyToRestart { version } => Some(format!("{version} ready · restart")),
        }
    }

    /// Whether this state is still in motion. Finished work stops animating so
    /// a permanently pulsing chip never becomes background noise.
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Available { .. })
    }

    /// Restarting is the only action the user can usefully take from the chip.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::ReadyToRestart { .. })
    }
}

fn state() -> &'static Mutex<UpdateState> {
    static STATE: OnceLock<Mutex<UpdateState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(UpdateState::Idle))
}

/// The updater's current state, for the render path.
pub fn current() -> UpdateState {
    state()
        .lock()
        .map(|state| state.clone())
        .unwrap_or_default()
}

/// Record a new state. Used by the Objective-C delegate and by tests.
pub fn set(next: UpdateState) {
    if let Ok(mut state) = state().lock() {
        *state = next;
    }
}

/// Numeric mirror of [`UpdateState`], shared with `updater_bootstrap.m`.
/// Keep these values in sync with the `JcodeUpdate*` constants there.
pub const STATE_IDLE: u32 = 0;
pub const STATE_CHECKING: u32 = 1;
pub const STATE_AVAILABLE: u32 = 2;
pub const STATE_READY: u32 = 3;

/// Called by the macOS updater bootstrap whenever Sparkle changes state.
///
/// # Safety
/// `version` must be null or a valid NUL-terminated C string that stays valid
/// for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn jcode_update_report(state: u32, version: *const c_char) {
    let version = if version.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(version) }
            .to_string_lossy()
            .into_owned()
    };
    set(from_parts(state, version));
}

/// Translate the C-level report into the typed state. Split out so a test can
/// exercise the mapping without constructing C strings.
pub(crate) fn from_parts(state: u32, version: String) -> UpdateState {
    let version = if version.trim().is_empty() {
        "a new version".to_owned()
    } else {
        version
    };
    match state {
        STATE_CHECKING => UpdateState::Checking,
        STATE_AVAILABLE => UpdateState::Available { version },
        STATE_READY => UpdateState::ReadyToRestart { version },
        _ => UpdateState::Idle,
    }
}

/// Entry point registered by the platform layer to relaunch into the staged
/// update. Stored as a raw pointer so this crate never links against Sparkle.
static CHECK_NOW: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static INSTALL_NOW: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Result of asking the platform updater to act now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateRequest {
    Checking,
    AlreadyChecking,
    Downloading,
    Restarting,
    Unavailable,
}

/// Called by the macOS updater bootstrap once Sparkle is live.
///
/// # Safety
/// Both arguments must be valid `extern "C" fn()` pointers that stay valid for
/// the lifetime of the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn jcode_update_register_actions(
    check_now: extern "C" fn(),
    install_now: extern "C" fn(),
) {
    CHECK_NOW.store(check_now as *mut c_void, Ordering::Release);
    INSTALL_NOW.store(install_now as *mut c_void, Ordering::Release);
}

fn call_action(action: &AtomicPtr<c_void>) -> bool {
    let pointer = action.load(Ordering::Acquire);
    if pointer.is_null() {
        return false;
    }
    // Safety: only `jcode_update_register_actions` stores these pointers, and
    // it requires `extern "C" fn()` values valid for the process lifetime.
    let action: extern "C" fn() = unsafe { std::mem::transmute(pointer) };
    action();
    true
}

/// Check for an update, continue an active download, or install a staged build.
/// This is the single entry point used by `/update`.
pub fn request_now() -> UpdateRequest {
    // Claim the request before calling the platform. Callbacks may complete
    // synchronously, so setting Checking afterwards loses their final result.
    let previous = {
        let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
        match &*state {
            UpdateState::Checking => return UpdateRequest::AlreadyChecking,
            UpdateState::Available { .. } => return UpdateRequest::Downloading,
            UpdateState::ReadyToRestart { .. } => {
                drop(state);
                return if install_now() {
                    UpdateRequest::Restarting
                } else {
                    UpdateRequest::Unavailable
                };
            }
            _ => std::mem::replace(&mut *state, UpdateState::Checking),
        }
    };
    if call_action(&CHECK_NOW) {
        return UpdateRequest::Checking;
    }
    #[cfg(all(target_os = "linux", not(test)))]
    {
        let _ = previous;
        linux::start();
        UpdateRequest::Checking
    }
    #[cfg(any(not(target_os = "linux"), test))]
    {
        set(previous);
        UpdateRequest::Unavailable
    }
}

/// Ask the platform updater to install the staged build and relaunch.
/// Returns whether an installer was actually available to call.
pub fn install_now() -> bool {
    call_action(&INSTALL_NOW)
}

#[cfg(test)]
pub(crate) fn clear_test_actions() {
    CHECK_NOW.store(std::ptr::null_mut(), Ordering::Release);
    INSTALL_NOW.store(std::ptr::null_mut(), Ordering::Release);
    set(UpdateState::Idle);
}

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_comparison_respects_numeric_beta_and_stable_precedence() {
        for (running, latest, newer) in [
            ("0.1.0-beta.9", "0.1.0-beta.10", true),
            ("0.1.0-beta.10", "0.1.0-beta.9", false),
            ("0.1.0-beta.10", "0.1.0-beta.10", false),
            ("0.1.0-beta.99", "0.1.0", true),
            ("0.1.0", "0.1.0-beta.99", false),
            ("0.1.0+local", "0.1.0", false),
            ("0.2.0", "0.1.0", false),
            ("0.3.0-dev.9", "0.3.0-dev.10", true),
            ("0.3.0-dev.10", "0.3.0", true),
            ("0.3.0-dev.10", "0.2.1", false),
        ] {
            let status = compare_release(running, semver::Version::parse(latest).unwrap()).unwrap();
            assert_eq!(
                matches!(status, ReleaseStatus::Newer { .. }),
                newer,
                "{running} versus {latest}"
            );
        }
        assert!(compare_release("not a version", semver::Version::new(1, 0, 0)).is_err());
    }

    #[test]
    fn release_status_is_independent_and_render_checks_do_not_repeat_or_install() {
        let _guard = test_lock();
        static CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        extern "C" fn unexpected() {
            CALLED.store(true, Ordering::Release);
        }
        unsafe { jcode_update_register_actions(unexpected, unexpected) };
        set(UpdateState::Idle);
        for status in [
            ReleaseStatus::Checking,
            ReleaseStatus::Current {
                latest: "0.1.0".into(),
            },
            ReleaseStatus::Newer {
                version: "0.2.0".into(),
            },
            ReleaseStatus::Error {
                message: "offline".into(),
            },
            ReleaseStatus::Source,
        ] {
            set_release_status(status.clone());
            ensure_release_check();
            ensure_release_check();
            assert_eq!(release_status(), status);
            assert_eq!(current(), UpdateState::Idle);
        }
        assert!(
            !CALLED.load(Ordering::Acquire),
            "release check invoked an updater action"
        );
        clear_test_actions();
        set_release_status(ReleaseStatus::Unknown);
    }

    #[test]
    fn offline_release_fixtures_cover_all_visible_states() {
        assert!(matches!(
            fixture_release_status("current"),
            ReleaseStatus::Current { .. }
        ));
        assert!(matches!(
            fixture_release_status("newer"),
            ReleaseStatus::Newer { .. }
        ));
        assert!(matches!(
            fixture_release_status("error"),
            ReleaseStatus::Error { .. }
        ));
        assert_eq!(fixture_release_status("checking"), ReleaseStatus::Checking);
        assert_eq!(fixture_release_status("source"), ReleaseStatus::Source);
    }

    #[test]
    fn synchronous_check_completion_is_not_overwritten() {
        let _guard = test_lock();
        extern "C" fn complete() {
            set(UpdateState::Finished {
                message: "Already current".into(),
            });
        }
        unsafe { jcode_update_register_actions(complete, complete) };
        set(UpdateState::Idle);
        assert_eq!(request_now(), UpdateRequest::Checking);
        assert!(matches!(current(), UpdateState::Finished { .. }));
        CHECK_NOW.store(std::ptr::null_mut(), Ordering::Release);
        INSTALL_NOW.store(std::ptr::null_mut(), Ordering::Release);
        set(UpdateState::Idle);
    }

    #[test]
    fn terminal_linux_states_are_visible_and_not_actionable() {
        for state in [
            UpdateState::Finished {
                message: "Restart when ready".into(),
            },
            UpdateState::Failed {
                message: "Network unavailable".into(),
            },
        ] {
            assert!(state.label().is_some());
            assert!(!state.is_busy());
            assert!(!state.is_actionable());
        }
    }

    #[test]
    fn a_quiet_updater_paints_nothing() {
        assert_eq!(UpdateState::Idle.label(), None);
        assert!(!UpdateState::Idle.is_busy());
        assert!(!UpdateState::Idle.is_actionable());
    }

    #[test]
    fn each_working_state_explains_itself_and_animates() {
        let checking = UpdateState::Checking;
        assert_eq!(checking.label().as_deref(), Some("checking for updates"));
        assert!(checking.is_busy());

        let available = UpdateState::Available {
            version: "0.1.0-beta.15".to_owned(),
        };
        assert_eq!(
            available.label().as_deref(),
            Some("downloading 0.1.0-beta.15")
        );
        assert!(available.is_busy());
        assert!(!available.is_actionable());
    }

    #[test]
    fn a_staged_update_stops_animating_and_offers_the_restart() {
        let ready = UpdateState::ReadyToRestart {
            version: "0.1.0-beta.15".to_owned(),
        };
        assert_eq!(
            ready.label().as_deref(),
            Some("0.1.0-beta.15 ready · restart")
        );
        assert!(!ready.is_busy(), "a finished download should stop pulsing");
        assert!(ready.is_actionable());
    }

    #[test]
    fn the_c_report_maps_onto_the_typed_state() {
        assert_eq!(from_parts(STATE_IDLE, String::new()), UpdateState::Idle);
        assert_eq!(
            from_parts(STATE_CHECKING, String::new()),
            UpdateState::Checking
        );
        assert_eq!(
            from_parts(STATE_AVAILABLE, "0.1.0-beta.15".to_owned()),
            UpdateState::Available {
                version: "0.1.0-beta.15".to_owned()
            }
        );
        assert_eq!(
            from_parts(STATE_READY, "0.1.0-beta.15".to_owned()),
            UpdateState::ReadyToRestart {
                version: "0.1.0-beta.15".to_owned()
            }
        );
    }

    #[test]
    fn a_version_less_report_still_reads_as_a_sentence() {
        // Sparkle can omit a display version string. The chip must not render
        // "downloading " with a dangling space.
        assert_eq!(
            from_parts(STATE_AVAILABLE, "   ".to_owned())
                .label()
                .as_deref(),
            Some("downloading a new version")
        );
    }

    #[test]
    fn reporting_through_the_c_abi_updates_what_the_ui_reads() {
        let _guard = test_lock();
        let version = std::ffi::CString::new("0.1.0-beta.15").unwrap();
        unsafe { jcode_update_report(STATE_READY, version.as_ptr()) };
        assert_eq!(
            current(),
            UpdateState::ReadyToRestart {
                version: "0.1.0-beta.15".to_owned()
            }
        );
        unsafe { jcode_update_report(STATE_IDLE, std::ptr::null()) };
        assert_eq!(current(), UpdateState::Idle);
    }

    #[test]
    fn a_registered_installer_is_invoked_and_reported() {
        let _guard = test_lock();
        // Source builds and tests have no Sparkle framework, so the chip must
        // report "nothing to run" rather than crashing. Once the platform
        // registers an entry point, clicking must actually reach it.
        static CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        extern "C" fn install() {
            CALLED.store(true, Ordering::Release);
        }

        assert!(
            INSTALL_NOW.load(Ordering::Acquire).is_null(),
            "no updater should be registered in a source build"
        );
        assert!(!install_now(), "an unregistered installer cannot run");

        unsafe { jcode_update_register_actions(install, install) };
        assert!(install_now(), "a registered installer should run");
        assert!(CALLED.load(Ordering::Acquire));

        INSTALL_NOW.store(std::ptr::null_mut(), Ordering::Release);
        CHECK_NOW.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn update_request_checks_when_idle_and_installs_when_ready() {
        let _guard = test_lock();
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        extern "C" fn action() {
            CALLS.fetch_add(1, Ordering::AcqRel);
        }

        unsafe { jcode_update_register_actions(action, action) };
        set(UpdateState::Idle);
        assert_eq!(request_now(), UpdateRequest::Checking);
        assert_eq!(current(), UpdateState::Checking);
        assert_eq!(request_now(), UpdateRequest::AlreadyChecking);

        set(UpdateState::ReadyToRestart {
            version: "0.1.0-beta.16".to_owned(),
        });
        assert_eq!(request_now(), UpdateRequest::Restarting);
        assert_eq!(CALLS.load(Ordering::Acquire), 2);

        INSTALL_NOW.store(std::ptr::null_mut(), Ordering::Release);
        CHECK_NOW.store(std::ptr::null_mut(), Ordering::Release);
        set(UpdateState::Idle);
    }
}
