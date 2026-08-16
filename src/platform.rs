//! Small platform integration helpers shared by the desktop surfaces.

use std::path::PathBuf;

/// Locate a companion executable shipped inside the app bundle, then fall back
/// to PATH for source builds. macOS GUI apps inherit a deliberately small PATH,
/// so depending on shell startup files would make Finder launches unreliable.
pub fn companion_executable(name: &str) -> PathBuf {
    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let sibling = directory.join(name);
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_companion_falls_back_to_the_program_name() {
        let name = "jcode-companion-that-does-not-exist";
        assert_eq!(companion_executable(name), PathBuf::from(name));
    }
}
