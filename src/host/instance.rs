#[cfg(unix)]
mod platform {
    use std::{
        fs,
        io::{self, Read, Write},
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
        sync::{
            Mutex,
            mpsc::{self, Receiver},
        },
        thread,
        time::Duration,
    };

    const SHOW: u8 = b'S';
    const RELOAD: u8 = b'R';
    const TOGGLE_VOICE: u8 = b'V';
    const OK: &[u8] = b"ok\n";

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Command {
        Show,
        Reload,
        ToggleVoice,
    }

    pub enum Instance {
        Primary {
            commands: Receiver<Command>,
            _socket: SocketGuard,
        },
        Secondary,
    }

    pub struct SocketGuard(Mutex<Option<PathBuf>>);

    impl SocketGuard {
        /// GPUI may terminate without dropping detached command-loop futures.
        /// Explicit shutdown consumes ownership so a later Drop cannot unlink
        /// a replacement host's socket at the same (non-single-panel) pathname.
        pub fn cleanup(&self) {
            if let Some(path) = self.0.lock().unwrap().take() {
                let _ = fs::remove_file(path);
            }
        }
    }

    impl Drop for SocketGuard {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    pub fn acquire_named(name: Option<&str>, command: Command) -> io::Result<Instance> {
        acquire_at(socket_path(name), command)
    }

    /// Explicit commands must never start a new host or replace its socket.
    /// In particular, an older live host may not understand a newer command.
    pub fn notify_named(name: Option<&str>, command: Command) -> io::Result<()> {
        notify(&socket_path(name), command)
    }

    fn socket_path(name: Option<&str>) -> PathBuf {
        let socket = match name {
            Some(name) => format!("jcode-desktop-{name}.sock"),
            None => "jcode-desktop.sock".to_owned(),
        };
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(runtime).join(socket);
        }
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
        std::env::temp_dir().join(format!("{user}-{socket}"))
    }

