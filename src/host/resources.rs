use std::{
    collections::HashMap,
    ffi::c_void,
    io::{Read, Write},
    sync::{Arc, Mutex},
};

use jcode_desktop_api::{
    ABI_VERSION, HOST_FAILED, HOST_OK, HostApi, TERMINAL_ENGINE_REPLIES, TerminalRead,
};
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
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
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
            terminal_resize,
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

    if requested_id != 0 {
        let mut terminals = state.terminals.lock().expect("terminal lock poisoned");
        if let Some(resource) = terminals.resources.get_mut(&requested_id) {
            resource.attachments += 1;
            return requested_id;
        }
        return 0;
    }

    let Ok(resource) = spawn_terminal(cwd.as_deref()) else {
        return 0;
    };
    let mut terminals = state.terminals.lock().expect("terminal lock poisoned");
    terminals.next_id = terminals.next_id.saturating_add(1).max(1);
    let id = terminals.next_id;
    terminals.resources.insert(id, resource);
    id
}

fn spawn_terminal(working_dir: Option<&str>) -> anyhow::Result<TerminalResource> {
    let shell = default_shell();
    let is_fish = shell.ends_with("/fish") || shell == "fish";
    let mut command = CommandBuilder::new(shell);
    if is_fish {
        command.arg("--interactive");
    }
    command.env("TERM", "xterm-256color");
    command.env("TERM_PROGRAM", "jcode-desktop");
    command.env("COLORTERM", "truecolor");
    if let Some(dir) = working_dir {
        command.cwd(dir);
    }

    spawn_terminal_command(command)
}

