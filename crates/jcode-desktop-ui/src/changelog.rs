//! Offline notes and explicit startup/reload acknowledgement.
//!
//! Merely rendering notes never changes persisted state. Call `should_open` only
//! for a real application startup or hot reload, not fixture/screenshot setup.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

const IDENTITY: &str = concat!(
    env!("JCODE_DESKTOP_VERSION"),
    "+",
    env!("JCODE_DESKTOP_BUILD_ID")
);
const MARKER: &str = "changelog-last-seen-build";

pub(crate) fn development() -> bool {
    include_development_snapshot(
        cfg!(debug_assertions),
        env::args_os().any(|arg| arg == "--hot-reload"),
        env::var_os("JCODE_DESKTOP_UI").is_some(),
    )
}

// Hot reload commonly uses an optimized release-profile plugin. The explicit
// development switches, not just Rust debug assertions, control its snapshot.
fn include_development_snapshot(debug: bool, hot_reload: bool, ui_override: bool) -> bool {
    debug || hot_reload || ui_override
}

#[cfg(test)]
fn render_markdown(development: bool) -> String {
    let mut notes = include_str!(concat!(env!("OUT_DIR"), "/changelog.md")).to_owned();
    if development {
        notes.push_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/changelog-debug.md"
        )));
    }
    notes
}

/// Acknowledge this build and return whether its notes should open.
///
/// First launches and changed version/build pairs open. Every real hot reload
/// opens, even for the same identity. Persistence is best effort: unavailable or
/// unreadable state shows notes again rather than hiding an update. Callers must
/// skip this function in isolated tests/screenshots unless testing it explicitly.
pub(crate) fn should_open(is_reload: bool) -> bool {
    let Some(path) = marker_path(
        env::var_os("JCODE_DESKTOP_STATE").map(PathBuf::from),
        env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    ) else {
        return true;
    };
    let open = acknowledge(&path, IDENTITY, is_reload);
    crate::update_notes::capture_unseen(&path.with_file_name("changelog-last-seen-commit"), open);
    open
}

fn marker_path(
    state: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    // JCODE_DESKTOP_STATE is the workspace's debug dump *file*. Keep the marker
    // beside it, never in the user's real state directory during isolated runs.
    if let Some(state) = state.filter(|path| !path.as_os_str().is_empty()) {
        return Some(state.parent().unwrap_or(Path::new(".")).join(MARKER));
    }
    xdg.filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.join("jcode-desktop").join(MARKER))
        .or_else(|| {
            home.filter(|path| !path.as_os_str().is_empty())
                .map(|path| path.join(".local/state/jcode-desktop").join(MARKER))
        })
}

fn decision(previous: Option<&str>, identity: &str, is_reload: bool) -> bool {
    is_reload || previous != Some(identity)
}

fn acknowledge(path: &Path, identity: &str, is_reload: bool) -> bool {
    let previous = fs::read_to_string(path).ok();
    let open = decision(previous.as_deref(), identity, is_reload);
    if open {
        // Avoid making read-only installations or corrupt state fatal at startup.
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if fs::create_dir_all(parent).is_ok() {
            let _ = fs::write(path, identity);
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let base = env::var_os("JCODE_SCRATCH_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(env::temp_dir);
            let path = base.join(format!(
                "changelog-test-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn first_launch_updates_and_every_reload_open() {
        assert!(decision(None, "1+100", false));
        assert!(!decision(Some("1+100"), "1+100", false));
        assert!(decision(Some("1+100"), "2+100", false));
        assert!(decision(Some("1+100"), "1+101", false));
        assert!(decision(Some("1+100"), "1+100", true));
        assert!(decision(Some(""), "1+100", false));
    }

    #[test]
    fn marker_persists_between_calls_and_reloads() {
        let scratch = Scratch::new();
        let path = scratch.0.join("nested").join(MARKER);
        assert!(acknowledge(&path, "1+100", false));
        assert_eq!(fs::read_to_string(&path).unwrap(), "1+100");
        assert!(!acknowledge(&path, "1+100", false));
        assert!(acknowledge(&path, "1+100", true));
        assert!(acknowledge(&path, "1+100", true));
        assert!(acknowledge(&path, "2+100", false));
        assert!(!acknowledge(&path, "2+100", false));
        assert!(acknowledge(&path, "2+101", false));
        assert_eq!(fs::read_to_string(&path).unwrap(), "2+101");
    }

    #[test]
    fn corrupt_or_unwritable_marker_does_not_hide_notes() {
        let scratch = Scratch::new();
        let path = scratch.0.join(MARKER);
        fs::write(&path, [0xff]).unwrap();
        assert!(acknowledge(&path, "1+100", false));
        assert_eq!(fs::read_to_string(&path).unwrap(), "1+100");
        assert!(acknowledge(&path.join("impossible-child"), "1+100", false));
    }

    #[test]
    fn state_override_is_isolated_and_fallbacks_match_desktop() {
        let p = |s: &str| Some(PathBuf::from(s));
        assert_eq!(
            marker_path(p("/isolated/state.txt"), p("/xdg"), p("/home")),
            p(&format!("/isolated/{MARKER}"))
        );
        assert_eq!(
            marker_path(None, p("/xdg"), p("/home")),
            p(&format!("/xdg/jcode-desktop/{MARKER}"))
        );
        assert_eq!(
            marker_path(None, p(""), p("/home")),
            p(&format!("/home/.local/state/jcode-desktop/{MARKER}"))
        );
        assert_eq!(marker_path(None, None, None), None);
        assert_eq!(marker_path(p("state.txt"), None, None), p(MARKER));
    }

    #[test]
    fn notes_embed_identity_and_snapshot_is_explicit() {
        let notes = render_markdown(false);
        assert!(notes.contains(env!("JCODE_DESKTOP_VERSION")));
        assert!(notes.contains(env!("JCODE_DESKTOP_BUILD_ID")));
        assert!(notes.contains("## What's new"));
        assert!(notes.contains("## Recent commits"));
        assert!(!notes.contains("## Development build snapshot"));
        assert!(render_markdown(true).contains("## Development build snapshot"));
    }

    #[test]
    fn release_profile_hot_reload_and_ui_override_include_snapshot() {
        for debug in [false, true] {
            for hot_reload in [false, true] {
                for ui_override in [false, true] {
                    assert_eq!(
                        include_development_snapshot(debug, hot_reload, ui_override),
                        debug || hot_reload || ui_override,
                    );
                }
            }
        }
        assert!(!include_development_snapshot(false, false, false));
        assert!(include_development_snapshot(false, true, false));
        assert!(include_development_snapshot(false, false, true));
    }
}
