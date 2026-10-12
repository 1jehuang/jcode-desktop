//! Per-user directories that resolve correctly on every desktop platform.
//!
//! Windows does not set `HOME`, so `$HOME/.local/state` style lookups returned
//! nothing (or a relative path) there. Diagnostics then landed in
//! `%TEMP%\.local\state\...` and other state silently went missing (#5).
//! Every Desktop path that means "the user's home" or "Desktop's durable
//! state" goes through these helpers instead of reading `HOME` directly.

use std::ffi::OsString;
use std::path::PathBuf;

/// The user's home directory: `HOME` when set and non-empty (Unix, and
/// MSYS/Cygwin shells on Windows), otherwise `USERPROFILE` on Windows, and
/// finally the platform's own lookup.
pub fn home_dir() -> Option<PathBuf> {
    home_dir_from(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        cfg!(windows),
    )
    .or_else(std::env::home_dir)
    .filter(|path| !path.as_os_str().is_empty())
}

fn home_dir_from(
    home: Option<OsString>,
    user_profile: Option<OsString>,
    windows: bool,
) -> Option<PathBuf> {
    let non_empty = |value: Option<OsString>| value.filter(|value| !value.is_empty());
    non_empty(home)
        .or_else(|| windows.then(|| non_empty(user_profile)).flatten())
        .map(PathBuf::from)
}

/// The home directory as a UTF-8 string, for shortening displayed paths.
pub fn home_string() -> Option<String> {
    home_dir()?.into_os_string().into_string().ok()
}

/// Jcode's own home (`$JCODE_HOME`, else `~/.jcode`), shared with the CLI.
pub fn jcode_home() -> Option<PathBuf> {
    std::env::var_os("JCODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".jcode")))
}

/// Desktop's durable state directory.
///
/// - `$XDG_STATE_HOME/jcode-desktop` when set (also used by isolated tests),
/// - `%LOCALAPPDATA%\Jcode Desktop` on Windows,
/// - `~/Library/Application Support/Jcode` on macOS,
/// - `~/.local/state/jcode-desktop` elsewhere.
pub fn state_dir() -> Option<PathBuf> {
    state_dir_from(
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("LOCALAPPDATA"),
        home_dir(),
        Platform::current(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Windows,
    MacOs,
    Unix,
}

impl Platform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Unix
        }
    }
}

fn state_dir_from(
    xdg_state: Option<OsString>,
    local_app_data: Option<OsString>,
    home: Option<PathBuf>,
    platform: Platform,
) -> Option<PathBuf> {
    if let Some(base) = xdg_state.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(base).join("jcode-desktop"));
    }
    match platform {
        Platform::Windows => local_app_data
            .filter(|value| !value.is_empty())
            .map(|base| PathBuf::from(base).join("Jcode Desktop"))
            .or_else(|| home.map(|home| home.join("AppData/Local/Jcode Desktop"))),
        Platform::MacOs => home.map(|home| home.join("Library/Application Support/Jcode")),
        Platform::Unix => home.map(|home| home.join(".local/state/jcode-desktop")),
    }
}

/// Replace a leading home directory with `~`, for compact path labels.
/// Accepts either separator after the home prefix, so Windows paths shorten.
pub fn tilde(path: &str) -> String {
    match home_string() {
        Some(home) => tilde_with(path, &home),
        None => path.to_owned(),
    }
}

fn tilde_with(path: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return path.to_owned();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> Option<OsString> {
        Some(value.into())
    }

    #[test]
    fn windows_home_falls_back_to_user_profile() {
        assert_eq!(
            home_dir_from(None, os(r"C:\Users\ada"), true),
            Some(PathBuf::from(r"C:\Users\ada"))
        );
        assert_eq!(
            home_dir_from(os(""), os(r"C:\Users\ada"), true),
            Some(PathBuf::from(r"C:\Users\ada"))
        );
        // MSYS shells export HOME, which wins.
        assert_eq!(
            home_dir_from(os("/c/Users/ada"), os(r"C:\Users\ada"), true),
            Some(PathBuf::from("/c/Users/ada"))
        );
        // USERPROFILE is meaningless off Windows.
        assert_eq!(home_dir_from(None, os("/x"), false), None);
    }

    #[test]
    fn state_dir_per_platform() {
        let home = Some(PathBuf::from("/h"));
        assert_eq!(
            state_dir_from(os("/xdg"), None, home.clone(), Platform::Windows),
            Some(PathBuf::from("/xdg/jcode-desktop"))
        );
        assert_eq!(
            state_dir_from(None, os(r"C:\L"), home.clone(), Platform::Windows),
            Some(PathBuf::from(r"C:\L").join("Jcode Desktop"))
        );
        assert_eq!(
            state_dir_from(None, None, home.clone(), Platform::Windows),
            Some(PathBuf::from("/h/AppData/Local/Jcode Desktop"))
        );
        assert_eq!(
            state_dir_from(None, os("/ignored"), home.clone(), Platform::Unix),
            Some(PathBuf::from("/h/.local/state/jcode-desktop"))
        );
        assert_eq!(
            state_dir_from(None, None, home, Platform::MacOs),
            Some(PathBuf::from("/h/Library/Application Support/Jcode"))
        );
        assert_eq!(state_dir_from(None, None, None, Platform::Unix), None);
    }

    #[test]
    fn tilde_handles_both_separators() {
        assert_eq!(tilde_with("/home/a/src", "/home/a"), "~/src");
        assert_eq!(tilde_with("/home/a", "/home/a/"), "~");
        assert_eq!(tilde_with("/home/ab", "/home/a"), "/home/ab");
        assert_eq!(tilde_with(r"C:\Users\a\src", r"C:\Users\a"), r"~\src");
        assert_eq!(tilde_with("/x", ""), "/x");
    }

    #[test]
    fn live_home_resolves() {
        assert!(home_dir().is_some());
        assert!(state_dir().is_some());
    }
}
