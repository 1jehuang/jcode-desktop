//! Jcode Desktop: a spatial, niri-inspired canvas of Jcode sessions.

mod accounts;
mod ack;
mod build_info;
mod changelog;
mod clipboard_image;
mod commands;
mod config;
mod diff;
mod diff_block;
mod diff_model;
mod diff_review_content;
mod diff_view;
mod fps_counter;
mod harness;
mod html_preview;
mod image_cache;
mod input;
mod learning;
mod live_profile;
pub mod login_input;
mod markdown;
mod native_mermaid;
#[cfg(test)]
mod native_mermaid_integration_tests;
mod panel;
mod pdf_render;
mod pdf_viewer;
mod performance;
mod platform;
mod preview_control;
pub mod preview_state;
mod remote_targets;
mod scrollbar;
mod sound_events;
mod sounds;
mod terminal;
mod text_selection;
mod theme;
pub mod todoist;
mod transition;
mod update_notes;
mod updates;
mod workspace;

use gpui::{App, KeyBinding, Window};

pub const APP_ID: &str = "jcode-desktop";

/// The linked Desktop build, available without initializing GPUI or a window.
pub fn build_version() -> String {
    format!("{} ({})", build_info::version(), build_info::revision())
}

use workspace::{
    ClosePanel, CycleTheme, CycleWidth, FocusDown, FocusFirst, FocusLast, FocusLeft, FocusPrevious,
    FocusRight, FocusUp, ForkPanel, MaximizeWidth, MovePanelDown, MovePanelLeft, MovePanelRight,
    MovePanelToFirst, MovePanelToLast, MovePanelUp, NewHelpSession, NewPanel,
    NewPanelInPinnedDirectory, NewTerminal, OpenFolder, OpenGmail, OpenTodoist, Quit, ToggleHints,
    ToggleOverview, ToggleShowcase, ToggleSidebar, WidthPreset1, WidthPreset2, WidthPreset3,
    WidthPreset4, Workspace,
};

