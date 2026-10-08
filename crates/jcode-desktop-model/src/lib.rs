//! Jcode Desktop's presentation-independent models, free of GPUI.
//!
//! `jcode-desktop-ui` re-exports these modules at its crate root, so UI code
//! keeps addressing them as `crate::diff_model`, `crate::learning`, and so on.
//! Test a change here with `justrust test -p jcode-desktop-model`.

pub mod diff;
pub mod diff_model;
pub mod learning;
pub mod pdf_render;
pub mod todoist;
