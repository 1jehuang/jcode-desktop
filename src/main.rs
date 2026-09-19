//! Stable native GPUI host for Jcode Desktop.

mod host {
    #[cfg(any(target_os = "macos", test))]
    pub mod global_shortcut;
    pub mod instance;
    pub mod reload;
    pub mod reload_config;
    pub mod resources;
    pub mod window_controls;
}
mod diagnostics;
#[cfg(test)]
mod voice_command_tests;
#[cfg(test)]
mod window_controls_tests;
#[cfg(test)]
mod window_lifecycle_tests;

use std::{
    cell::{Cell, RefCell},
    env,
    path::PathBuf,
    process::Command,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use gpui::{
    App, Bounds, KeyBinding, Point, Render, TitlebarOptions, Window, WindowBounds, WindowOptions,
    actions, div, prelude::*, px, size,
};
use gpui_platform::application;

use host::{
    instance::{self, Command as InstanceCommand, Instance},
    reload::ReloadManager,
    resources::HostState,
};

actions!(
    jcode_desktop_host,
    [ReloadUi, RebuildAndReloadUi, RollbackUi]
);

fn rebuild_ui(force: bool) -> anyhow::Result<()> {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    // Unify dependency features with the host build. Building only the plugin
    // can give shared GPUI event types different Rust TypeIds, so native mouse
    // events silently fail their downcast across the plugin boundary.
    command.args(["build", "-p", "jcode-desktop", "-p", "jcode-desktop-ui"]);
    // GPUI crosses the plugin ABI as concrete Rust types. A release host must
    // therefore load a release plugin, since debug-only fields and assertions
    // can change their in-memory layout and behavior. Loading a debug cdylib
    // into a release host made `App::quitting` read as true and rejected every
    // replacement workspace during activation.
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    // Explicit reloads need a unique input so Cargo does not preserve the
    // timestamp of an older cdylib. Startup deliberately uses normal Cargo
    // freshness and avoids needless compilation while the user begins typing.
    if force {
        let requested_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| anyhow::anyhow!("system clock is before the Unix epoch: {error}"))?
            .as_millis()
            .to_string();
        command.env("JCODE_DESKTOP_BUILD_EPOCH", requested_at);
    }
    let output = command.current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
    if output.status.success() {
        return Ok(());
    }

    anyhow::bail!(
        "cargo build failed with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    )
}

#[derive(Default)]
struct RebuildState {
    in_progress: Cell<bool>,
}

impl RebuildState {
    fn try_start(&self) -> bool {
        !self.in_progress.replace(true)
    }

    fn finish(&self) {
        self.in_progress.set(false);
    }
}

fn rebuild_and_reload(
    manager: Rc<RefCell<ReloadManager>>,
    rebuild_state: Rc<RebuildState>,
    force: bool,
    source: &'static str,
    cx: &mut App,
) {
    if !rebuild_state.try_start() {
        eprintln!("UI rebuild already in progress; ignoring {source} request");
        return;
    }

    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { rebuild_ui(force) })
            .await;
        rebuild_state.finish();
        match result {
            Ok(()) => {
                if let Err(error) = cx.update(|cx| manager.borrow_mut().reload(cx)) {
                    eprintln!("{source} UI reload failed after rebuild: {error:#}");
                }
            }
            Err(error) => eprintln!("{source} UI rebuild failed: {error:#}"),
        }
    })
    .detach();
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

/// A macOS-native titlebar for a window whose content runs edge to edge.
///
/// The system titlebar stays in place, so the window keeps native traffic
/// lights, native dragging, double-click zoom, and Window-menu tiling. Making
/// it transparent lets the workspace background continue behind it, which is
/// the unified look Finder, Safari, and Xcode use. The traffic lights are
/// nudged down so they sit centered against the app's own header row.
fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some("Jcode".into()),
        appears_transparent: true,
        traffic_light_position: Some(Point {
            x: px(16.0),
            y: px(16.0),
        }),
    }
}

