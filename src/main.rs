//! Stable native GPUI host for Jcode Desktop.

mod host {
    pub mod instance;
    pub mod reload;
    pub mod resources;
}

use std::{
    cell::{Cell, RefCell},
    env,
    path::PathBuf,
    process::Command,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use gpui::{
    App, Bounds, KeyBinding, Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*,
    px, size,
};
use gpui_platform::application;

use host::{
    instance::{self, Instance},
    reload::ReloadManager,
    resources::HostState,
};

actions!(
    jcode_desktop_host,
    [ReloadUi, RebuildAndReloadUi, RollbackUi]
);

fn rebuild_ui() -> anyhow::Result<()> {
    // Give every explicit rebuild a unique input. Without this, Cargo can treat
    // the command as a no-op and preserve the timestamp from an older cdylib.
    let requested_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| anyhow::anyhow!("system clock is before the Unix epoch: {error}"))?
        .as_millis()
        .to_string();
    let output = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "-p", "jcode-desktop-ui"])
        .env("JCODE_DESKTOP_BUILD_EPOCH", requested_at)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    anyhow::bail!(
        "cargo build failed with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    )
}

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
    // Sidebar-free windows are intentionally independent from the main window.
    // Otherwise a shortcut for `--no-sidebar` only wakes the already-running
    // main instance, which silently ignores the new process's launch flags.
    let instance_name = env::args_os()
        .any(|argument| argument == "--no-sidebar" || argument == "--workspace")
        .then_some("no-sidebar");
    let (commands, _instance_socket) = match instance::acquire_named(instance_name)
        .expect("initialize Jcode Desktop instance socket")
    {
        Instance::Primary { commands, _socket } => (commands, _socket),
        Instance::Secondary => return,
    };
    let plugin_path = hot_reload_path();
    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("ctrl-r", ReloadUi, None),
            KeyBinding::new("ctrl-shift-r", RebuildAndReloadUi, None),
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

        // Niri does not implement window minimization, so intercepting close and
        // calling `minimize_window` made Alt+Q appear to do nothing. Let the
        // compositor close the surface and terminate this host cleanly. A later
        // shortcut starts a fresh host; while open, repeated shortcuts still use
        // the instance socket to focus it immediately.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let show_window = window;
        cx.spawn(async move |cx| {
            let mut commands = commands;
            loop {
                let (receiver, command) = cx
                    .background_executor()
                    .spawn(async move {
                        let command = commands.recv();
                        (commands, command)
                    })
                    .await;
                commands = receiver;
                if command.is_err()
                    || show_window
                        .update(cx, |_, window, cx| {
                            window.activate_window();
                            cx.activate(true);
                        })
                        .is_err()
                {
                    return;
                }
            }
        })
        .detach();

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
                let manager = manager.clone();
                // Action dispatch already holds the window update. Defer the
                // root swap so `AnyWindowHandle::update` can enter it cleanly.
                cx.defer(move |cx| {
                    if let Err(error) = manager.borrow_mut().reload(cx) {
                        eprintln!("UI reload failed: {error:#}");
                    }
                });
            }
        });
        let rebuild_in_progress = Rc::new(Cell::new(false));
        cx.on_action({
            let manager = manager.clone();
            let rebuild_in_progress = rebuild_in_progress.clone();
            move |_: &RebuildAndReloadUi, cx| {
                if rebuild_in_progress.replace(true) {
                    eprintln!("UI rebuild already in progress");
                    return;
                }

                let manager = manager.clone();
                let rebuild_in_progress = rebuild_in_progress.clone();
                cx.spawn(async move |cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async { rebuild_ui() })
                        .await;
                    rebuild_in_progress.set(false);
                    match result {
                        Ok(()) => {
                            if let Err(error) = cx.update(|cx| manager.borrow_mut().reload(cx)) {
                                eprintln!("UI reload failed after rebuild: {error:#}");
                            }
                        }
                        Err(error) => eprintln!("UI rebuild failed: {error:#}"),
                    }
                })
                .detach();
            }
        });
        cx.on_action({
            let manager = manager.clone();
            move |_: &RollbackUi, cx| {
                let manager = manager.clone();
                cx.defer(move |cx| {
                    if let Err(error) = manager.borrow_mut().rollback(cx) {
                        eprintln!("UI rollback failed: {error:#}");
                    }
                });
            }
        });

        manager
            .borrow_mut()
            .activate_initial(cx)
            .expect("activate linked Jcode Desktop UI");
        if let Some(path) = plugin_path.as_ref() {
            eprintln!(
                "Jcode Desktop hot reload enabled: Ctrl+R reloads {}, Ctrl+Shift+R rebuilds and reloads; F6 rolls back",
                path.display()
            );
        }
        cx.activate(true);
    });
}
