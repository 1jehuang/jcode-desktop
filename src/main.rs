//! Stable native GPUI host for Jcode Desktop.

mod host {
    pub mod reload;
    pub mod resources;
}

use std::{cell::RefCell, env, path::PathBuf, rc::Rc};

use gpui::{
    App, Bounds, KeyBinding, Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*,
    px, size,
};
use gpui_platform::application;

use host::{reload::ReloadManager, resources::HostState};

actions!(jcode_desktop_host, [ReloadUi, RollbackUi]);

struct HostFallback;

impl Render for HostFallback {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgb(0x111318))
            .text_color(gpui::rgb(0xe5e7eb))
            .child("Starting Jcode Desktop…")
    }
}

fn hot_reload_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("JCODE_DESKTOP_UI") {
        return Some(path.into());
    }
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--hot-reload" {
            return arguments
                .next()
                .filter(|next| !next.to_string_lossy().starts_with('-'))
                .map(PathBuf::from)
                .or_else(|| Some(host::reload::default_plugin_path()));
        }
    }
    None
}

fn main() {
    let plugin_path = hot_reload_path();
    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("f5", ReloadUi, None),
            KeyBinding::new("f6", RollbackUi, None),
        ]);

        let bounds = Bounds::centered(None, size(px(1500.0), px(950.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| HostFallback),
            )
            .expect("failed to open native Jcode Desktop window");

        let host = Rc::new(HostState::default());
        let manager = Rc::new(RefCell::new(
            ReloadManager::new(
                jcode_desktop_ui::plugin_api(),
                plugin_path.clone(),
                window.into(),
                host,
            )
            .expect("create UI reload manager"),
        ));

        cx.on_action({
            let manager = manager.clone();
            move |_: &ReloadUi, cx| {
                if let Err(error) = manager.borrow_mut().reload(cx) {
                    eprintln!("UI reload failed: {error:#}");
                }
            }
        });
        cx.on_action({
            let manager = manager.clone();
            move |_: &RollbackUi, cx| {
                if let Err(error) = manager.borrow_mut().rollback(cx) {
                    eprintln!("UI rollback failed: {error:#}");
                }
            }
        });

        manager
            .borrow_mut()
            .activate_initial(cx)
            .expect("activate linked Jcode Desktop UI");
        if let Some(path) = plugin_path.as_ref() {
            eprintln!(
                "Jcode Desktop hot reload enabled: build jcode-desktop-ui, then press F5 to load {}; F6 rolls back",
                path.display()
            );
        }
        cx.activate(true);
    });
}
