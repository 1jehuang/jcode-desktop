//! Jcode Desktop: a spatial, niri-inspired canvas of Jcode sessions.

mod accounts;
mod ack;
mod build_info;
mod clipboard_image;
mod commands;
mod harness;
mod input;
mod learning;
mod markdown;
mod panel;
mod platform;
mod scrollbar;
mod terminal;
mod theme;
mod transition;
mod workspace;

use gpui::{App, KeyBinding, Window};

use workspace::{
    ClosePanel, CycleWidth, FocusDown, FocusFirst, FocusLast, FocusLeft, FocusPrevious, FocusRight,
    FocusUp, MaximizeWidth, MovePanelDown, MovePanelLeft, MovePanelRight, MovePanelToFirst,
    MovePanelToLast, MovePanelUp, NewHelpSession, NewPanel, NewTerminal, OpenFolder, Quit,
    ToggleHints, ToggleOverview, WidthPreset1, WidthPreset2, WidthPreset3, WidthPreset4, Workspace,
};

/// The workspace keymap. Extracted so tests can dispatch through exactly the
/// bindings the user presses, rather than a second copy that could drift.
pub fn bind_workspace_keys(cx: &mut App) {
    terminal::bind_keys(cx);
    cx.bind_keys([
        // Canonical Jcode TUI workspace bindings. On niri these are normally
        // intercepted by the compositor, so Super aliases remain below.
        KeyBinding::new("alt-h", FocusLeft, None),
        KeyBinding::new("alt-l", FocusRight, None),
        KeyBinding::new("alt-j", FocusDown, None),
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
        KeyBinding::new("super-t", NewTerminal, None),
        KeyBinding::new("super-enter", NewPanel, None),
        KeyBinding::new("super-q", ClosePanel, None),
        // niri: Alt+Tab is focus-window-previous, Mod+Tab is the overview.
        KeyBinding::new("super-tab", FocusPrevious, None),
        KeyBinding::new("ctrl-tab", FocusRight, None),
        KeyBinding::new("ctrl-shift-tab", FocusLeft, None),
        KeyBinding::new("ctrl-pageup", FocusLeft, None),
        KeyBinding::new("ctrl-pagedown", FocusRight, None),
        KeyBinding::new("ctrl-alt-left", FocusLeft, None),
        KeyBinding::new("ctrl-alt-right", FocusRight, None),
        KeyBinding::new("ctrl-alt-up", FocusUp, None),
        KeyBinding::new("ctrl-alt-down", FocusDown, None),
        KeyBinding::new("super-shift-tab", ToggleOverview, None),
        KeyBinding::new("super-o", ToggleOverview, None),
        KeyBinding::new("super-/", ToggleHints, None),
        KeyBinding::new("f1", ToggleHints, None),
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
        KeyBinding::new("cmd-t", NewPanel, None),
        KeyBinding::new("cmd-w", ClosePanel, None),
        KeyBinding::new("cmd-o", ToggleOverview, None),
        KeyBinding::new("cmd-shift-o", OpenFolder, None),
        KeyBinding::new("cmd-/", ToggleHints, None),
        KeyBinding::new("cmd-q", Quit, None),
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
        None
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
        bind_workspace_keys(app);
        app.on_action(|_: &Quit, cx| cx.quit());
        let workspace =
            window.replace_root(app, |window, cx| Workspace::new(window, cx, host, snapshot));
        workspace.update(app, |workspace, cx| {
            workspace.restore_focus(window, cx);
        });
        app.activate(true);
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
