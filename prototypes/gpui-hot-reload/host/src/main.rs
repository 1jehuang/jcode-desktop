use std::{
    cell::RefCell,
    env,
    ffi::c_void,
    fs,
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context as _, Result, bail};
use gpui::*;
use gpui_platform::application;
use hot_reload_api::{ABI_VERSION, ACTIVATE_OK, ENTRY_POINT, EntryPoint};
use libloading::{Library, Symbol};
use tempfile::TempDir;

actions!(hot_reload_host, [Reload]);

struct HostFallback {
    plugin_path: PathBuf,
}

impl Render for HostFallback {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap_3()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5))
            .child("GPUI hot reload host")
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa1a1aa))
                    .child("Build the UI cdylib, then press F5 to load it."),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x71717a))
                    .child(self.plugin_path.display().to_string()),
            )
    }
}

/// A library is intentionally never unloaded, including during `App` teardown.
/// GPUI entities, render functions, and event callbacks can all contain code
/// pointers into any previously activated generation, and relying on GPUI's
/// internal field drop order would not be sound.
struct LoadedPlugin {
    _library: ManuallyDrop<Library>,
    _staged_path: PathBuf,
}

struct ReloadManager {
    loaded: Vec<LoadedPlugin>,
    staging: TempDir,
    source: PathBuf,
    window: AnyWindowHandle,
    generation: u64,
}

impl ReloadManager {
    fn new(source: PathBuf, window: AnyWindowHandle) -> Result<Self> {
        Ok(Self {
            loaded: Vec::new(),
            staging: tempfile::Builder::new()
                .prefix("gpui-hot-reload-")
                .tempdir()
                .context("create plugin staging directory")?,
            source,
            window,
            generation: 0,
        })
    }

    fn reload(&mut self, cx: &mut App) -> Result<()> {
        let staged_path = self.stage_next_generation()?;

        // SAFETY: the staged file is opened only to resolve the versioned ABI
        // entry point below. It is retained before any plugin code is called.
        let library = unsafe { Library::new(&staged_path) }
            .with_context(|| format!("load {}", staged_path.display()))?;

        let api = {
            // SAFETY: ENTRY_POINT has a fixed C ABI and returns only a repr(C)
            // table. Compatibility is checked before its callback is used.
            let entry: Symbol<EntryPoint> =
                unsafe { library.get(ENTRY_POINT) }.context("resolve gpui_hot_reload_plugin")?;
            unsafe { entry() }
        };

        if !api.is_compatible() {
            bail!(
                "plugin ABI mismatch: host {}, plugin {} (size {})",
                ABI_VERSION,
                api.abi_version,
                api.struct_size
            );
        }

        // Retain the candidate before invoking it. Even a failed activation
        // could have registered callbacks or allocated GPUI entities.
        self.loaded.push(LoadedPlugin {
            _library: ManuallyDrop::new(library),
            _staged_path: staged_path,
        });

        let result = self
            .window
            .update(cx, |_, window, cx| {
                // SAFETY: both pointers are valid for this synchronous call.
                // The API/version checks above ensure both sides agree on the
                // callback contract and use the pinned GPUI revision.
                unsafe {
                    (api.activate)(
                        window as *mut Window as *mut c_void,
                        cx as *mut App as *mut c_void,
                    )
                }
            })
            .context("update host window")?;

        if result != ACTIVATE_OK {
            bail!("plugin declined activation; existing root was preserved");
        }

        eprintln!(
            "activated generation {} from {} ({} libraries retained)",
            self.generation,
            self.source.display(),
            self.loaded.len()
        );
        Ok(())
    }

    fn stage_next_generation(&mut self) -> Result<PathBuf> {
        self.generation += 1;
        let extension = self
            .source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or(env::consts::DLL_EXTENSION);
        let destination = self
            .staging
            .path()
            .join(format!("ui-{:04}.{extension}", self.generation));
        fs::copy(&self.source, &destination).with_context(|| {
            format!(
                "copy plugin {} to {}",
                self.source.display(),
                destination.display()
            )
        })?;
        Ok(destination)
    }
}

fn default_plugin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("host crate has a workspace parent")
        .join("target")
        .join("debug")
        .join(format!(
            "{}hot_reload_ui{}",
            env::consts::DLL_PREFIX,
            env::consts::DLL_SUFFIX
        ))
}

fn main() {
    let plugin_path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOT_RELOAD_UI").map(PathBuf::from))
        .unwrap_or_else(default_plugin_path);

    application().run(move |cx: &mut App| {
        cx.bind_keys([KeyBinding::new("f5", Reload, None)]);

        let bounds = Bounds::centered(None, size(px(760.), px(480.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                {
                    let plugin_path = plugin_path.clone();
                    move |_, cx| cx.new(|_| HostFallback { plugin_path })
                },
            )
            .expect("open host window");

        let manager = Rc::new(RefCell::new(
            ReloadManager::new(plugin_path, window.into()).expect("create reload manager"),
        ));
        cx.on_action({
            let manager = manager.clone();
            move |_: &Reload, cx| {
                if let Err(error) = manager.borrow_mut().reload(cx) {
                    eprintln!("reload failed: {error:#}");
                }
            }
        });

        if let Err(error) = manager.borrow_mut().reload(cx) {
            eprintln!("initial plugin load failed: {error:#}");
        }
        cx.activate(true);
    });
}
