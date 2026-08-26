//! Automatic-update status, surfaced to the user instead of happening silently.
//!
//! Sparkle already downloads and installs betas on a daily schedule, but it did
//! so invisibly: a user on a broken build had no way to know a fix existed, and
//! no way to ask for it now. This module is the shared state between the macOS
//! updater bootstrap and the workspace chrome.
//!
//! The direction of the dependency matters. Objective-C calls *into* Rust
//! through [`jcode_update_report`], and registers its "check now" entry point
//! through [`jcode_update_register_actions`]. Nothing here links against
//! Sparkle, so this crate still builds, links, and tests on its own, and the
//! render tests below drive exactly the states the delegate reports.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

/// What the updater is doing right now, in the user's terms.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum UpdateState {
    /// Nothing to say: the app is current, or has not looked yet.
    #[default]
    Idle,
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
    match current() {
        UpdateState::Idle => {
            if call_action(&CHECK_NOW) {
                set(UpdateState::Checking);
                UpdateRequest::Checking
            } else {
                UpdateRequest::Unavailable
            }
        }
        UpdateState::Checking => UpdateRequest::AlreadyChecking,
        UpdateState::Available { .. } => UpdateRequest::Downloading,
        UpdateState::ReadyToRestart { .. } => {
            if install_now() {
                UpdateRequest::Restarting
            } else {
                UpdateRequest::Unavailable
            }
        }
    }
}

/// Ask the platform updater to install the staged build and relaunch.
/// Returns whether an installer was actually available to call.
pub fn install_now() -> bool {
    call_action(&INSTALL_NOW)
}

#[cfg(test)]
mod tests {
    use super::*;

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
