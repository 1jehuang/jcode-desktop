use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

const SHOW: &[u8] = b"show\n";
const OK: &[u8] = b"ok\n";

pub enum Instance {
    Primary {
        commands: Receiver<()>,
        _socket: SocketGuard,
    },
    Secondary,
}

pub struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn acquire() -> io::Result<Instance> {
    acquire_at(socket_path())
}

fn socket_path() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("jcode-desktop.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("jcode-desktop-{user}.sock"))
}

fn acquire_at(path: PathBuf) -> io::Result<Instance> {
    if notify(&path).is_ok() {
        return Ok(Instance::Secondary);
    }

    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            // The socket may belong to a process that is still starting. Give it
            // a short opportunity to begin accepting before treating it as stale.
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(5));
                if notify(&path).is_ok() {
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
        _socket: SocketGuard(path),
    })
}

fn notify(path: &Path) -> io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.write_all(SHOW)?;
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

fn serve(listener: UnixListener, commands: mpsc::Sender<()>) {
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else { continue };
        let mut command = [0; 5];
        if stream.read_exact(&mut command).is_ok() && command == SHOW {
            let _ = commands.send(());
            let _ = stream.write_all(OK);
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
        let primary = acquire_at(path.clone()).unwrap();
        let commands = match &primary {
            Instance::Primary { commands, .. } => commands,
            Instance::Secondary => panic!(),
        };
        assert!(matches!(acquire_at(path), Ok(Instance::Secondary)));
        commands.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn stale_socket_is_replaced() {
        let (_root, path) = path("stale.sock");
        fs::write(&path, b"stale").unwrap();
        let primary = acquire_at(path).unwrap();
        assert!(matches!(&primary, Instance::Primary { .. }));
    }

    #[test]
    fn socket_is_private() {
        let (_root, path) = path("private.sock");
        let primary = acquire_at(path.clone()).unwrap();
        assert!(matches!(&primary, Instance::Primary { .. }));
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