fn spawn_terminal_command(command: CommandBuilder) -> anyhow::Result<TerminalResource> {
    let pair = native_pty_system().openpty(PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut child = pair.slave.spawn_command(command)?;
    let killer = child.clone_killer();
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
                    // Linux reports EIO when the final slave closes.
                    Err(error) if error.raw_os_error() == Some(5) => break,
                    Err(_) => break,
                }
            }
            reader_output
                .lock()
                .expect("terminal output poisoned")
                .closed = true;
        })?;
    std::thread::Builder::new()
        .name("jcode-terminal-host-waiter".into())
        .spawn(move || {
            // Reap the child, but only the reader may declare EOF. The PTY can
            // still contain output, or descendants can still hold its slave open.
            let _ = child.wait();
        })?;

    Ok(TerminalResource {
        master: pair.master,
        writer,
        killer,
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
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let writer = {
        let terminals = state.terminals.lock().expect("terminal lock poisoned");
        let Some(resource) = terminals.resources.get(&id) else {
            return HOST_FAILED;
        };
        resource.writer.clone()
    };
    let result = writer
        .lock()
        .expect("terminal writer poisoned")
        .write_all(bytes);
    if result.is_ok() { HOST_OK } else { HOST_FAILED }
}

unsafe extern "C-unwind" fn terminal_resize(
    context: *mut c_void,
    id: u64,
    rows: u16,
    cols: u16,
) -> i32 {
    let Some(state) = state(context) else {
        return HOST_FAILED;
    };
    if rows == 0 || cols == 0 {
        return HOST_FAILED;
    }
    let terminals = state.terminals.lock().expect("terminal lock poisoned");
    let Some(resource) = terminals.resources.get(&id) else {
        return HOST_FAILED;
    };
    match resource.master.resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(()) => HOST_OK,
        Err(_) => HOST_FAILED,
    }
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
    // ID zero is never allocated. This tagged, zero-capacity probe lets a new
    // UI distinguish transport-only hosts from older hosts that answer queries.
    // Ordinary EOF remains a boolean, including for older UI generations.
    if id == 0 && cursor == 0 && capacity == 0 {
        return TerminalRead {
            next_cursor: TERMINAL_ENGINE_REPLIES,
            closed: 1,
            ..Default::default()
        };
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
        // Consumers may stop polling on closed, so deliver the entire retained
        // tail first, even when their buffer is smaller than the pending output.
        closed: u8::from(output.closed && relative + copied == output.bytes.len()),
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
        if let Some(mut resource) = terminals.resources.remove(&id) {
            let _ = resource.killer.kill();
        }
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
    fn terminal_engine_reply_probe_does_not_create_a_resource() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        assert!(host.terminal_engine_replies());
        assert!(state.terminals.lock().unwrap().resources.is_empty());
        let missing = host.terminal_read(999, 0, &mut []);
        assert_eq!(missing.closed, 1);
        assert_eq!(missing.next_cursor, 0);
        assert_eq!(missing.copied, 0);
    }

    #[cfg(unix)]
    fn scripted_terminal(state: &HostState, script: &str) -> u64 {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        let resource = spawn_terminal_command(command).expect("spawn scripted PTY");
        let mut terminals = state.terminals.lock().unwrap();
        terminals.next_id += 1;
        let id = terminals.next_id;
        terminals.resources.insert(id, resource);
        id
    }

    fn collect_until_closed(host: HostHandle, id: u64, capacity: usize) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut cursor = 0;
        let mut bytes = Vec::new();
        let mut buffer = vec![0; capacity];
        loop {
            let read = host.terminal_read(id, cursor, &mut buffer);
            cursor = read.next_cursor;
            bytes.extend_from_slice(&buffer[..read.copied]);
            if read.closed != 0 {
                return bytes;
            }
            assert!(Instant::now() < deadline, "PTY never reached EOF");
            if read.copied == 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn terminal_queries_are_forwarded_without_host_replies() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        // In noncanonical mode dd times out if the host correctly leaves query
        // handling to the UI. An unsolicited host reply appears in the output.
        let id = scripted_terminal(
            &state,
            r"stty -echo -icanon min 0 time 2; printf '\033[?u\033[>0q\033]11;?\033\\\033[c'; dd bs=256 count=1 2>/dev/null; printf done",
        );
        let bytes = collect_until_closed(host, id, 4096);
        host.terminal_release(id);
        assert_eq!(bytes, b"\x1b[?u\x1b[>0q\x1b]11;?\x1b\\\x1b[cdone");
    }

    #[test]
    #[cfg(unix)]
    fn terminal_eof_waits_for_descendants_and_buffered_tail() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        let id = scripted_terminal(
            &state,
            "trap '' HUP; (sleep 0.3; printf trailing-output-after-shell-exit) & exit 0",
        );
        let bytes = collect_until_closed(host, id, 3);
        host.terminal_release(id);
        assert_eq!(bytes, b"trailing-output-after-shell-exit");
    }

    #[test]
    #[cfg(unix)]
    fn terminal_closed_is_reported_only_after_retained_tail_is_delivered() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        let id = scripted_terminal(&state, "printf buffered-tail");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let closed = state.terminals.lock().unwrap().resources[&id]
                .output
                .lock()
                .unwrap()
                .closed;
            if closed {
                break;
            }
            assert!(Instant::now() < deadline, "reader never reached EOF");
            std::thread::sleep(Duration::from_millis(10));
        }
        // A zero-capacity read must not announce EOF with unread bytes pending.
        assert_eq!(host.terminal_read(id, 0, &mut []).closed, 0);
        let bytes = collect_until_closed(host, id, 2);
        assert_eq!(bytes, b"buffered-tail");
        assert_ne!(
            host.terminal_read(id, bytes.len() as u64, &mut []).closed,
            0
        );
        host.terminal_release(id);
    }

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
        assert!(host.terminal_write(same, b"printf '%s%s\\n' jcode-pty- preserved\r"));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut cursor = 0;
        let mut parser = handterm_common::terminal::Terminal::new(COLS, ROWS);
        while Instant::now() < deadline {
            let mut buffer = [0; 4096];
            let read = host.terminal_read(same, cursor, &mut buffer);
            cursor = read.next_cursor;
            parser.process(&buffer[..read.copied]);
            if let Some(responses) = parser.drain_responses() {
                assert!(host.terminal_write(same, &responses));
            }
            if parser.grid.get_all_text().contains("jcode-pty-preserved") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(parser.grid.get_all_text().contains("jcode-pty-preserved"));
        host.terminal_release(same);
    }

    #[test]
    fn terminal_resizes_and_reports_shell_exit() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        let id = host.terminal_create(None, None).expect("create PTY");

        assert!(host.terminal_resize(id, 24, 80));
        state
            .terminals
            .lock()
            .unwrap()
            .resources
            .get_mut(&id)
            .unwrap()
            .killer
            .kill()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut cursor = 0;
        let closed = loop {
            let mut buffer = [0; 4096];
            let read = host.terminal_read(id, cursor, &mut buffer);
            cursor = read.next_cursor;
            if read.closed != 0 {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(closed, "PTY did not report the exited shell as closed");
        host.terminal_release(id);
    }

    #[test]
    fn zero_length_null_write_is_safe() {
        let state = HostState::default();
        let api = state.api();
        let id = unsafe { (api.terminal_create)(api.context, 0, std::ptr::null(), 0) };
        assert_ne!(id, 0);
        assert_eq!(
            unsafe { (api.terminal_write)(api.context, id, std::ptr::null(), 0) },
            HOST_OK
        );
        unsafe { (api.terminal_release)(api.context, id) };
    }

    #[test]
    fn terminal_startup_has_no_visible_error() {
        let state = HostState::default();
        let api = state.api();
        let host = unsafe { HostHandle::new(&api) }.unwrap();
        let id = host.terminal_create(None, None).expect("create PTY");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut cursor = 0;
        let mut parser = handterm_common::terminal::Terminal::new(COLS, ROWS);
        while Instant::now() < deadline {
            let mut buffer = [0; 4096];
            let read = host.terminal_read(id, cursor, &mut buffer);
            cursor = read.next_cursor;
            parser.process(&buffer[..read.copied]);
            if let Some(responses) = parser.drain_responses() {
                assert!(host.terminal_write(id, &responses));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let contents = parser.grid.get_all_text();
        eprintln!("headless terminal startup:\n{contents}");
        assert!(
            !contents.trim().is_empty(),
            "shell startup never reached a visible prompt"
        );
        assert!(
            !contents.to_ascii_lowercase().contains("error"),
            "shell startup rendered an error: {contents}"
        );
        assert!(host.terminal_write(
            id,
            b"printf '%s%s %s|%s\\n' jcode-terminal- ready \"$TERM_PROGRAM\" \"$COLORTERM\"\r",
        ));
        let command_deadline = Instant::now() + Duration::from_secs(2);
        let mut command_ran = false;
        while Instant::now() < command_deadline {
            let mut buffer = [0; 4096];
            let read = host.terminal_read(id, cursor, &mut buffer);
            cursor = read.next_cursor;
            parser.process(&buffer[..read.copied]);
            if let Some(responses) = parser.drain_responses() {
                assert!(host.terminal_write(id, &responses));
            }
            if parser
                .grid
                .get_all_text()
                .contains("jcode-terminal-ready jcode-desktop|truecolor")
            {
                command_ran = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            command_ran,
            "shell did not execute terminal input with Desktop's terminal identity: {}",
            parser.grid.get_all_text(),
        );
        host.terminal_release(id);
    }
}
