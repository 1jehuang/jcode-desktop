//! Jcode Desktop's harness bridge and its transports, free of GPUI.
//!
//! `jcode-desktop-ui` re-exports these modules at its crate root, so UI code
//! keeps addressing them as `crate::harness`, `crate::platform`, and so on.

use std::sync::OnceLock;

pub mod accounts;
pub mod harness;
pub mod managed_cloud;
pub mod managed_cloud_parity;
pub mod platform;
pub mod remote_targets;

static CLIENT_VERSION: OnceLock<&'static str> = OnceLock::new();

/// Record the Desktop display version reported in SDK client names and the
/// managed-cloud user agent. The UI calls this when each hot-reloaded
/// generation activates, since build metadata lives in the UI's build script.
pub fn set_client_version(version: &'static str) {
    let _ = CLIENT_VERSION.set(version);
}

/// The Desktop version advertised to the runtime and cloud services.
pub fn client_version() -> &'static str {
    CLIENT_VERSION
        .get()
        .copied()
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}
