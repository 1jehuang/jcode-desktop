use std::{
    collections::HashMap,
    ffi::c_void,
    io::{Read, Write},
    sync::{Arc, Mutex},
};

use jcode_desktop_api::{ABI_VERSION, HOST_FAILED, HOST_OK, HostApi, TerminalRead};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const ROWS: u16 = 40;
const COLS: u16 = 120;
const MAX_REPLAY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    available_from: u64,
    closed: bool,
}

struct TerminalResource {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    output: Arc<Mutex<Output>>,
    attachments: usize,
}

#[derive(Default)]
struct Terminals {
    next_id: u64,
    resources: HashMap<u64, TerminalResource>,
}

#[derive(Default)]
pub struct HostState {
    snapshot: Mutex<Option<(u32, Vec<u8>)>>,
    terminals: Mutex<Terminals>,
}

impl HostState {
    pub fn api(&self) -> HostApi {
        HostApi {
            abi_version: ABI_VERSION,
            struct_size: size_of::<HostApi>() as u32,
            context: self as *const Self as *mut c_void,
            store_snapshot,
            terminal_create,
            terminal_write,
            terminal_read,
            terminal_release,
        }
    }

    pub fn clear_snapshot(&self) {
        *self.snapshot.lock().expect("snapshot lock poisoned") = None;
    }

    pub fn take_snapshot(&self) -> Option<(u32, Vec<u8>)> {
        self.snapshot.lock().expect("snapshot lock poisoned").take()
    }
}

fn state<'a>(context: *mut c_void) -> Option<&'a HostState> {
    unsafe { context.cast::<HostState>().as_ref() }
}

unsafe extern "C-unwind" fn store_snapshot(
    context: *mut c_void,
    data: *const u8,
    len: usize,
    schema: u32,
) -> i32 {
    let Some(state) = state(context) else {
        return HOST_FAILED;
    };
    if len != 0 && data.is_null() {
        return HOST_FAILED;
    }
    let bytes = if len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    *state.snapshot.lock().expect("snapshot lock poisoned") = Some((schema, bytes));
    HOST_OK
}

unsafe extern "C-unwind" fn terminal_create(
    context: *mut c_void,
    requested_id: u64,
    working_dir: *const u8,
    working_dir_len: usize,
) -> u64 {
    let Some(state) = state(context) else {
        return 0;
    };
    let cwd = if working_dir_len == 0 {
        None
    } else if working_dir.is_null() {
        return 0;
    } else {
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(working_dir, working_dir_len) })
            .ok()
            .map(str::to_owned)
    };

    let mut terminals = state.terminals.lock().expect("terminal lock poisoned");
    if requested_id != 0 {
        if let Some(resource) = terminals.resources.get_mut(&requested_id) {
            resource.attachments += 1;
            return requested_id;
        }
        return 0;
    }

    let Ok(resource) = spawn_terminal(cwd.as_deref()) else {
        return 0;
    };
    terminals.next_id = terminals.next_id.saturating_add(1).max(1);
    let id = terminals.next_id;
    terminals.resources.insert(id, resource);
    id
}

fn spawn_terminal(working_dir: Option<&str>) -> anyhow::Result<TerminalResource> {
    let pair = native_pty_system().openpty(PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let shell = default_shell();
    let is_fish = shell.ends_with("/fish") || shell == "fish";
    let mut command = CommandBuilder::new(shell);
    if is_fish {
        command.arg("--interactive");
    }
    command.env("TERM", "dumb");
    if let Some(dir) = working_dir {
        command.cwd(dir);
    }

    let child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
    let output = Arc::new(Mutex::new(Output::default()));
    let reader_output = output.clone();
    std::thread::Builder::new()
        .name("jcode-terminal-host-reader".into())
        .spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let mut output = reader_output.lock().expect("terminal output poisoned");
                        output.bytes.extend_from_slice(&buffer[..count]);
                        if output.bytes.len() > MAX_REPLAY_BYTES {
                            let remove = output.bytes.len() - MAX_REPLAY_BYTES;
                            output.bytes.drain(..remove);
                            output.available_from += remove as u64;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    // Linux PTY masters can briefly return EIO between spawn
                    // and the child opening the slave side.
                    Err(error) if error.raw_os_error() == Some(5) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            reader_output
                .lock()
                .expect("terminal output poisoned")
                .closed = true;
        })?;

    Ok(TerminalResource {
        writer,
        _child: child,
        output,
        attachments: 1,
    })
}

