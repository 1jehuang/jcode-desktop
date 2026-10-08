//! Jcode Desktop's shared UI foundation.
//!
//! Theme, config, text selection, scrollbars, image caching, render stats and
//! animation timing: the leaves every view uses. `jcode-desktop-ui` re-exports
//! these modules at its crate root, so UI code keeps addressing them as
//! `crate::theme`, `crate::config`, and so on. Test a change here with
//! `justrust test -p jcode-desktop-ui-core`.

// `test-support` stubs out config persistence and test-only paths for
// downstream UI tests. This crate's own tests still exercise the real code,
// so the writers and fixed-mode items are only unused in that one build.
#![cfg_attr(all(feature = "test-support", not(test)), allow(dead_code))]

pub mod animation_clock;
pub mod config;
pub mod image_cache;
pub mod prompt_background;
pub mod pulse_text;
pub mod render_stats;
pub mod scrollbar;
pub mod text_selection;
pub mod theme;
pub mod transition;
