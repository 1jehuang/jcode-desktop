//! Versioned C ABI between the native GPUI host and reloadable desktop UI.
//!
//! This is a development-time ABI. Opaque GPUI pointers still require both
//! sides to use the exact same Rust toolchain and pinned GPUI revision.

use std::ffi::c_void;

pub const ABI_VERSION: u32 = 2;
pub const STATE_SCHEMA_VERSION: u32 = 1;
pub const ENTRY_POINT: &[u8] = b"jcode_desktop_ui_plugin\0";
pub const GPUI_REVISION: [u8; 40] = *b"bc538def4545534201bbfcac4e95ac34ea6501b6";

pub const ACTIVATE_OK: i32 = 0;
pub const ACTIVATE_FAILED: i32 = 1;
pub const ACTIVATE_STATE_INCOMPATIBLE: i32 = 2;
pub const HOST_OK: i32 = 0;
pub const HOST_FAILED: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalRead {
    pub copied: usize,
    pub next_cursor: u64,
    pub available_from: u64,
    pub closed: u8,
}

pub type StoreSnapshotFn = unsafe extern "C-unwind" fn(*mut c_void, *const u8, usize, u32) -> i32;
pub type TerminalCreateFn = unsafe extern "C-unwind" fn(*mut c_void, u64, *const u8, usize) -> u64;
pub type TerminalWriteFn = unsafe extern "C-unwind" fn(*mut c_void, u64, *const u8, usize) -> i32;
pub type TerminalReadFn =
    unsafe extern "C-unwind" fn(*mut c_void, u64, u64, *mut u8, usize) -> TerminalRead;
pub type TerminalReleaseFn = unsafe extern "C-unwind" fn(*mut c_void, u64);

/// Host services whose storage and native resources survive UI generations.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostApi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub context: *mut c_void,
    pub store_snapshot: StoreSnapshotFn,
    pub terminal_create: TerminalCreateFn,
    pub terminal_write: TerminalWriteFn,
    pub terminal_read: TerminalReadFn,
    pub terminal_release: TerminalReleaseFn,
}

impl HostApi {
    pub fn is_compatible(self) -> bool {
        self.abi_version == ABI_VERSION && self.struct_size as usize == size_of::<Self>()
    }
}

/// Copyable UI-side wrapper around the host function table.
#[derive(Clone, Copy)]
pub struct HostHandle(HostApi);

impl HostHandle {
    /// # Safety
    /// `api.context` must remain valid while this handle is used.
    pub unsafe fn new(api: *const HostApi) -> Option<Self> {
        let api = unsafe { api.as_ref() }.copied()?;
        api.is_compatible().then_some(Self(api))
    }

    /// Host handle for UI-only tests that do not create native resources.
    pub fn inert() -> Self {
        Self(HostApi {
            abi_version: ABI_VERSION,
            struct_size: size_of::<HostApi>() as u32,
            context: std::ptr::null_mut(),
            store_snapshot: inert_store_snapshot,
            terminal_create: inert_terminal_create,
            terminal_write: inert_terminal_write,
            terminal_read: inert_terminal_read,
            terminal_release: inert_terminal_release,
        })
    }

    pub fn store_snapshot(self, bytes: &[u8], schema: u32) -> bool {
        unsafe {
            (self.0.store_snapshot)(self.0.context, bytes.as_ptr(), bytes.len(), schema) == HOST_OK
        }
    }

    pub fn terminal_create(
        self,
        requested_id: Option<u64>,
        working_dir: Option<&str>,
    ) -> Option<u64> {
        let working_dir = working_dir.unwrap_or_default().as_bytes();
        let id = unsafe {
            (self.0.terminal_create)(
                self.0.context,
                requested_id.unwrap_or_default(),
                working_dir.as_ptr(),
                working_dir.len(),
            )
        };
        (id != 0).then_some(id)
    }

    pub fn terminal_write(self, id: u64, bytes: &[u8]) -> bool {
        unsafe {
            (self.0.terminal_write)(self.0.context, id, bytes.as_ptr(), bytes.len()) == HOST_OK
        }
    }

    pub fn terminal_read(self, id: u64, cursor: u64, output: &mut [u8]) -> TerminalRead {
        unsafe {
            (self.0.terminal_read)(
                self.0.context,
                id,
                cursor,
                output.as_mut_ptr(),
                output.len(),
            )
        }
    }