unsafe extern "C-unwind" fn terminal_write(
    context: *mut c_void,
    id: u64,
    data: *const u8,
    len: usize,
) -> i32 {
    let Some(state) = state(context) else {
        return HOST_FAILED;
    };
    if len != 0 && data.is_null() {
        return HOST_FAILED;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let terminals = state.terminals.lock().expect("terminal lock poisoned");
    let Some(resource) = terminals.resources.get(&id) else {
        return HOST_FAILED;
    };
    let result = resource
        .writer
        .lock()
        .expect("terminal writer poisoned")
        .write_all(bytes);
    if result.is_ok() { HOST_OK } else { HOST_FAILED }
}

unsafe extern "C-unwind" fn terminal_read(
    context: *mut c_void,
    id: u64,
    cursor: u64,
    destination: *mut u8,
    capacity: usize,
) -> TerminalRead {
    let Some(state) = state(context) else {
        return TerminalRead::default();
    };
    if capacity != 0 && destination.is_null() {
        return TerminalRead::default();
    }
    let terminals = state.terminals.lock().expect("terminal lock poisoned");
    let Some(resource) = terminals.resources.get(&id) else {
        return TerminalRead {
            closed: 1,
            ..Default::default()
        };
    };
    let output = resource.output.lock().expect("terminal output poisoned");
    let cursor = cursor.max(output.available_from);
    let relative = usize::try_from(cursor - output.available_from)
        .unwrap_or(usize::MAX)
        .min(output.bytes.len());
    let copied = capacity.min(output.bytes.len() - relative);
    if copied != 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(output.bytes[relative..].as_ptr(), destination, copied)
        };
    }
    TerminalRead {
        copied,
        next_cursor: cursor + copied as u64,
        available_from: output.available_from,
        closed: u8::from(output.closed),
    }
}

unsafe extern "C-unwind" fn terminal_release(context: *mut c_void, id: u64) {
    let Some(state) = state(context) else {
        return;
    };
    let mut terminals = state.terminals.lock().expect("terminal lock poisoned");
    let remove = terminals.resources.get_mut(&id).is_some_and(|resource| {
        resource.attachments = resource.attachments.saturating_sub(1);
        resource.attachments == 0
    });
    if remove {
        terminals.resources.remove(&id);
    }
}

fn default_shell() -> String {
    for candidate in ["/usr/bin/fish", "/bin/fish", "/bin/bash", "/bin/sh"] {
        if std::path::Path::new(candidate).exists() {
            return candidate.into();
        }
    }
    std::env::var("SHELL").unwrap_or_else(|_| "sh".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_desktop_api::HostHandle;
    use std::time::{Duration, Instant};

    #[test]
    fn snapshot_is_copied_into_host_storage() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        let mut source = b"workspace".to_vec();
        assert!(host.store_snapshot(&source, 7));
        source.fill(0);
        assert_eq!(state.take_snapshot(), Some((7, b"workspace".to_vec())));
    }

    #[test]
    fn terminal_resource_survives_generation_handoff() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        let id = host.terminal_create(None, None).expect("create PTY");
        let same = host.terminal_create(Some(id), None).expect("reattach PTY");
        assert_eq!(same, id);
        host.terminal_release(id);
        assert!(host.terminal_write(same, b"printf jcode-pty-preserved\\n"));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut cursor = 0;
        let mut collected = Vec::new();
        while Instant::now() < deadline {
            let mut buffer = [0; 4096];
            let read = host.terminal_read(same, cursor, &mut buffer);
            cursor = read.next_cursor;
            collected.extend_from_slice(&buffer[..read.copied]);
            if String::from_utf8_lossy(&collected).contains("jcode-pty-preserved") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(String::from_utf8_lossy(&collected).contains("jcode-pty-preserved"));
        host.terminal_release(same);
    }
}
