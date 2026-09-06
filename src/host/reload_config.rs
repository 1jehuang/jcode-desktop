use std::{ffi::OsString, path::PathBuf};

/// Development hosts should not silently disable Ctrl+R just because the
/// launcher omitted a flag. Packaged builds and offline fixtures stay linked.
pub fn plugin_path() -> Option<PathBuf> {
    resolve(
        std::env::args_os().skip(1).collect(),
        std::env::var_os("JCODE_DESKTOP_UI"),
        cfg!(debug_assertions)
            && std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("crates/jcode-desktop-ui/Cargo.toml")
                .is_file()
            && std::env::var_os("JCODE_DESKTOP_SCREENSHOT").is_none(),
    )
}

fn resolve(
    arguments: Vec<OsString>,
    environment_path: Option<OsString>,
    development_default: bool,
) -> Option<PathBuf> {
    // An explicit opt-out also overrides a plugin inherited from the shell.
    if arguments.iter().any(|arg| arg == "--no-hot-reload") {
        return None;
    }
    if let Some(path) = environment_path {
        return Some(path.into());
    }
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--hot-reload" {
            return Some(
                arguments
                    .next()
                    .filter(|next| !next.to_string_lossy().starts_with('-'))
                    .map(PathBuf::from)
                    .unwrap_or_else(super::reload::default_plugin_path),
            );
        }
    }
    development_default.then(super::reload::default_plugin_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn development_launch_without_flags_enables_reload() {
        assert_eq!(
            resolve(vec![], None, true),
            Some(super::super::reload::default_plugin_path())
        );
    }

    #[test]
    fn packaged_or_fixture_launch_stays_linked() {
        assert_eq!(resolve(vec![], None, false), None);
    }

    #[test]
    fn explicit_flag_enables_reload_even_without_development_default() {
        for arguments in [
            args(&["--hot-reload"]),
            args(&["--hot-reload", "--no-sidebar"]),
        ] {
            assert_eq!(
                resolve(arguments, None, false),
                Some(super::super::reload::default_plugin_path())
            );
        }
    }

    #[test]
    fn custom_plugin_paths_are_preserved() {
        assert_eq!(
            resolve(args(&["--hot-reload", "custom.so"]), None, true),
            Some("custom.so".into())
        );
        assert_eq!(
            resolve(
                args(&["--hot-reload", "custom.so"]),
                Some("env.so".into()),
                true
            ),
            Some("env.so".into())
        );
    }

    #[test]
    fn opt_out_overrides_defaults_flags_and_environment() {
        assert_eq!(
            resolve(
                args(&["--hot-reload", "--no-hot-reload"]),
                Some("env.so".into()),
                true
            ),
            None
        );
    }
}