/// The workspace keymap. Extracted so tests can dispatch through exactly the
/// bindings the user presses, rather than a second copy that could drift.
pub fn bind_workspace_keys(cx: &mut App) {
    terminal::bind_keys(cx);
    panel::voice::bind_keys(cx);
    cx.bind_keys([
        // Canonical Jcode TUI workspace bindings. On niri these are normally
        // intercepted by the compositor, so Super aliases remain below.
        // On macOS ScrollWM owns Opt+H/J/K/L for its own column and workspace
        // motions, so these would never reach us; Cmd+H/J/K/L covers us there.
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("alt-h", FocusLeft, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("alt-l", FocusRight, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("alt-j", FocusDown, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("alt-k", FocusUp, None),
        KeyBinding::new("super-h", FocusLeft, None),
        KeyBinding::new("super-l", FocusRight, None),
        KeyBinding::new("super-j", FocusDown, None),
        KeyBinding::new("super-k", FocusUp, None),
        KeyBinding::new("super-left", FocusLeft, None),
        KeyBinding::new("super-right", FocusRight, None),
        KeyBinding::new("super-down", FocusDown, None),
        KeyBinding::new("super-up", FocusUp, None),
        // niri focus-column-first / focus-column-last (Mod+Home/End).
        KeyBinding::new("super-home", FocusFirst, None),
        KeyBinding::new("super-end", FocusLast, None),
        KeyBinding::new("super-u", FocusFirst, None),
        KeyBinding::new("super-p", FocusLast, None),
        KeyBinding::new("super-shift-h", MovePanelLeft, None),
        KeyBinding::new("super-shift-l", MovePanelRight, None),
        KeyBinding::new("super-shift-k", MovePanelUp, None),
        KeyBinding::new("super-shift-j", MovePanelDown, None),
        KeyBinding::new("super-shift-home", MovePanelToFirst, None),
        KeyBinding::new("super-shift-end", MovePanelToLast, None),
        KeyBinding::new("super-n", NewPanel, None),
        KeyBinding::new("super-space", ForkPanel, None),
        KeyBinding::new("super-t", NewTerminal, None),
        KeyBinding::new("super-shift-g", OpenGmail, None),
        KeyBinding::new("super-shift-d", OpenTodoist, None),
        // Enter and ; use the configured fixed favorite. ' explicitly uses
        // home. Super+T remains the terminal shortcut.
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("super-enter", NewPanelInPinnedDirectory, None),
        // Non-Super alias for global-shortcut helpers that intercept Enter.
        KeyBinding::new("ctrl-alt-enter", NewPanelInPinnedDirectory, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("super-;", NewPanelInPinnedDirectory, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("super-'", NewPanel, None),
        // Cmd/Super+Q closes the focused panel on every platform. Quitting the
        // app lives on Cmd+Shift+Q so the common chord does the common thing.
        KeyBinding::new("super-q", ClosePanel, None),
        // Global shortcut helpers must not forward Ctrl+W: the composer uses
        // it to delete a word. Keep panel dismissal on a distinct alias.
        KeyBinding::new("ctrl-shift-w", ClosePanel, None),
        // niri: Alt+Tab is focus-window-previous, Mod+Tab is the overview.
        KeyBinding::new("super-tab", FocusPrevious, None),
        KeyBinding::new("ctrl-tab", FocusRight, None),
        KeyBinding::new("ctrl-shift-tab", FocusLeft, None),
        KeyBinding::new("ctrl-pageup", FocusLeft, None),
        KeyBinding::new("ctrl-pagedown", FocusRight, None),
        // ScrollWM owns Ctrl+Opt+Left/Right for its own window focus on macOS,
        // so binding them here would advertise a shortcut that never arrives.
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-alt-left", FocusLeft, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-alt-right", FocusRight, None),
        KeyBinding::new("ctrl-alt-up", FocusUp, None),
        KeyBinding::new("ctrl-alt-down", FocusDown, None),
        KeyBinding::new("super-shift-tab", ToggleOverview, None),
        KeyBinding::new("super-o", ToggleOverview, None),
        KeyBinding::new("super-/", ToggleHints, None),
        KeyBinding::new("f1", ToggleHints, None),
        KeyBinding::new("f2", workspace::RenameSession, None),
        KeyBinding::new("alt-9", workspace::ToggleOnboardingSimulator, None),
        KeyBinding::new("super-shift-s", ToggleShowcase, None),
        KeyBinding::new("super-shift-t", CycleTheme, None),
        KeyBinding::new("ctrl-shift-e", ToggleSidebar, None),
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("super-b", ToggleSidebar, None),
        KeyBinding::new("super-shift-/", NewHelpSession, None),
        // niri switch-preset-column-width / maximize-column.
        KeyBinding::new("super-r", CycleWidth, None),
        KeyBinding::new("super-f", MaximizeWidth, None),
        KeyBinding::new("super-1", WidthPreset1, None),
        KeyBinding::new("super-2", WidthPreset2, None),
        KeyBinding::new("super-3", WidthPreset3, None),
        KeyBinding::new("super-4", WidthPreset4, None),
        KeyBinding::new("super-shift-q", Quit, None),
        KeyBinding::new("ctrl-shift-n", NewPanel, None),
        KeyBinding::new("ctrl-t", NewPanel, None),
        KeyBinding::new("ctrl-o", OpenFolder, None),
    ]);

    // GPUI names the native macOS Command modifier `cmd`. Keep these explicit
    // instead of relying on Super translation so the app feels native when it
    // is launched from Finder.
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-left", FocusLeft, None),
        KeyBinding::new("cmd-right", FocusRight, None),
        KeyBinding::new("cmd-down", FocusDown, None),
        KeyBinding::new("cmd-up", FocusUp, None),
        KeyBinding::new("cmd-shift-left", MovePanelLeft, None),
        KeyBinding::new("cmd-shift-right", MovePanelRight, None),
        KeyBinding::new("cmd-shift-up", MovePanelUp, None),
        KeyBinding::new("cmd-shift-down", MovePanelDown, None),
        KeyBinding::new("cmd-n", NewPanel, None),
        KeyBinding::new("cmd-space", ForkPanel, None),
        // Cmd+T is the terminal shortcut, matching the cross-platform
        // `super-t` binding above. Binding it to NewPanel here shadowed that
        // binding, so macOS opened a session panel instead of a terminal.
        KeyBinding::new("cmd-t", NewTerminal, None),
        KeyBinding::new("cmd-shift-g", OpenGmail, None),
        KeyBinding::new("cmd-shift-t", CycleTheme, None),
        KeyBinding::new("cmd-shift-d", OpenTodoist, None),
        KeyBinding::new("cmd-enter", NewPanelInPinnedDirectory, None),
        KeyBinding::new("cmd-;", NewPanelInPinnedDirectory, None),
        KeyBinding::new("cmd-'", NewPanel, None),
        KeyBinding::new("cmd-w", ClosePanel, None),
        KeyBinding::new("cmd-q", ClosePanel, None),
        KeyBinding::new("cmd-o", ToggleOverview, None),
        KeyBinding::new("cmd-shift-o", OpenFolder, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-/", ToggleHints, None),
        KeyBinding::new("cmd-shift-q", Quit, None),
    ]);
}

use jcode_desktop_api::{
    ACTIVATE_FAILED, ACTIVATE_OK, ACTIVATE_STATE_INCOMPATIBLE, HostApi, HostHandle, PluginApi,
    STATE_SCHEMA_VERSION,
};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
};

unsafe extern "C-unwind" fn activate(
    window: *mut c_void,
    app: *mut c_void,
    host: *const HostApi,
    snapshot: *const u8,
    snapshot_len: usize,
    snapshot_schema: u32,
) -> i32 {
    if window.is_null()
        || app.is_null()
        || host.is_null()
        || (snapshot_len != 0 && snapshot.is_null())
    {
        return ACTIVATE_FAILED;
    }
    if snapshot_schema != 0 && snapshot_schema != STATE_SCHEMA_VERSION {
        return ACTIVATE_STATE_INCOMPATIBLE;
    }
    let Some(host) = (unsafe { HostHandle::new(host) }) else {
        return ACTIVATE_FAILED;
    };
    let snapshot = if snapshot_len == 0 {
        workspace::recovery::load()
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(snapshot, snapshot_len) };
        match workspace::WorkspaceSnapshot::decode(bytes) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                eprintln!("refusing invalid workspace snapshot: {error:#}");
                return ACTIVATE_FAILED;
            }
        }
    };

    let activated = catch_unwind(AssertUnwindSafe(|| {
        let window = unsafe { &mut *window.cast::<Window>() };
        let app = unsafe { &mut *app.cast::<App>() };
        // App globals survive hot reload. Reapply the saved sound preference so
        // a persisted mute cannot be overridden by the previous UI generation.
        sounds::set_enabled(config::get().sounds.enabled, app);
        // Retrofit already-running hosts on reload. New hosts set the ID in
        // WindowOptions before mapping. Updating WM_CLASS during initial X11
        // setup can disrupt the first paint, so defer until the next frame.
        window.on_next_frame(|window, _| window.set_app_id(APP_ID));
        bind_workspace_keys(app);
        app.on_action(|_: &Quit, cx| cx.quit());
        let workspace =
            window.replace_root(app, |window, cx| Workspace::new(window, cx, host, snapshot));
        workspace.update(app, |workspace, cx| {
            workspace.restore_focus(window, cx);
            let fixture = harness::screenshot_mode();
            let changelog_enabled =
                !fixture || std::env::var_os("JCODE_DESKTOP_SCREENSHOT_CHANGELOG").is_some();
            if changelog_enabled && changelog::should_open(snapshot_len != 0) {
                workspace.open_changelog(&workspace::OpenChangelog, window, cx);
            }
        });
        workspace::recovery::install(&workspace, window, app);
        // The host activates explicit launches/reopens. A background startup
        // rebuild must not steal OS focus if the user switched applications.
    }));
    if activated.is_ok() {
        ACTIVATE_OK
    } else {
        ACTIVATE_FAILED
    }
}

