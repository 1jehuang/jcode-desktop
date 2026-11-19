//! Omarchy desktop detection.
//!
//! Home for Omarchy-specific behavior as it grows (keybinding copy today,
//! theme-awareness later), keyed off a real signal rather than guessing from
//! `target_os`. Omarchy exports `OMARCHY_PATH` for its own tooling to locate
//! itself; `DESKTOP_SESSION=omarchy` is a fallback for sessions that strip
//! custom environment variables before launching apps.

use std::env;

/// True when running inside an Omarchy session.
pub fn detected() -> bool {
    detected_from(env::var_os("OMARCHY_PATH").is_some(), env::var("DESKTOP_SESSION").ok())
}

fn detected_from(has_omarchy_path: bool, desktop_session: Option<String>) -> bool {
    has_omarchy_path || desktop_session.as_deref() == Some("omarchy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omarchy_path_alone_is_sufficient() {
        assert!(detected_from(true, None));
    }

    #[test]
    fn matching_desktop_session_alone_is_sufficient() {
        assert!(detected_from(false, Some("omarchy".into())));
    }

    #[test]
    fn neither_signal_is_insufficient() {
        assert!(!detected_from(false, None));
        assert!(!detected_from(false, Some("gnome".into())));
    }
}
