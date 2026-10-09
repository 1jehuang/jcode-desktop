//! Jcode Desktop's login leaves.
//!
//! The masked secret input, the provider catalog ordering, and the
//! `jcode auth doctor` connection health. None of these touch `Panel` or
//! `Workspace`. `jcode-desktop-ui` re-exports them at their old paths
//! (`crate::login_input`, `panel::login::{catalog, connection}`). Test a change
//! here with `justrust test -p jcode-desktop-accounts-ui`.

pub mod catalog;
pub mod connection;
pub mod login_input;

use jcode_desktop_harness::{harness, platform};
use jcode_desktop_ui_core::{render_stats, theme};