/// Bring the desktop window back, whatever the traffic lights did to it.
///
/// If a window still exists (visible or minimized), activate it. If the red
/// close button destroyed it, open a replacement and resume the suspended
/// workspace into it. Shared by the single-instance `Show` command and the
/// macOS Dock reopen event and global shortcut so all paths behave identically.
fn restore_window(
    manager: &Rc<RefCell<ReloadManager>>,
    current_window: &Rc<RefCell<Option<gpui::AnyWindowHandle>>>,
    cx: &mut App,
) -> anyhow::Result<()> {
    if let Some(window) = *current_window.borrow() {
        if window
            .update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            })
            .is_ok()
        {
            return Ok(());
        }
    }

    let bounds = Bounds::centered(None, size(px(1500.0), px(950.0)), cx);
    let replacement = cx.open_window(
        WindowOptions {
            app_id: Some(jcode_desktop_ui::APP_ID.into()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(titlebar_options()),
            ..Default::default()
        },
        |_, cx| cx.new(|_| HostFallback),
    )?;
    replacement.update(cx, {
        let manager = manager.clone();
        let current_window = current_window.clone();
        move |_, window, cx| {
            install_close_handler(window, cx, manager, current_window);
            window.activate_window();
        }
    })?;
    let replacement = gpui::AnyWindowHandle::from(replacement);
    if let Err(error) = manager.borrow_mut().resume(replacement, cx) {
        // A failed activation may leave HostFallback (or a partial UI) behind.
        // It cannot snapshot, so its normal close callback would veto every X.
        // Remove only this failed surface, retaining the suspended workspace
        // for another Dock/global-shortcut restore attempt.
        let _ = replacement.update(cx, |_, window, _| window.remove_window());
        return Err(error);
    }
    *current_window.borrow_mut() = Some(replacement);
    cx.activate(true);
    Ok(())
}

/// The red close button hides the app rather than losing the workspace: the
/// UI state is suspended so a later reopen resumes exactly where the user
/// left off. Refusing to close on a failed suspend keeps the workspace alive
/// instead of silently discarding it.
fn install_close_handler(
    window: &mut Window,
    cx: &mut gpui::Context<HostFallback>,
    manager: Rc<RefCell<ReloadManager>>,
    current_window: Rc<RefCell<Option<gpui::AnyWindowHandle>>>,
) {
    window.on_window_should_close(cx, move |window, cx| {
        if let Err(error) = manager.borrow_mut().suspend(window, cx) {
            eprintln!("failed to suspend desktop workspace: {error:#}");
            return false;
        }
        *current_window.borrow_mut() = None;
        true
    });
}

fn observe_closed_window(cx: &App, current_window: Rc<RefCell<Option<gpui::AnyWindowHandle>>>) {
    cx.on_window_closed(move |_, closed_id| {
        let mut current = current_window.borrow_mut();
        if current.is_some_and(|window| window.window_id() == closed_id) {
            *current = None;
        }
    })
    .detach();
}

fn quit_mode(screenshot: bool, macos_lifecycle_fixture: bool) -> gpui::QuitMode {
    if cfg!(target_os = "macos") || (screenshot && macos_lifecycle_fixture) {
        // macOS keeps the app (and global shortcut) alive after red-close.
        // Offline native tests can exercise that policy on private Linux Xvfb
        // without changing normal Linux last-window-closed behavior.
        gpui::QuitMode::Explicit
    } else {
        gpui::QuitMode::Default
    }
}

