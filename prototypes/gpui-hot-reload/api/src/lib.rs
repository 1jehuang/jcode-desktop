//! The deliberately small C ABI shared by the host and reloadable UI.

use std::ffi::c_void;

pub const ABI_VERSION: u32 = 1;
pub const ENTRY_POINT: &[u8] = b"gpui_hot_reload_plugin\0";

pub const ACTIVATE_OK: i32 = 0;
pub const ACTIVATE_FAILED: i32 = 1;

/// Installs a new root in the host-owned window.
///
/// The pointers are a `gpui::Window` and `gpui::App`, respectively. Keeping
/// them opaque here prevents Rust types from becoming part of the symbol ABI.
pub type ActivateFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> i32;

/// Returned by the plugin entry point. The version and size checks must pass
/// before the host calls `activate`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginApi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub activate: ActivateFn,
}

impl PluginApi {
    pub const fn new(activate: ActivateFn) -> Self {
        Self {
            abi_version: ABI_VERSION,
            struct_size: size_of::<Self>() as u32,
            activate,
        }
    }

    pub fn is_compatible(self) -> bool {
        self.abi_version == ABI_VERSION && self.struct_size as usize == size_of::<Self>()
    }
}

pub type EntryPoint = unsafe extern "C-unwind" fn() -> PluginApi;

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C-unwind" fn activate(_: *mut c_void, _: *mut c_void) -> i32 {
        ACTIVATE_OK
    }

    #[test]
    fn current_api_is_compatible() {
        assert!(PluginApi::new(activate).is_compatible());
    }
}
