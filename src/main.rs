//! Jcode Desktop: a spatial, niri-inspired canvas of Jcode sessions.

mod harness;
mod input;
mod markdown;
mod panel;
mod theme;
mod workspace;

use gpui::{
    App, Bounds, KeyBinding, WindowBounds, WindowOptions, prelude::*, px, size,
};
use gpui_platform::application;

use workspace::{
    ClosePanel, FocusDown, FocusLeft, FocusRight, FocusUp, MovePanelLeft, MovePanelRight,
    NewPanel, Quit, ToggleOverview, WidthPreset1, WidthPreset2, WidthPreset3, WidthPreset4,
    Workspace,
};

fn main() {
    application().run(|cx: &mut App| {
        // Niri-inspired bindings, but on super instead of alt. The user's niri
        // config owns alt; super is free inside an application window.
        cx.bind_keys([
            KeyBinding::new("super-h", FocusLeft, None),
            KeyBinding::new("super-l", FocusRight, None),
            KeyBinding::new("super-j", FocusDown, None),
            KeyBinding::new("super-k", FocusUp, None),
            KeyBinding::new("super-left", FocusLeft, None),
            KeyBinding::new("super-right", FocusRight, None),
            KeyBinding::new("super-down", FocusDown, None),
            KeyBinding::new("super-up", FocusUp, None),
            KeyBinding::new("super-shift-h", MovePanelLeft, None),
            KeyBinding::new("super-shift-l", MovePanelRight, None),
            KeyBinding::new("super-n", NewPanel, None),
            KeyBinding::new("super-t", NewPanel, None),
            KeyBinding::new("super-q", ClosePanel, None),
            KeyBinding::new("super-tab", ToggleOverview, None),
            KeyBinding::new("super-o", ToggleOverview, None),
            KeyBinding::new("super-1", WidthPreset1, None),
            KeyBinding::new("super-2", WidthPreset2, None),
            KeyBinding::new("super-3", WidthPreset3, None),
            KeyBinding::new("super-4", WidthPreset4, None),
            KeyBinding::new("super-shift-q", Quit, None),
        ]);
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
