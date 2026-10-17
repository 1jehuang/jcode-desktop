#[cfg(unix)]
mod platform {
    use jcode_desktop_api::WindowLaunch;
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
    const VOICE_PRESS: u8 = b'P';
    const VOICE_RELEASE: u8 = b'U';
    const OPEN_WINDOW: u8 = b'W';
    const OK: &[u8] = b"ok\n";
    /// Arguments plus a few forwarded variables, never a transcript.
    const MAX_LAUNCH_BYTES: usize = 64 * 1024;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Command {
        Show,
        Reload,
        ToggleVoice,
        VoicePress,
        VoiceRelease,
        /// Open another window in the shared single-panel host.
        OpenWindow(WindowLaunch),
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
        if notify(&path, command.clone()).is_ok() {
            return Ok(Instance::Secondary);
        }

        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                // The socket may belong to a process that is still starting. Give it
                // a short opportunity to begin accepting before treating it as stale.
                for _ in 0..10 {
                    thread::sleep(Duration::from_millis(5));
                    if notify(&path, command.clone()).is_ok() {
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
        let mut message = vec![match &command {
            Command::Show => SHOW,
            Command::Reload => RELOAD,
            Command::ToggleVoice => TOGGLE_VOICE,
            Command::VoicePress => VOICE_PRESS,
            Command::VoiceRelease => VOICE_RELEASE,
            Command::OpenWindow(_) => OPEN_WINDOW,
        }];
        if let Command::OpenWindow(launch) = &command {
            let payload = launch.encode();
            if payload.len() > MAX_LAUNCH_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "desktop launch request is too large",
                ));
            }
            message.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            message.extend_from_slice(&payload);
        }
        stream.write_all(&message)?;
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
                    VOICE_PRESS => Command::VoicePress,
                    VOICE_RELEASE => Command::VoiceRelease,
                    OPEN_WINDOW => match read_launch(&mut stream) {
                        Some(launch) => Command::OpenWindow(launch),
                        None => continue,
                    },
                    _ => continue,
                };
                if commands.send(command).is_ok() {
                    let _ = stream.write_all(OK);
                }
            }
        }
    }

    fn read_launch(stream: &mut UnixStream) -> Option<WindowLaunch> {
        let mut len = [0; 4];
        stream.read_exact(&mut len).ok()?;
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_LAUNCH_BYTES {
            return None;
        }
        let mut payload = vec![0; len];
        stream.read_exact(&mut payload).ok()?;
        WindowLaunch::decode(&payload)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn open_window_carries_its_own_launch_to_the_shared_host() {
            let (_root, path) = path("shared.sock");
            let primary = acquire_at(path.clone(), Command::Show).unwrap();
            let Instance::Primary { commands, .. } = &primary else {
                panic!()
            };
            let launch = WindowLaunch::from_parts(
                ["--single-panel", "--session=abc"],
                [("JCODE_DESKTOP_WORKING_DIR".into(), "/srv/project".into())],
            );
            assert!(matches!(
                acquire_at(path.clone(), Command::OpenWindow(launch.clone())),
                Ok(Instance::Secondary)
            ));
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(1)).unwrap(),
                Command::OpenWindow(launch)
            );
            // A malformed or oversized request is dropped without killing the listener.
            let mut stream = UnixStream::connect(&path).unwrap();
            stream.write_all(&[OPEN_WINDOW]).unwrap();
            stream
                .write_all(&((MAX_LAUNCH_BYTES as u32) + 1).to_le_bytes())
                .unwrap();
            drop(stream);
            notify(&path, Command::Show).unwrap();
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(1)).unwrap(),
                Command::Show
            );
        }

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
        fn voice_hold_lifecycle_is_forwarded_in_order_without_startup_replay() {
            let (_root, path) = path("hold.sock");
            for command in [Command::VoicePress, Command::VoiceRelease] {
                assert!(notify(&path, command.clone()).is_err());
                assert!(!path.exists(), "explicit edges must not start a host");
            }
            let primary = acquire_at(path.clone(), Command::Show).unwrap();
            let Instance::Primary { commands, .. } = &primary else {
                panic!()
            };
            assert!(commands.try_recv().is_err());
            // IPC preserves edges, including repeats. The UI owns hold idempotency.
            for command in [
                Command::VoicePress,
                Command::VoicePress,
                Command::VoiceRelease,
                Command::VoiceRelease,
                Command::VoicePress,
                Command::VoiceRelease,
                Command::ToggleVoice,
            ] {
                notify(&path, command.clone()).unwrap();
                assert_eq!(
                    commands.recv_timeout(Duration::from_secs(1)).unwrap(),
                    command
                );
            }
            assert!(commands.try_recv().is_err());
            drop(primary);
            assert!(!path.exists());
            assert!(notify(&path, Command::VoiceRelease).is_err());
            assert!(!path.exists());
        }

        #[test]
        fn unsupported_voice_request_preserves_live_old_host_socket() {
            for (command, expected_byte) in [
                (Command::ToggleVoice, TOGGLE_VOICE),
                (Command::VoicePress, VOICE_PRESS),
                (Command::VoiceRelease, VOICE_RELEASE),
            ] {
                let (_root, path) = path("old-host.sock");
                let listener = UnixListener::bind(&path).unwrap();
                let server = thread::spawn(move || {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    assert_eq!(byte, [expected_byte]);
                    // Old hosts simply close an unknown command connection.
                });
                assert!(notify(&path, command).is_err());
                server.join().unwrap();
                assert!(path.exists(), "do not unlink an unsupported host's socket");
            }
        }
    }
} // unix platform

