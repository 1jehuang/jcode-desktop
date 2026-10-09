//! Jcode Desktop's voice leaves.
//!
//! The global Copilot-key listener, the OS-level voice overlay pill, the
//! logind session check, and the transcription tags. None of these touch
//! `Panel` or `Workspace`. `jcode-desktop-ui` re-exports them at their old
//! paths (`crate::global_voice_*`, `panel::voice::tag`). Test a change here
//! with `justrust test -p jcode-desktop-voice`.

#[cfg(target_os = "linux")]
pub mod global_voice_input;
pub mod global_voice_overlay;
pub mod global_voice_session;
pub mod tag;

use jcode_desktop_ui_core::{render_stats, theme};