    pub fn terminal_release(self, id: u64) {
        unsafe { (self.0.terminal_release)(self.0.context, id) }
    }
}

unsafe extern "C-unwind" fn inert_store_snapshot(
    _: *mut c_void,
    _: *const u8,
    _: usize,
    _: u32,
) -> i32 {
    HOST_FAILED
}

unsafe extern "C-unwind" fn inert_terminal_create(
    _: *mut c_void,
    _: u64,
    _: *const u8,
    _: usize,
) -> u64 {
    0
}

unsafe extern "C-unwind" fn inert_terminal_write(
    _: *mut c_void,
    _: u64,
    _: *const u8,
    _: usize,
) -> i32 {
    HOST_FAILED
}

unsafe extern "C-unwind" fn inert_terminal_read(
    _: *mut c_void,
    _: u64,
    cursor: u64,
    _: *mut u8,
    _: usize,
) -> TerminalRead {
    TerminalRead {
        next_cursor: cursor,
        closed: 1,
        ..Default::default()
    }
}

unsafe extern "C-unwind" fn inert_terminal_release(_: *mut c_void, _: u64) {}

/// Snapshot callback. The plugin serializes its root and asks the host to copy it.
pub type SnapshotFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void, *const HostApi) -> i32;

/// Activation callback. The snapshot pointer is host-owned and valid only for
/// the duration of the call. A zero length means a fresh workspace.
pub type ActivateFn = unsafe extern "C-unwind" fn(
    *mut c_void,
    *mut c_void,
    *const HostApi,
    *const u8,
    usize,
    u32,
) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginApi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub gpui_revision: [u8; 40],
    pub state_schema: u32,
    pub minimum_state_schema: u32,
    pub activate: ActivateFn,
    pub snapshot: SnapshotFn,
}

impl PluginApi {
    pub const fn new(activate: ActivateFn, snapshot: SnapshotFn) -> Self {
        Self {
            abi_version: ABI_VERSION,
            struct_size: size_of::<Self>() as u32,
            gpui_revision: GPUI_REVISION,
            state_schema: STATE_SCHEMA_VERSION,
            minimum_state_schema: STATE_SCHEMA_VERSION,
            activate,
            snapshot,
        }
    }

    pub fn compatibility_error(self) -> Option<&'static str> {
        if self.abi_version != ABI_VERSION {
            Some("plugin ABI version differs from host")
        } else if self.struct_size as usize != size_of::<Self>() {
            Some("plugin ABI table size differs from host")
        } else if self.gpui_revision != GPUI_REVISION {
            Some("plugin GPUI revision differs from host")
        } else if self.minimum_state_schema > self.state_schema {
            Some("plugin state schema range is invalid")
        } else {
            None
        }
    }

    pub fn accepts_state(self, schema: u32) -> bool {
        schema == 0 || (self.minimum_state_schema..=self.state_schema).contains(&schema)
    }
}

pub type EntryPoint = unsafe extern "C-unwind" fn() -> PluginApi;

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C-unwind" fn activate(
        _: *mut c_void,
        _: *mut c_void,
        _: *const HostApi,
        _: *const u8,
        _: usize,
        _: u32,
    ) -> i32 {
        ACTIVATE_OK
    }

    unsafe extern "C-unwind" fn snapshot(_: *mut c_void, _: *mut c_void, _: *const HostApi) -> i32 {
        ACTIVATE_OK
    }

    #[test]
    fn current_plugin_api_is_compatible() {
        let api = PluginApi::new(activate, snapshot);
        assert_eq!(api.compatibility_error(), None);
        assert!(api.accepts_state(STATE_SCHEMA_VERSION));
        assert!(api.accepts_state(0));
        assert!(!api.accepts_state(STATE_SCHEMA_VERSION + 1));
    }

    #[test]
    fn rejects_wrong_gpui_revision() {
        let mut api = PluginApi::new(activate, snapshot);
        api.gpui_revision[0] ^= 1;
        assert_eq!(
            api.compatibility_error(),
            Some("plugin GPUI revision differs from host")
        );
    }

    #[test]
    fn fingerprint_matches_the_workspace_gpui_pin() {
        let manifest = include_str!("../../../Cargo.toml");
        let revision = std::str::from_utf8(&GPUI_REVISION).unwrap();
        assert!(
            manifest.contains(&format!("rev = \"{revision}\"")),
            "update GPUI_REVISION whenever the workspace GPUI pin changes"
        );
    }
}