unsafe extern "C-unwind" fn snapshot(
    window: *mut c_void,
    app: *mut c_void,
    host: *const HostApi,
) -> i32 {
    if window.is_null() || app.is_null() || host.is_null() {
        return ACTIVATE_FAILED;
    }
    let Some(host) = (unsafe { HostHandle::new(host) }) else {
        return ACTIVATE_FAILED;
    };
    let captured = catch_unwind(AssertUnwindSafe(|| {
        let window = unsafe { &mut *window.cast::<Window>() };
        let app = unsafe { &mut *app.cast::<App>() };
        let Some(Some(workspace)) = window.root::<Workspace>() else {
            return false;
        };
        match workspace
            .read(app)
            .snapshot(window, app)
            .and_then(|snapshot| snapshot.encode())
        {
            Ok(bytes) => host.store_snapshot(&bytes, STATE_SCHEMA_VERSION),
            Err(error) => {
                eprintln!("workspace snapshot failed: {error:#}");
                false
            }
        }
    }));
    if matches!(captured, Ok(true)) {
        ACTIVATE_OK
    } else {
        ACTIVATE_FAILED
    }
}

pub const fn plugin_api() -> PluginApi {
    PluginApi::new(activate, snapshot)
}

#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn jcode_desktop_ui_plugin() -> PluginApi {
    plugin_api()
}
