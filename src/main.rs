//! Jcode Desktop: a spatial, niri-inspired canvas of Jcode sessions.

mod accounts;
mod ack;
mod clipboard_image;
mod harness;
mod input;
mod learning;
mod markdown;
mod panel;
mod theme;
mod transition;
mod workspace;

use gpui::{App, Bounds, KeyBinding, WindowBounds, WindowOptions, prelude::*, px, size};
use gpui_platform::application;

use workspace::{
    ClosePanel, CycleWidth, FocusDown, FocusFirst, FocusLast, FocusLeft, FocusPrevious, FocusRight,
    FocusUp, MaximizeWidth, MovePanelDown, MovePanelLeft, MovePanelRight, MovePanelToFirst,
    MovePanelToLast, MovePanelUp, NewHelpSession, NewPanel, Quit, ToggleHints, ToggleOverview,
    WidthPreset1, WidthPreset2, WidthPreset3, WidthPreset4, Workspace,
};

/// The workspace keymap. Extracted so tests can dispatch through exactly the
/// bindings the user presses, rather than a second copy that could drift.
pub fn bind_workspace_keys(cx: &mut App) {
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
        KeyBinding::new("super-shift-h", MovePanelLeft, None),
        KeyBinding::new("super-shift-l", MovePanelRight, None),
        KeyBinding::new("super-shift-k", MovePanelUp, None),
        KeyBinding::new("super-shift-j", MovePanelDown, None),
        KeyBinding::new("super-shift-home", MovePanelToFirst, None),
        KeyBinding::new("super-shift-end", MovePanelToLast, None),
        KeyBinding::new("super-n", NewPanel, None),
        KeyBinding::new("super-t", NewPanel, None),
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
    ]);
}

fn main() {
    application().run(|cx: &mut App| {
        // The user's niri binds live on alt (alt-hjkl navigate, alt-shift-hjkl
        // move, alt-1..4 widths, alt-r presets, alt-f maximize, alt-tab
        // previous, alt-q close). niri grabs alt before the window sees it, so
        // the same layout is mirrored onto super here.
        bind_workspace_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1500.0), px(950.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(window, cx)),
            )
            .expect("failed to open window");

        window
            .update(cx, |workspace, window, cx| {
                workspace.focus_active(window, cx);
                cx.activate(true);
            })
            .ok();
    });
}
