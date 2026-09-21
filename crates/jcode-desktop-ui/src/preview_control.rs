//! Private, UI-owned self-development control endpoint. Never enabled in normal runs.
use serde::Deserialize;
use std::{sync::mpsc, time::Instant};

pub(crate) fn enabled() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    enabled_for(
        std::env::var("JCODE_DESKTOP_SELF_DEV").as_deref() == Ok("1"),
        std::env::var_os("JCODE_DESKTOP_SCREENSHOT").is_some(),
        args.iter().any(|arg| arg == "--no-hot-reload"),
        std::env::var_os("JCODE_DESKTOP_UI").is_some()
            || args.iter().any(|arg| arg == "--hot-reload"),
        cfg!(debug_assertions)
            && std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../Cargo.toml")
                .is_file(),
    )
}

fn enabled_for(
    explicit: bool,
    screenshot: bool,
    opt_out: bool,
    reload: bool,
    development: bool,
) -> bool {
    explicit || (!screenshot && !opt_out && (reload || development))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum Request {
    List {},
    Onboarding {},
    Open { state: String },
    Reset { state: String },
}

pub(crate) struct Pending {
    pub request: Request,
    pub reply: mpsc::SyncSender<serde_json::Value>,
    pub deadline: Instant,
}

#[cfg(not(unix))]
pub(crate) struct Server;
#[cfg(not(unix))]
impl Server {
    pub fn start() -> std::io::Result<(Self, async_channel::Receiver<Pending>)> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "preview control sockets require Unix",
        ))
    }
}
#[cfg(unix)]
pub(crate) use unix::Server;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        io::{self, Read, Write},
        os::unix::{
            fs::{DirBuilderExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };
    const TIMEOUT: Duration = Duration::from_secs(2);
    const MAX_REQUEST: usize = 4096;

    /// Join before unloading the UI dylib: no thread may execute retired UI code.
    /// Unlink only the inode we created, never an endpoint from another generation.
    pub(crate) struct Server {
        path: PathBuf,
        identity: (u64, u64),
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Server {
        pub fn start() -> io::Result<(Self, async_channel::Receiver<Pending>)> {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(|runtime| PathBuf::from(runtime).join("jcode-desktop-preview"))
                .unwrap_or_else(|| {
                    std::env::temp_dir().join(format!("jcode-desktop-preview-{}", unsafe {
                        libc::geteuid()
                    }))
                });
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&dir) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
            let meta = std::fs::symlink_metadata(&dir)?;
            if !meta.is_dir()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o077 != 0
            {
                return Err(io::Error::other(
                    "preview directory must be owned by this user with mode 0700",
                ));
            }
            Self::bind(dir.join(format!("{}.sock", std::process::id())))
        }

        #[cfg(target_os = "linux")]
        fn retire_same_process_endpoint(path: &std::path::Path) -> io::Result<()> {
            use std::os::{fd::AsRawFd, unix::fs::FileTypeExt};
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                return Ok(());
            };
            if !meta.file_type().is_socket()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o077 != 0
            {
                return Err(io::Error::other(
                    "refusing to replace unsafe preview endpoint",
                ));
            }
            let stream = UnixStream::connect(path)?;
            let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
            let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            let result = unsafe {
                libc::getsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    (&mut peer as *mut libc::ucred).cast(),
                    &mut length,
                )
            };
            if result != 0
                || peer.pid != std::process::id() as libc::pid_t
                || peer.uid != unsafe { libc::geteuid() }
            {
                return Err(io::Error::other(
                    "preview endpoint belongs to another process",
                ));
            }
            let current = std::fs::symlink_metadata(path)?;
            if (current.dev(), current.ino()) != (meta.dev(), meta.ino()) {
                return Err(io::Error::other("preview endpoint changed during handover"));
            }
            std::fs::remove_file(path)
        }

        fn bind(path: PathBuf) -> io::Result<(Self, async_channel::Receiver<Pending>)> {
            // Retained GPUI roots can keep an old generation's server alive.
            // Only take over a private socket proven to belong to this process.
            #[cfg(target_os = "linux")]
            Self::retire_same_process_endpoint(&path)?;
            let listener = UnixListener::bind(&path)?;
            let meta = std::fs::symlink_metadata(&path)?;
            let stop = Arc::new(AtomicBool::new(false));
            let mut server = Self {
                path,
                identity: (meta.dev(), meta.ino()),
                stop: stop.clone(),
                thread: None,
            };
            std::fs::set_permissions(&server.path, std::fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            let (tx, rx) = async_channel::bounded(8);
            server.thread = Some(thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let result = handle(&mut stream, &tx, &stop);
                            if let Err(error) = result {
                                let _ = writeln!(
                                    stream,
                                    "{}",
                                    serde_json::json!({"ok":false,"error":error.to_string()})
                                );
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            }));
            Ok((server, rx))
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            if std::fs::symlink_metadata(&self.path)
                .is_ok_and(|m| (m.dev(), m.ino()) == self.identity)
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    fn handle(
        stream: &mut UnixStream,
        tx: &async_channel::Sender<Pending>,
        stop: &AtomicBool,
    ) -> io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        let deadline = Instant::now() + TIMEOUT;
        let mut bytes = Vec::new();
        loop {
            if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                return Err(io::Error::other("request timed out or UI reloading"));
            }
            let mut byte = [0];
            match stream.read(&mut byte) {
                Ok(0) => return Err(io::Error::other("request must end with newline")),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => {
                    bytes.push(byte[0]);
                    if bytes.len() > MAX_REQUEST {
                        return Err(io::Error::other("request too large"));
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let request = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let (reply, result) = mpsc::sync_channel(1);
        tx.try_send(Pending {
            request,
            reply,
            deadline,
        })
        .map_err(io::Error::other)?;
        loop {
            if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                return Err(io::Error::other("UI response timed out or reloading"));
            }
            match result.recv_timeout(Duration::from_millis(10)) {
                Ok(value) => return writeln!(stream, "{value}"),
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(e) => return Err(io::Error::other(e)),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[cfg(target_os = "linux")]
        #[test]
        fn new_generation_takes_over_retained_root_endpoint() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("preview.sock");
            let (old, old_rx) = Server::bind(path.clone()).unwrap();
            let (new, new_rx) = Server::bind(path.clone()).unwrap();
            drop(old);
            assert!(
                path.exists(),
                "retired server must not unlink the new endpoint"
            );
            let mut client = UnixStream::connect(&path).unwrap();
            client.set_read_timeout(Some(TIMEOUT)).unwrap();
            writeln!(client, "{{\"command\":\"onboarding\"}}").unwrap();
            let pending = new_rx.recv_blocking().unwrap();
            assert!(matches!(pending.request, Request::Onboarding {}));
            assert!(old_rx.try_recv().is_err());
            pending
                .reply
                .send(serde_json::json!({"ok":true,"step":"welcome"}))
                .unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert!(response.contains("welcome"));
            drop(new);
            assert!(!path.exists());
        }

        #[test]
        fn protocol_is_strict() {
            assert!(matches!(
                serde_json::from_str::<Request>(r#"{"command":"open","state":"idle"}"#).unwrap(),
                Request::Open { .. }
            ));
            for invalid in [
                r#"{"command":"send"}"#,
                r#"{"command":"open"}"#,
                r#"{"command":"list","extra":1}"#,
            ] {
                assert!(serde_json::from_str::<Request>(invalid).is_err());
            }
        }
        #[test]
        fn malformed_and_oversized_requests_do_not_reach_ui() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("preview.sock");
            let (_server, rx) = Server::bind(path.clone()).unwrap();
            for input in ["not-json\n".to_string(), "x".repeat(MAX_REQUEST + 1) + "\n"] {
                let mut stream = UnixStream::connect(&path).unwrap();
                stream.set_read_timeout(Some(TIMEOUT)).unwrap();
                stream.write_all(input.as_bytes()).unwrap();
                let mut output = String::new();
                // Oversized input may leave unread bytes, which can yield a reset after the error reply.
                let _ = stream.read_to_string(&mut output);
                assert!(output.contains("\"ok\":false"), "{output}");
                assert!(rx.try_recv().is_err());
            }
        }

        #[test]
        fn dropping_server_interrupts_partial_request() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("preview.sock");
            let (server, _rx) = Server::bind(path.clone()).unwrap();
            let mut stream = UnixStream::connect(&path).unwrap();
            stream.write_all(b"{").unwrap();
            thread::sleep(Duration::from_millis(30));
            let start = Instant::now();
            drop(server);
            assert!(start.elapsed() < Duration::from_secs(1));
            assert!(!path.exists());
        }

        #[test]
        fn socket_ack_waits_for_ui_and_drop_allows_rebind() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("preview.sock");
            let (server, rx) = Server::bind(path.clone()).unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
            let mut client = UnixStream::connect(&path).unwrap();
            client
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            writeln!(client, "{{\"command\":\"list\"}}").unwrap();
            let pending = rx.recv_blocking().unwrap();
            assert!(client.read(&mut [0]).is_err());
            pending.reply.send(serde_json::json!({"ok":true})).unwrap();
            client.set_read_timeout(Some(TIMEOUT)).unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert!(response.contains("true"));
            drop(server);
            assert!(!path.exists());
            let (_replacement, _) = Server::bind(path).unwrap();
        }
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    #[test]
    fn production_off_and_explicit_overrides() {
        assert!(!enabled_for(false, false, false, false, false));
        assert!(enabled_for(false, false, false, true, false));
        assert!(enabled_for(false, false, false, false, true));
        assert!(!enabled_for(false, false, true, true, true));
        assert!(!enabled_for(false, true, false, true, true));
        assert!(enabled_for(true, true, true, false, false));
    }
}
