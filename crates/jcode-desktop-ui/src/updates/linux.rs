//! Linux dispatch. Source checkouts deliberately rebuild local work, not git pull.
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};

use super::{UpdateState, set};

pub(super) fn start() {
    if let Err(error) = std::thread::Builder::new()
        .name("desktop-update".into())
        .spawn(|| {
            // A failed worker must not leave an indefinitely busy indicator.
            let result = std::panic::catch_unwind(update)
                .unwrap_or_else(|_| Err(anyhow::anyhow!("desktop updater unexpectedly stopped")));
            set(match result {
                Ok(message) => UpdateState::Finished { message },
                Err(error) => UpdateState::Failed {
                    message: format!(
                        "Jcode Desktop update failed: {error:#}. Run /update to retry."
                    ),
                },
            });
        })
    {
        set(UpdateState::Failed {
            message: format!("Could not start desktop updater: {error}"),
        });
    }
}

fn update() -> Result<String> {
    let executable = std::env::current_exe().context("locate running desktop")?;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let checkout = manifest.parent().and_then(Path::parent);
    if checkout.is_some_and(|root| source_executable(&executable, root)) {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let enabled = reload_enabled(
            &args,
            std::env::var_os("JCODE_DESKTOP_UI").is_some(),
            cfg!(debug_assertions) && std::env::var_os("JCODE_DESKTOP_SCREENSHOT").is_none(),
        );
        if !enabled {
            bail!(
                "this source build has hot reload disabled. Relaunch with --hot-reload, then use /update or Ctrl+R to rebuild the current checkout (no git changes are made)"
            );
        }
        request_reload(&socket_path(&args))?;
        return Ok("Requested Jcode Desktop's Ctrl+R rebuild-and-reload of the current checkout. This does not fetch Git changes or update the CLI. The current window stays open. Build and activation results are recorded in the desktop log.".into());
    }
    super::linux_package::update()
}

fn source_executable(executable: &Path, checkout: &Path) -> bool {
    // A packaged binary may embed a checkout path that happens to still exist.
    // Only an executable actually in this checkout's Cargo output is a source
    // launch. Never classify ~/.local/opt or /usr/bin by compile-time metadata.
    if !checkout.join("Cargo.toml").is_file() {
        return false;
    }
    let Ok(relative) = executable.strip_prefix(checkout.join("target")) else {
        return false;
    };
    let components: Vec<_> = relative.iter().collect();
    // Cargo's native output is target/<profile>/<binary>, or
    // target/<triple>/<profile>/<binary>. A managed install or packaging stage
    // nested elsewhere under target is not a source executable.
    match components.as_slice() {
        [profile, binary] | [_, profile, binary] => {
            (*profile == "debug" || *profile == "release") && *binary == "jcode-desktop"
        }
        _ => false,
    }
}

fn reload_enabled(args: &[std::ffi::OsString], environment: bool, development: bool) -> bool {
    !args.iter().any(|arg| arg == "--no-hot-reload")
        && (environment || development || args.iter().any(|arg| arg == "--hot-reload"))
}

fn socket_path(args: &[std::ffi::OsString]) -> PathBuf {
    let name = if args
        .iter()
        .any(|arg| arg == "--no-sidebar" || arg == "--workspace")
    {
        "jcode-desktop-no-sidebar.sock"
    } else {
        "jcode-desktop.sock"
    };
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(runtime) => PathBuf::from(runtime).join(name),
        None => std::env::temp_dir().join(format!(
            "{}-{name}",
            std::env::var("USER").unwrap_or_else(|_| "user".into())
        )),
    }
}

fn request_reload(path: &Path) -> Result<()> {
    let mut stream = UnixStream::connect(path).context("connect to the desktop's reload socket")?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"R")?;
    let mut response = [0; 3];
    stream.read_exact(&mut response)?;
    if &response != b"ok\n" {
        bail!("desktop host did not acknowledge the rebuild request");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn installed_executable_is_not_source_even_when_checkout_exists() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[workspace]").unwrap();
        assert!(source_executable(
            &root.path().join("target/debug/jcode-desktop"),
            root.path()
        ));
        assert!(!source_executable(
            Path::new("/usr/bin/jcode-desktop"),
            root.path()
        ));
        assert!(!source_executable(
            &root.path().join("target/debug/deps/ui-test"),
            root.path()
        ));
    }

    #[test]
    fn managed_test_install_under_target_is_not_a_source_launch() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[workspace]").unwrap();
        assert!(!source_executable(
            &root
                .path()
                .join("target/accept/home/.local/opt/jcode-desktop/0.1.0-beta.24/jcode-desktop"),
            root.path(),
        ));
        assert!(source_executable(
            &root
                .path()
                .join("target/x86_64-unknown-linux-gnu/release/jcode-desktop"),
            root.path(),
        ));
    }

    #[test]
    fn source_reload_respects_opt_out_and_release_defaults() {
        assert!(reload_enabled(&[], false, true));
        assert!(!reload_enabled(&[], false, false));
        assert!(reload_enabled(&["--hot-reload".into()], false, false));
        assert!(!reload_enabled(
            &["--hot-reload".into(), "--no-hot-reload".into()],
            true,
            true
        ));
    }

    #[test]
    fn source_update_uses_the_real_host_reload_protocol() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut command = [0];
            stream.read_exact(&mut command).unwrap();
            assert_eq!(&command, b"R");
            stream.write_all(b"ok\n").unwrap();
        });
        request_reload(&path).unwrap();
        host.join().unwrap();
    }

    #[test]
    fn missing_host_reports_error_instead_of_launching_another_window() {
        let root = tempfile::tempdir().unwrap();
        assert!(request_reload(&root.path().join("missing.sock")).is_err());
    }
}
