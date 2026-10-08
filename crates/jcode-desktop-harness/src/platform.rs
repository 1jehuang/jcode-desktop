//! Small platform integration helpers shared by the desktop surfaces.

use std::path::{Path, PathBuf};

/// Locate a companion executable shipped inside the app bundle, then fall back
/// to PATH for source builds. macOS GUI apps inherit a deliberately small PATH,
/// so depending on shell startup files would make Finder launches unreliable.
pub fn companion_executable(name: &str) -> PathBuf {
    #[cfg(windows)]
    let name = if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_owned()
    } else {
        format!("{name}.exe")
    };
    #[cfg(not(windows))]
    let name = name.to_owned();
    if let Ok(current) = std::env::current_exe()
        && let Some(companion) = companion_next_to(&current, &name)
    {
        return companion;
    }
    PathBuf::from(name)
}

/// The desktop executable to relaunch for new windows. A long-running host
/// whose binary was rebuilt in place reports `/path (deleted)` on Linux, which
/// cannot be spawned. The rebuilt file at the original path is the right
/// target, so strip that marker whenever the unmarked path still exists.
pub fn self_executable() -> std::io::Result<PathBuf> {
    let current = std::env::current_exe()?;
    Ok(relaunchable(current))
}

fn relaunchable(current: PathBuf) -> PathBuf {
    if current.exists() {
        return current;
    }
    current
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .unwrap_or(current)
}

fn companion_next_to(current: &Path, name: &str) -> Option<PathBuf> {
    let sibling = current.parent()?.join(name);
    is_executable(&sibling).then_some(sibling)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn relaunches_the_rebuilt_binary_after_an_in_place_rebuild() {
        let root = std::env::temp_dir().join(format!("jcode-relaunch-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join("jcode-desktop");
        fs::write(&binary, []).unwrap();
        let deleted = PathBuf::from(format!("{} (deleted)", binary.display()));
        assert_eq!(relaunchable(deleted), binary);
        assert_eq!(relaunchable(binary.clone()), binary);
        let missing = root.join("gone (deleted)");
        assert_eq!(relaunchable(missing.clone()), missing);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_companion_falls_back_to_the_program_name() {
        let name = "jcode-companion-that-does-not-exist";
        assert_eq!(companion_executable(name), PathBuf::from(name));
    }

    #[cfg(unix)]
    #[test]
    fn finds_an_executable_bundle_companion_without_using_path() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("jcode-bundle-test-{}", std::process::id()));
        let macos = root.join("Jcode.app/Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        let desktop = macos.join("jcode-desktop");
        let cli = macos.join("jcode");
        fs::write(&desktop, []).unwrap();
        fs::write(&cli, []).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(companion_next_to(&desktop, "jcode"), Some(cli));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ignores_a_non_executable_bundle_companion() {
        let root =
            std::env::temp_dir().join(format!("jcode-bundle-mode-test-{}", std::process::id()));
        let macos = root.join("Jcode.app/Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        let cli = macos.join("jcode");
        fs::write(&cli, []).unwrap();

        assert_eq!(
            companion_next_to(&macos.join("jcode-desktop"), "jcode"),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }
}