#[cfg(unix)]
pub use platform::{Command, Instance, acquire_named, notify_named};

/// Windows: the same one-byte protocol over a per-user named pipe.
///
/// Before this, every launch from the Start menu or a pinned taskbar icon
/// started an independent host, each with its own window, sessions, and
/// global voice shortcut. Now a second launch shows the running window, as on
/// Linux and macOS. The first pipe instance is created with
/// `FILE_FLAG_FIRST_PIPE_INSTANCE`, which atomically makes exactly one process
/// the owner. The pipe's default security grants access to the creating user
/// only, and remote clients are rejected.
#[cfg(windows)]
mod platform {
    use jcode_desktop_api::WindowLaunch;
    use std::{
        ffi::OsStr,
        fs::OpenOptions,
        io::{self, Read, Write},
        os::windows::{
            ffi::OsStrExt,
            io::{FromRawHandle, OwnedHandle},
        },
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, Receiver},
        },
        thread,
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
            INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX,
        },
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            WaitNamedPipeW,
        },
    };

    const SHOW: u8 = b'S';
    const RELOAD: u8 = b'R';
    const TOGGLE_VOICE: u8 = b'V';
    const VOICE_PRESS: u8 = b'P';
    const VOICE_RELEASE: u8 = b'U';
    const OPEN_WINDOW: u8 = b'W';
    const OK: &[u8] = b"ok\n";
    const MAX_LAUNCH_BYTES: usize = 64 * 1024;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Command {
        Show,
        Reload,
        ToggleVoice,
        VoicePress,
        VoiceRelease,
        /// Open another window in the shared single-panel host.
        OpenWindow(WindowLaunch),
    }

    pub enum Instance {
        Primary {
            commands: Receiver<Command>,
            _socket: SocketGuard,
        },
        Secondary,
    }

    /// Owns the listening pipe. Dropping it stops accepting new clients, so a
    /// replacement host can take over the name.
    pub struct SocketGuard {
        stopped: std::sync::Arc<AtomicBool>,
        name: Mutex<Option<String>>,
    }

    impl SocketGuard {
        pub fn cleanup(&self) {
            self.stopped.store(true, Ordering::SeqCst);
            if let Some(name) = self.name.lock().unwrap().take() {
                // Wake the accept loop so it observes `stopped` and exits.
                let _ = OpenOptions::new().read(true).write(true).open(&name);
            }
        }
    }

    impl Drop for SocketGuard {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    /// Per-user, per-session pipe name. `USERNAME` keeps users on a shared
    /// machine apart. The pipe's default DACL enforces the same boundary.
    fn pipe_name(name: Option<&str>) -> String {
        let user: String = std::env::var("USERNAME")
            .unwrap_or_else(|_| "user".into())
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            .take(64)
            .collect();
        let suffix = match name {
            Some(name) => format!("-{name}"),
            None => String::new(),
        };
        format!(r"\\.\pipe\jcode-desktop-{user}{suffix}")
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }

    pub fn acquire_named(name: Option<&str>, command: Command) -> io::Result<Instance> {
        acquire_at(pipe_name(name), command)
    }

    pub fn notify_named(name: Option<&str>, command: Command) -> io::Result<()> {
        notify(&pipe_name(name), command)
    }

    fn create_instance(name: &str, first: bool) -> io::Result<OwnedHandle> {
        let wide_name = wide(name);
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `wide_name` is NUL terminated and outlives the call.
        let handle: HANDLE = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh, valid handle owned by nobody else.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
    }

    fn acquire_at(name: String, command: Command) -> io::Result<Instance> {
        if notify(&name, command.clone()).is_ok() {
            return Ok(Instance::Secondary);
        }
        let first = match create_instance(&name, true) {
            Ok(handle) => handle,
            // Another host owns the name but is still starting. Give it a
            // moment to accept before starting a second, independent host.
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                for _ in 0..20 {
                    thread::sleep(Duration::from_millis(25));
                    if notify(&name, command.clone()).is_ok() {
                        return Ok(Instance::Secondary);
                    }
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let (commands, receiver) = mpsc::channel();
        let stopped = std::sync::Arc::new(AtomicBool::new(false));
        let serve_name = name.clone();
        let serve_stopped = stopped.clone();
        thread::Builder::new()
            .name("jcode-desktop-instance".into())
            .spawn(move || serve(serve_name, first, commands, serve_stopped))?;
        Ok(Instance::Primary {
            commands: receiver,
            _socket: SocketGuard {
                stopped,
                name: Mutex::new(Some(name)),
            },
        })
    }

    fn serve(
        name: String,
        mut next: OwnedHandle,
        commands: mpsc::Sender<Command>,
        stopped: std::sync::Arc<AtomicBool>,
    ) {
        loop {
            let raw = {
                use std::os::windows::io::AsRawHandle;
                next.as_raw_handle() as HANDLE
            };
            // SAFETY: `raw` is a valid pipe server handle owned by `next`.
            let connected = unsafe { ConnectNamedPipe(raw, std::ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if stopped.load(Ordering::SeqCst) {
                return;
            }
            // Create the next instance before serving, so a client arriving
            // meanwhile finds a listener instead of ERROR_FILE_NOT_FOUND.
            let following = match create_instance(&name, false) {
                Ok(handle) => handle,
                Err(error) => {
                    eprintln!("desktop instance pipe stopped: {error}");
                    return;
                }
            };
            let current = std::mem::replace(&mut next, following);
            if connected {
                let mut stream = std::fs::File::from(current);
                if let Some(command) = read_command(&mut stream)
                    && commands.send(command).is_ok()
                {
                    if stream.write_all(OK).is_ok() {
                        // File::flush does not drain a Windows named pipe.
                        // DisconnectNamedPipe discards unread bytes, so wait
                        // until the launcher reads OK before disconnecting.
                        // Otherwise Show succeeds but the launcher sees EOF,
                        // retries the command, then fails to become a primary.
                        // SAFETY: the live File owns this pipe server handle.
                        unsafe { FlushFileBuffers(raw) };
                    }
                }
                use std::os::windows::io::AsRawHandle;
                // SAFETY: valid handle; disconnect lets the client see EOF.
                unsafe { DisconnectNamedPipe(stream.as_raw_handle() as HANDLE) };
            }
        }
    }

    fn read_command(stream: &mut std::fs::File) -> Option<Command> {
        let mut command = [0; 1];
        stream.read_exact(&mut command).ok()?;
        Some(match command[0] {
            SHOW => Command::Show,
            RELOAD => Command::Reload,
            TOGGLE_VOICE => Command::ToggleVoice,
            VOICE_PRESS => Command::VoicePress,
            VOICE_RELEASE => Command::VoiceRelease,
            OPEN_WINDOW => {
                let mut len = [0; 4];
                stream.read_exact(&mut len).ok()?;
                let len = u32::from_le_bytes(len) as usize;
                if len > MAX_LAUNCH_BYTES {
                    return None;
                }
                let mut payload = vec![0; len];
                stream.read_exact(&mut payload).ok()?;
                Command::OpenWindow(WindowLaunch::decode(&payload)?)
            }
            _ => return None,
        })
    }

    fn notify(name: &str, command: Command) -> io::Result<()> {
        let mut stream = match OpenOptions::new().read(true).write(true).open(name) {
            Ok(stream) => stream,
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                let wide_name = wide(name);
                // SAFETY: NUL-terminated name that outlives the call.
                if unsafe { WaitNamedPipeW(wide_name.as_ptr(), 500) } == 0 {
                    return Err(error);
                }
                OpenOptions::new().read(true).write(true).open(name)?
            }
            Err(error) => return Err(error),
        };
        let mut message = vec![match &command {
            Command::Show => SHOW,
            Command::Reload => RELOAD,
            Command::ToggleVoice => TOGGLE_VOICE,
            Command::VoicePress => VOICE_PRESS,
            Command::VoiceRelease => VOICE_RELEASE,
            Command::OpenWindow(_) => OPEN_WINDOW,
        }];
        if let Command::OpenWindow(launch) = &command {
            let payload = launch.encode();
            if payload.len() > MAX_LAUNCH_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "desktop launch request is too large",
                ));
            }
            message.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            message.extend_from_slice(&payload);
        }
        // The running host must be allowed to raise its window: Windows only
        // lets the foreground process (this launcher) hand focus over.
        // SAFETY: plain Win32 call without pointers.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
            );
        }
        stream.write_all(&message)?;
        // A host that is stopping, or an older host that does not know this
        // command, closes the pipe without answering. Bound the wait so a
        // wedged host cannot hang a launch: read on a helper thread.
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut response = [0; 3];
            let _ = sender.send(stream.read_exact(&mut response).map(|()| response));
        });
        match receiver.recv_timeout(Duration::from_millis(1500)) {
            Ok(Ok(response)) if response == OK => Ok(()),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid desktop host response",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "desktop host did not answer",
            )),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicU64;

        fn primary() -> (String, Instance) {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let name = format!(
                r"\\.\pipe\jcode-desktop-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let instance = acquire_at(name.clone(), Command::Show).unwrap();
            assert!(matches!(instance, Instance::Primary { .. }));
            (name, instance)
        }

        #[test]
        fn acknowledgement_survives_a_delayed_client_read() {
            let (name, primary) = primary();
            let Instance::Primary { commands, .. } = &primary else {
                unreachable!()
            };
            let mut client = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
                .unwrap();
            client.write_all(&[SHOW]).unwrap();
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(2)).unwrap(),
                Command::Show
            );
            // Force the server to finish its write before reading the reply.
            // DisconnectNamedPipe discards unread bytes, even after File::flush.
            thread::sleep(Duration::from_millis(100));
            let mut response = [0; 3];
            client.read_exact(&mut response).unwrap();
            assert_eq!(response, OK);
            drop(client);
            notify(&name, Command::Reload).unwrap();
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(2)).unwrap(),
                Command::Reload
            );
        }

        #[test]
        fn disconnected_launcher_does_not_block_the_next_handoff() {
            let (name, primary) = primary();
            let Instance::Primary { commands, .. } = &primary else {
                unreachable!()
            };
            let mut client = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
                .unwrap();
            client.write_all(&[SHOW]).unwrap();
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(2)).unwrap(),
                Command::Show
            );
            drop(client);
            notify(&name, Command::Reload).unwrap();
            assert_eq!(
                commands.recv_timeout(Duration::from_secs(2)).unwrap(),
                Command::Reload
            );
        }

        #[test]
        fn second_launch_delivers_each_command_once_and_remains_secondary() {
            let (name, primary) = primary();
            let Instance::Primary { commands, .. } = &primary else {
                unreachable!()
            };
            let launch = WindowLaunch::from_parts(
                ["--single-panel", "--session=windows-test"],
                [(
                    "JCODE_DESKTOP_WORKING_DIR".into(),
                    r"C:\project with spaces".into(),
                )],
            );
            for command in [
                Command::Show,
                Command::Reload,
                Command::ToggleVoice,
                Command::VoicePress,
                Command::VoiceRelease,
                Command::OpenWindow(launch),
            ]
            .into_iter()
            .cycle()
            .take(30)
            {
                assert!(matches!(
                    acquire_at(name.clone(), command.clone()).unwrap(),
                    Instance::Secondary
                ));
                assert_eq!(
                    commands.recv_timeout(Duration::from_secs(2)).unwrap(),
                    command
                );
                assert!(commands.try_recv().is_err(), "handoff must not be replayed");
            }
        }
    }
}

#[cfg(windows)]
pub use platform::{Command, Instance, acquire_named, notify_named};

// Other non-Unix targets permit independent instances.
#[cfg(not(any(unix, windows)))]
mod platform {
    use std::{
        io,
        sync::mpsc::{self, Receiver},
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Command {
        Show,
        Reload,
        ToggleVoice,
        VoicePress,
        VoiceRelease,
        #[allow(dead_code)]
        OpenWindow(jcode_desktop_api::WindowLaunch),
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

#[cfg(not(any(unix, windows)))]
pub use platform::{Command, Instance, acquire_named, notify_named};