fn main() {
    if env::args_os().any(|argument| argument == "--version" || argument == "-V") {
        println!("Jcode Desktop {}", jcode_desktop_ui::build_version());
        return;
    }
    // Global compositor shortcuts forward only. A missing or older host must
    // never turn a keypress into a fresh application or recovered microphone.
    let instance_name = env::args_os()
        .any(|argument| argument == "--no-sidebar" || argument == "--workspace")
        .then_some("no-sidebar");
    if env::args_os().any(|argument| argument == "--toggle-voice") {
        if let Err(error) = instance::notify_named(instance_name, InstanceCommand::ToggleVoice) {
            eprintln!(
                "could not toggle desktop voice: {error}. Start an updated Jcode Desktop host first."
            );
            std::process::exit(1);
        }
        return;
    }
    let diagnostics_path = diagnostics::install().unwrap_or_else(|error| {
        eprintln!("failed to initialize desktop diagnostics: {error}");
        PathBuf::new()
    });
    if !diagnostics_path.as_os_str().is_empty() {
        eprintln!("desktop diagnostics: {}", diagnostics_path.display());
    }
    // Sidebar-free windows are intentionally independent from the main window.
    // Otherwise a shortcut for `--no-sidebar` only wakes the already-running
    // main instance, which silently ignores the new process's launch flags.
    let requested_command = if env::args_os().any(|argument| argument == "--reload-ui") {
        InstanceCommand::Reload
    } else {
        InstanceCommand::Show
    };
    let (commands, instance_socket) =
        match instance::acquire_named(instance_name, requested_command)
            .expect("initialize Jcode Desktop instance socket")
        {
            Instance::Primary { commands, _socket } => (commands, _socket),
            Instance::Secondary => return,
        };
    let plugin_path = host::reload_config::plugin_path();
    let app = application().with_quit_mode(quit_mode(
        env::var("JCODE_DESKTOP_SCREENSHOT").as_deref() == Ok("1"),
        env::var("JCODE_DESKTOP_SCREENSHOT_MACOS_LIFECYCLE").as_deref() == Ok("1"),
    ));
    // Clicking the Dock icon after the red button closed the last window must
    // bring the workspace back, exactly like a second `jcode-desktop` launch
    // does through the instance socket. GPUI only fires this when no window is
    // visible, which also covers a window hidden by the yellow minimize button.
    type ReopenState = Rc<RefCell<Option<gpui::AnyWindowHandle>>>;
    let reopen_state: Rc<RefCell<Option<(Rc<RefCell<ReloadManager>>, ReopenState)>>> =
        Rc::new(RefCell::new(None));
    app.on_reopen({
        let reopen_state = reopen_state.clone();
        move |cx| {
            let state = reopen_state.borrow().clone();
            if let Some((manager, current_window)) = state {
                if let Err(error) = restore_window(&manager, &current_window, cx) {
                    eprintln!("failed to reopen desktop window from the Dock: {error:#}");
                }
            }
        }
    });
    app.run(move |cx: &mut App| {
        cx.bind_keys([
            // Ctrl+R must always activate code built from the current checkout,
            // rather than silently reloading a stale cdylib from an earlier build.
            KeyBinding::new("ctrl-r", RebuildAndReloadUi, None),
            KeyBinding::new("ctrl-shift-r", RebuildAndReloadUi, None),
            KeyBinding::new("f6", RollbackUi, None),
        ]);

        let bounds = Bounds::centered(None, size(px(1500.0), px(950.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    app_id: Some(jcode_desktop_ui::APP_ID.into()),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(titlebar_options()),
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

        let current_window = Rc::new(RefCell::new(Some(gpui::AnyWindowHandle::from(window))));
        host::window_controls::install(cx, manager.clone(), current_window.clone());
        let rebuild_state = Rc::new(RebuildState::default());
        *reopen_state.borrow_mut() = Some((manager.clone(), current_window.clone()));
        // Only the main desktop owns the global shortcut. Auxiliary workspace
        // instances must not steal it or report a spurious registration conflict.
        #[cfg(target_os = "macos")]
        if instance_name.is_none() {
            if let Err(error) = host::global_shortcut::install(cx, {
                let manager = manager.clone();
                let current_window = current_window.clone();
                move |cx| restore_window(&manager, &current_window, cx)
            }) {
                eprintln!("could not register global Control+Command+I shortcut: {error:#}");
            }
        }
        window
            .update(cx, {
                let manager = manager.clone();
                let current_window = current_window.clone();
                move |_, window, cx| {
                    install_close_handler(window, cx, manager, current_window);
                }
            })
            .expect("install persistent host close handler");
        observe_closed_window(cx, current_window.clone());

        cx.spawn({
            let manager = manager.clone();
            let current_window = current_window.clone();
            let rebuild_state = rebuild_state.clone();
            async move |cx| {
                // The guard owns the socket pathname. It must live as long as
                // the command loop, not merely until application setup returns.
                let _instance_socket = instance_socket;
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
                    let Ok(command) = command else { return };

                    if command == InstanceCommand::Reload {
                        cx.update(|cx| {
                            rebuild_and_reload(
                                manager.clone(),
                                rebuild_state.clone(),
                                true,
                                "remote",
                                cx,
                            );
                        });
                        continue;
                    }

                    let result = cx.update(|cx| {
                        restore_window(&manager, &current_window, cx)?;
                        if command == InstanceCommand::ToggleVoice {
                            let window = current_window.borrow().ok_or_else(|| anyhow::anyhow!("desktop window unavailable"))?;
                            dispatch_ui_action(window, "workspace::ToggleVoice", cx)?;
                        }
                        Ok::<_, anyhow::Error>(())
                    });
                    if let Err(error) = result {
                        eprintln!("failed to restore desktop window: {error:#}");
                    }
                }
            }
        })
        .detach();

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
        cx.on_action({
            let manager = manager.clone();
            let rebuild_state = rebuild_state.clone();
            move |_: &RebuildAndReloadUi, cx| {
                rebuild_and_reload(
                    manager.clone(),
                    rebuild_state.clone(),
                    true,
                    "interactive",
                    cx,
                );
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
                "Jcode Desktop hot reload enabled: Ctrl+R rebuilds and reloads the latest UI from {}; Ctrl+Shift+R does the same; F6 rolls back",
                path.display()
            );
            // Make the linked generation interactive immediately. The current
            // checkout is built off the UI thread, then ReloadManager suspends
            // and resumes that live workspace so drafts survive the swap.
            rebuild_and_reload(
                manager.clone(),
                rebuild_state.clone(),
                false,
                "startup",
                cx,
            );
        }
        cx.activate(true);
    });
}

/// Resolve against the current UI generation, not a statically linked Rust
/// action TypeId. No host/plugin ABI or persisted-state changes are needed.
fn dispatch_ui_action(
    window: gpui::AnyWindowHandle,
    name: &str,
    cx: &mut App,
) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        // A restored root may not have mounted its listeners yet. Prepare its
        // dispatch tree now, just as GPUI does before a native key event. Do
        // not wait for a compositor frame: background activation can be denied.
        window.draw(cx).clear(cx);
        let action = cx.build_action(name, None)?;
        if !window.is_action_available(action.as_ref(), cx) {
            // A removed picker may leave no mounted focus target. The UI
            // supplies its workspace as a tab stop for this bounded repair.
            window.focus_next(cx);
        }
        anyhow::ensure!(
            window.is_action_available(action.as_ref(), cx),
            "current UI cannot handle {name}"
        );
        window.dispatch_action(action, cx);
        Ok::<_, anyhow::Error>(())
    })??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::RebuildState;

    #[test]
    fn rebuild_state_allows_only_one_start_until_finished() {
        let state = RebuildState::default();

        assert!(state.try_start());
        assert!(!state.try_start());
        state.finish();
        assert!(state.try_start());
    }

    #[test]
    fn macos_lifecycle_override_requires_an_offline_fixture() {
        use gpui::QuitMode;
        assert!(matches!(super::quit_mode(true, true), QuitMode::Explicit));
        for (screenshot, lifecycle) in [(false, false), (false, true), (true, false)] {
            let mode = super::quit_mode(screenshot, lifecycle);
            if cfg!(target_os = "macos") {
                assert!(matches!(mode, QuitMode::Explicit));
            } else {
                assert!(matches!(mode, QuitMode::Default));
            }
        }
    }
}