    fn acquire_at(path: PathBuf, command: Command) -> io::Result<Instance> {
        if notify(&path, command).is_ok() {
            return Ok(Instance::Secondary);
        }

        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                // The socket may belong to a process that is still starting. Give it
                // a short opportunity to begin accepting before treating it as stale.
                for _ in 0..10 {
                    thread::sleep(Duration::from_millis(5));
                    if notify(&path, command).is_ok() {
                        return Ok(Instance::Secondary);
                    }
                }
                fs::remove_file(&path)?;
                UnixListener::bind(&path)?
            }
            Err(error) => return Err(error),
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("jcode-desktop-instance".into())
            .spawn(move || serve(listener, commands))?;
        Ok(Instance::Primary {
            commands: receiver,
            _socket: SocketGuard(Mutex::new(Some(path))),
        })
    }

    fn notify(path: &Path, command: Command) -> io::Result<()> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        stream.write_all(&[match command {
            Command::Show => SHOW,
            Command::Reload => RELOAD,
            Command::ToggleVoice => TOGGLE_VOICE,
        }])?;
        let mut response = [0; 3];
        stream.read_exact(&mut response)?;
        if response == OK {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid desktop host response",
            ))
        }
    }

    fn serve(listener: UnixListener, commands: mpsc::Sender<Command>) {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
            let mut command = [0; 1];
            if stream.read_exact(&mut command).is_ok() {
                let command = match command[0] {
                    SHOW => Command::Show,
                    RELOAD => Command::Reload,
                    TOGGLE_VOICE => Command::ToggleVoice,
                    _ => continue,
                };
                if commands.send(command).is_ok() {
                    let _ = stream.write_all(OK);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn path(name: &str) -> (tempfile::TempDir, PathBuf) {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join(name);
            (root, path)
        }

        #[test]
        fn second_instance_notifies_the_primary() {
            let (_root, path) = path("instance.sock");
            let primary = acquire_at(path.clone(), Command::Show).unwrap();
            let commands = match &primary {
                Instance::Primary { commands, .. } => commands,
                Instance::Secondary => panic!(),
            };
            assert!(matches!(
                acquire_at(path, Command::Reload),
                Ok(Instance::Secondary)
            ));
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(1)).unwrap(),
                Command::Reload
            );
        }

        #[test]
        fn single_panel_sockets_are_independent_and_reload_only_their_owner() {
            let root = tempfile::tempdir().unwrap();
            use jcode_desktop_api::LaunchMode;
            let names: Vec<_> = [
                (LaunchMode::Workspace, 1),
                (LaunchMode::NoSidebar, 2),
                (LaunchMode::SinglePanel, 123),
                (LaunchMode::SinglePanel, 456),
            ]
            .into_iter()
            .map(|(mode, pid)| match mode.instance_name(pid) {
                Some(name) => format!("jcode-desktop-{name}.sock"),
                None => "jcode-desktop.sock".into(),
            })
            .collect();
            let hosts: Vec<_> = names
                .iter()
                .map(|name| acquire_at(root.path().join(name), Command::Show).unwrap())
                .collect();
            for host in &hosts {
                let Instance::Primary { commands, .. } = host else {
                    panic!("independent primary expected")
                };
                assert!(
                    commands.try_recv().is_err(),
                    "launch must not wake another host"
                );
            }
            notify(&root.path().join(&names[2]), Command::Reload).unwrap();
            for (index, host) in hosts.iter().enumerate() {
                let Instance::Primary { commands, .. } = host else {
                    panic!()
                };
                if index == 2 {
                    assert_eq!(
                        commands.recv_timeout(Duration::from_secs(1)).unwrap(),
                        Command::Reload
                    );
                } else {
                    assert!(
                        commands.try_recv().is_err(),
                        "reload must reach only its owner"
                    );
                }
            }
            drop(hosts);
            assert!(names.iter().all(|name| !root.path().join(name).exists()));
        }

        #[test]
        fn explicit_cleanup_is_idempotent_and_preserves_a_replacement_host() {
            let (_root, path) = path("quit.sock");
            let primary = acquire_at(path.clone(), Command::Show).unwrap();
            let Instance::Primary { _socket, .. } = &primary else {
                panic!()
            };
            _socket.cleanup();
            assert!(!path.exists());
            let replacement = acquire_at(path.clone(), Command::Show).unwrap();
            assert!(matches!(replacement, Instance::Primary { .. }));
            _socket.cleanup();
            drop(primary);
            assert!(
                path.exists(),
                "old owner must not unlink replacement socket"
            );
            drop(replacement);
            assert!(!path.exists());
        }

        #[test]
        fn stale_socket_is_replaced() {
            let (_root, path) = path("stale.sock");
            fs::write(&path, b"stale").unwrap();
            let primary = acquire_at(path, Command::Show).unwrap();
            assert!(matches!(&primary, Instance::Primary { .. }));
        }

        #[test]
        fn socket_is_private() {
            let (_root, path) = path("private.sock");
            let primary = acquire_at(path.clone(), Command::Show).unwrap();
            assert!(matches!(&primary, Instance::Primary { .. }));
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        #[test]
        fn voice_request_is_explicit_and_does_not_replay_on_startup() {
            let (_root, path) = path("voice.sock");
            assert!(notify(&path, Command::ToggleVoice).is_err());
            assert!(
                !path.exists(),
                "a voice request must not create a host socket"
            );
            let primary = acquire_at(path.clone(), Command::Show).unwrap();
            let Instance::Primary { commands, .. } = &primary else {
                panic!()
            };
            assert!(
                commands.try_recv().is_err(),
                "startup must not toggle voice"
            );
            notify(&path, Command::ToggleVoice).unwrap();
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(1)).unwrap(),
                Command::ToggleVoice
            );
            assert!(commands.try_recv().is_err());
        }

        #[test]
        fn unsupported_voice_request_preserves_live_old_host_socket() {
            let (_root, path) = path("old-host.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                assert_eq!(byte, [TOGGLE_VOICE]);
                // Old hosts simply close an unknown command connection.
            });
            assert!(notify(&path, Command::ToggleVoice).is_err());
            server.join().unwrap();
            assert!(path.exists(), "do not unlink an unsupported host's socket");
        }
    }
} // unix platform

#[cfg(unix)]
pub use platform::{Command, Instance, acquire_named, notify_named};

// GPUI's Windows event loop is supported, but Unix-domain socket ownership and
// permissions are not. Permit independent instances until named pipes land.
#[cfg(not(unix))]
mod platform {
    use std::{
        io,
        sync::mpsc::{self, Receiver},
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Command {
        Show,
        Reload,
        ToggleVoice,
    }

    pub enum Instance {
        Primary {
            commands: Receiver<Command>,
            _socket: SocketGuard,
        },
        Secondary,
    }

    pub struct SocketGuard;

    impl SocketGuard {
        pub fn cleanup(&self) {}
    }

    pub fn notify_named(_name: Option<&str>, _command: Command) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "desktop command forwarding is not supported on this platform",
        ))
    }

    pub fn acquire_named(_name: Option<&str>, _command: Command) -> io::Result<Instance> {
        let (_sender, commands) = mpsc::channel();
        Ok(Instance::Primary {
            commands,
            _socket: SocketGuard,
        })
    }
}

#[cfg(not(unix))]
pub use platform::{Command, Instance, acquire_named, notify_named};
