//! Stable native GPUI host for Jcode Desktop.

mod host {
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
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
use jcode_desktop_api::LaunchMode;

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
    let started = std::time::Instant::now();
    let output = command.current_dir(env!("CARGO_MANIFEST_DIR")).output()?;
    record_build_timing(force, started.elapsed(), output.status.success());
    if output.status.success() {
        return Ok(());
    }

    anyhow::bail!(
        "cargo build failed with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Append one Ctrl+R build measurement to `target/build-timings.jsonl`, the
/// same log `scripts/build-timings.py` writes, so real reloads are tracked
/// alongside controlled benchmarks. Best effort: timing never fails a reload.
fn record_build_timing(force: bool, elapsed: std::time::Duration, success: bool) {
    use std::io::Write;
    let secs = elapsed.as_secs_f64();
    eprintln!("UI rebuild took {secs:.2}s (success={success})");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let profile = if cfg!(debug_assertions) { "dev" } else { "release" };
    let scenario = if force { "ctrl-r" } else { "startup" };
    let line = format!(
        "{{\"ts\":\"{ts}\",\"unix\":{ts},\"source\":\"host\",\"profile\":\"{profile}\",\"scenario\":\"{scenario}\",\"build_s\":{secs:.2},\"median_s\":{secs:.2},\"success\":{success},\"pid\":{}}}\n",
        std::process::id()
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/build-timings.jsonl");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
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

    let (width, height) = LaunchMode::from_args(env::args_os()).initial_window_size();
    let bounds = Bounds::centered(None, size(px(width as f32), px(height as f32)), cx);
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

fn validate_launch_command(
    mode: LaunchMode,
    args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        mode != LaunchMode::SinglePanel
            || !args.into_iter().any(|arg| {
                voice_cli_command(arg.as_ref()).is_some() || arg.as_ref() == "--reload-ui"
            }),
        "--single-panel cannot target an existing window with --toggle-voice, --voice-press, --voice-release or --reload-ui. Use the window's controls or /update instead."
    );
    Ok(())
}

fn voice_cli_command(arg: &std::ffi::OsStr) -> Option<InstanceCommand> {
    match arg.to_str()? {
        "--toggle-voice" => Some(InstanceCommand::ToggleVoice),
        "--voice-press" => Some(InstanceCommand::VoicePress),
        "--voice-release" => Some(InstanceCommand::VoiceRelease),
        _ => None,
    }
}

fn voice_action(command: InstanceCommand) -> Option<&'static str> {
    match command {
        InstanceCommand::ToggleVoice => Some("workspace::ToggleVoice"),
        InstanceCommand::VoicePress => Some("workspace::BeginVoiceHold"),
        InstanceCommand::VoiceRelease => Some("workspace::EndVoiceHold"),
        _ => None,
    }
}

fn dispatch_instance_command(
    command: InstanceCommand,
    manager: &Rc<RefCell<ReloadManager>>,
    current_window: &Rc<RefCell<Option<gpui::AnyWindowHandle>>>,
    cx: &mut App,
) -> anyhow::Result<()> {
    if let InstanceCommand::OpenWindow(launch) = command {
        return open_shared_window(manager, launch, cx);
    }
    // Releasing a global chord must not pull focus back from another app or
    // reopen a closed surface. Suspending the UI cancels its recording.
    if command != InstanceCommand::VoiceRelease {
        restore_window(manager, current_window, cx)?;
    }
    if let Some(action) = voice_action(command.clone()) {
        if let Some(window) = *current_window.borrow() {
            dispatch_ui_action(window, action, cx)?;
        }
    }
    Ok(())
}

/// Edges from the process-lifetime global voice shortcut (macOS/Windows).
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GlobalVoiceEdge {
    Press,
    Release,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn global_voice_action(edge: GlobalVoiceEdge) -> &'static str {
    match edge {
        GlobalVoiceEdge::Press => "workspace::BeginGlobalVoiceHold",
        GlobalVoiceEdge::Release => "workspace::EndGlobalVoiceHold",
    }
}

/// A global voice hold never activates, restores or raises the window. The UI
/// decides between its focused hold and the unfocused dictation-only owner
/// with an OS-level pill. Only when no live UI can take the unfocused path
/// (red-closed window, or an older UI generation without the action) does it
/// fall back to the legacy restore-and-hold behavior.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn dispatch_global_voice(
    edge: GlobalVoiceEdge,
    manager: &Rc<RefCell<ReloadManager>>,
    current_window: &Rc<RefCell<Option<gpui::AnyWindowHandle>>>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let window = *current_window.borrow();
    if let Some(window) = window {
        match dispatch_ui_action(window, global_voice_action(edge), cx) {
            Ok(()) => return Ok(()),
            Err(error) => eprintln!("global voice: unfocused path unavailable: {error:#}"),
        }
    }
    let legacy = match edge {
        GlobalVoiceEdge::Press => InstanceCommand::VoicePress,
        GlobalVoiceEdge::Release => InstanceCommand::VoiceRelease,
    };
    dispatch_instance_command(legacy, manager, current_window, cx)
}

/// Open one more single-panel window inside the shared host. It gets its own
/// UI root and launch arguments, and closes (not suspends) like any other
/// standalone chat. The host quits only after its last window closes.
fn open_shared_window(
    manager: &Rc<RefCell<ReloadManager>>,
    launch: jcode_desktop_api::WindowLaunch,
    cx: &mut App,
) -> anyhow::Result<()> {
    let (width, height) = LaunchMode::SinglePanel.initial_window_size();
    let bounds = Bounds::centered(None, size(px(width as f32), px(height as f32)), cx);
    let window = cx.open_window(
        WindowOptions {
            app_id: Some(jcode_desktop_ui::APP_ID.into()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(titlebar_options()),
            ..Default::default()
        },
        |_, cx| cx.new(|_| HostFallback),
    )?;
    let handle = gpui::AnyWindowHandle::from(window);
    jcode_desktop_api::WindowLaunches::insert(cx, handle.window_id().as_u64(), launch);
    if let Err(error) = manager.borrow_mut().attach_window(handle, cx) {
        jcode_desktop_api::WindowLaunches::remove(cx, handle.window_id().as_u64());
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        return Err(error);
    }
    let _ = handle.update(cx, |_, window, _| window.activate_window());
    cx.activate(true);
    Ok(())
}

/// The request a single-panel launch forwards to the shared host.
fn forwarded_launch() -> jcode_desktop_api::WindowLaunch {
    jcode_desktop_api::WindowLaunch::current_process()
}

fn launch_quit_mode(mode: LaunchMode, screenshot: bool, lifecycle: bool) -> gpui::QuitMode {
    if mode == LaunchMode::SinglePanel {
        gpui::QuitMode::Default
    } else {
        quit_mode(screenshot, lifecycle)
    }
}

/// Bound glibc's per-thread malloc arenas before any thread starts.
///
/// glibc allows eight arenas per core. The host runs GPUI's 16-thread
/// executor, session workers, and terminal readers, and each thread that ever
/// allocated pinned its own 64 MiB-aligned arena of freed-but-retained pages.
/// On a 16-core laptop that retained slack was larger than the live UI heap.
/// Four arenas keep allocation contention low for a UI process while the
/// housekeeping trim returns what they free. `MALLOC_ARENA_MAX` still wins.
fn configure_system_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if env::var_os("MALLOC_ARENA_MAX").is_none() {
        // SAFETY: mallopt only adjusts allocator parameters and runs before
        // any other thread exists.
        unsafe {
            libc::mallopt(libc::M_ARENA_MAX, 4);
        }
    }
}

fn main() {
    configure_system_allocator();
    if env::args_os().any(|argument| argument == "--version" || argument == "-V") {
        println!("Jcode Desktop {}", jcode_desktop_ui::build_version());
        return;
    }
    // Global compositor shortcuts forward only. A missing or older host must
    // never turn a keypress into a fresh application or recovered microphone.
    let launch_mode = LaunchMode::from_args(env::args_os());
    if let Err(error) = validate_launch_command(launch_mode, env::args_os()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
    // Single-panel windows share one host unless `--new-process` isolates them.
    let shared_single_panel = LaunchMode::shared_single_panel(env::args_os());
    let instance_name =
        LaunchMode::instance_name_for_args(env::args_os().collect::<Vec<_>>(), std::process::id());
    if let Some(command) = env::args_os().find_map(|argument| voice_cli_command(&argument)) {
        if let Err(error) = instance::notify_named(instance_name.as_deref(), command.clone()) {
            eprintln!(
                "could not send desktop voice command {command:?}: {error}. Start an updated Jcode Desktop host first."
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
    // Single-panel launches have a per-process identity. Sidebar-free windows
    // retain their shared identity, independent from the main window.
    // Otherwise a shortcut for `--no-sidebar` only wakes the already-running
    // main instance, which silently ignores the new process's launch flags.
    let requested_command = if env::args_os().any(|argument| argument == "--reload-ui") {
        InstanceCommand::Reload
    } else if shared_single_panel {
        InstanceCommand::OpenWindow(forwarded_launch())
    } else {
        InstanceCommand::Show
    };
    let (commands, instance_socket) =
        match instance::acquire_named(instance_name.as_deref(), requested_command)
            .expect("initialize Jcode Desktop instance socket")
        {
            Instance::Primary { commands, _socket } => (commands, _socket),
            Instance::Secondary => return,
        };
    let instance_socket = Rc::new(instance_socket);
    let plugin_path = host::reload_config::plugin_path();
    let app = application().with_quit_mode(launch_quit_mode(
        launch_mode,
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
        let quit_socket = instance_socket.clone();
        cx.on_app_quit(move |_| {
            quit_socket.cleanup();
            std::future::ready(())
        })
        .detach();
        cx.bind_keys([
            // Ctrl+R must always activate code built from the current checkout,
            // rather than silently reloading a stale cdylib from an earlier build.
            KeyBinding::new("ctrl-r", RebuildAndReloadUi, None),
            KeyBinding::new("ctrl-shift-r", RebuildAndReloadUi, None),
            KeyBinding::new("f6", RollbackUi, None),
        ]);

        let (width, height) = launch_mode.initial_window_size();
        let bounds = Bounds::centered(None, size(px(width as f32), px(height as f32)), cx);
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
        if shared_single_panel {
            // Marks the App as a shared host for the UI. Later windows
            // register their own launch arguments under their window ID.
            jcode_desktop_api::WindowLaunches::install(cx);
        }
        cx.on_window_closed({
            let manager = manager.clone();
            move |cx, closed| {
                manager.borrow_mut().forget_window(closed);
                jcode_desktop_api::WindowLaunches::remove(cx, closed.as_u64());
            }
        })
        .detach();
        if shared_single_panel {
            host::window_controls::install_close_only(cx);
        } else {
            host::window_controls::install(cx, manager.clone(), current_window.clone());
        }
        let rebuild_state = Rc::new(RebuildState::default());
        *reopen_state.borrow_mut() = Some((manager.clone(), current_window.clone()));
        // Only the main desktop owns the global shortcut. Auxiliary workspace
        // instances must not steal it or report a spurious registration conflict.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if instance_name.is_none() {
            if let Err(error) = host::global_shortcut::install(cx, {
                let manager = manager.clone();
                let current_window = current_window.clone();
                move |event, cx| {
                    use host::global_shortcut::ShortcutEvent;
                    match event {
                        ShortcutEvent::Activate => dispatch_instance_command(
                            InstanceCommand::Show,
                            &manager,
                            &current_window,
                            cx,
                        ),
                        ShortcutEvent::VoicePress => dispatch_global_voice(
                            GlobalVoiceEdge::Press,
                            &manager,
                            &current_window,
                            cx,
                        ),
                        ShortcutEvent::VoiceRelease => dispatch_global_voice(
                            GlobalVoiceEdge::Release,
                            &manager,
                            &current_window,
                            cx,
                        ),
                    }
                }
            }) {
                eprintln!("could not register global desktop shortcuts: {error:#}");
            }
        }
        // A shared single-panel host closes windows outright, like separate
        // processes did. Suspending would keep an invisible chat alive.
        if !shared_single_panel {
            window
                .update(cx, {
                    let manager = manager.clone();
                    let current_window = current_window.clone();
                    move |_, window, cx| {
                        install_close_handler(window, cx, manager, current_window);
                    }
                })
                .expect("install persistent host close handler");
        }
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
                        dispatch_instance_command(command, &manager, &current_window, cx)
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
    fn voice_cli_flags_route_to_distinct_current_ui_actions() {
        for (flag, command, action) in [
            (
                "--toggle-voice",
                super::InstanceCommand::ToggleVoice,
                "workspace::ToggleVoice",
            ),
            (
                "--voice-press",
                super::InstanceCommand::VoicePress,
                "workspace::BeginVoiceHold",
            ),
            (
                "--voice-release",
                super::InstanceCommand::VoiceRelease,
                "workspace::EndVoiceHold",
            ),
        ] {
            assert_eq!(super::voice_cli_command(flag.as_ref()), Some(command.clone()));
            assert_eq!(super::voice_action(command), Some(action));
        }
        assert_eq!(super::voice_cli_command("--reload-ui".as_ref()), None);
        assert_eq!(
            super::global_voice_action(super::GlobalVoiceEdge::Press),
            "workspace::BeginGlobalVoiceHold"
        );
        assert_eq!(
            super::global_voice_action(super::GlobalVoiceEdge::Release),
            "workspace::EndGlobalVoiceHold"
        );
        assert_eq!(super::voice_action(super::InstanceCommand::Show), None);
    }

    #[test]
    fn single_panel_rejects_ambiguous_remote_commands_before_startup() {
        use jcode_desktop_api::LaunchMode;
        for command in [
            "--toggle-voice",
            "--voice-press",
            "--voice-release",
            "--reload-ui",
        ] {
            assert!(super::validate_launch_command(LaunchMode::SinglePanel, [command]).is_err());
            assert!(super::validate_launch_command(LaunchMode::Workspace, [command]).is_ok());
            assert!(super::validate_launch_command(LaunchMode::NoSidebar, [command]).is_ok());
        }
        assert!(super::validate_launch_command(LaunchMode::SinglePanel, ["--hot-reload"]).is_ok());
    }

    #[test]
    fn single_panel_quits_after_its_only_window_closes() {
        for (screenshot, lifecycle) in [(false, false), (true, true)] {
            assert!(matches!(
                super::launch_quit_mode(
                    jcode_desktop_api::LaunchMode::SinglePanel,
                    screenshot,
                    lifecycle
                ),
                gpui::QuitMode::Default
            ));
        }
    }

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
