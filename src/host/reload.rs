use std::{
    env, fs,
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context as _, Result, bail};
use gpui::{AnyWindowHandle, App, Window};
use jcode_desktop_api::{
    ACTIVATE_OK, ACTIVATE_STATE_INCOMPATIBLE, ENTRY_POINT, EntryPoint, PluginApi,
};
use libloading::{Library, Symbol};
use tempfile::TempDir;

use super::resources::HostState;

struct Generation {
    api: PluginApi,
    _library: Option<ManuallyDrop<Library>>,
    _staged_path: Option<PathBuf>,
    activated: bool,
    label: String,
}

/// Owns all UI generations. Libraries are intentionally retained until process
/// exit because GPUI callbacks and entities may still contain their code pointers.
pub struct ReloadManager {
    generations: Vec<Generation>,
    active: usize,
    activation_history: Vec<usize>,
    staging: TempDir,
    source: Option<PathBuf>,
    window: AnyWindowHandle,
    host: Rc<HostState>,
    next_generation: u64,
}

impl ReloadManager {
    pub fn new(
        static_api: PluginApi,
        source: Option<PathBuf>,
        window: AnyWindowHandle,
        host: Rc<HostState>,
    ) -> Result<Self> {
        validate_api(static_api)?;
        Ok(Self {
            generations: vec![Generation {
                api: static_api,
                _library: None,
                _staged_path: None,
                activated: false,
                label: "linked release UI".into(),
            }],
            active: 0,
            activation_history: Vec::new(),
            staging: tempfile::Builder::new()
                .prefix("jcode-desktop-ui-")
                .tempdir()
                .context("create UI staging directory")?,
            source,
            window,
            host,
            next_generation: 0,
        })
    }

    pub fn activate_initial(&mut self, cx: &mut App) -> Result<()> {
        self.activate_generation(0, None, cx)?;
        self.generations[0].activated = true;
        self.activation_history.push(0);
        Ok(())
    }

    pub fn reload(&mut self, cx: &mut App) -> Result<()> {
        let source = self
            .source
            .clone()
            .context("hot reload is disabled; launch with --hot-reload")?;
        let snapshot = self.snapshot_active(cx)?;
        let (library, staged_path, api) = self.load_candidate(&source)?;
        validate_api(api)?;
        if !api.accepts_state(snapshot.0) {
            bail!(
                "UI state schema {} is outside candidate range {}..={}",
                snapshot.0,
                api.minimum_state_schema,
                api.state_schema
            );
        }

        // Retain before invoking any candidate code. Even a failed activation
        // could have registered a callback or created an entity.
        let index = self.generations.len();
        self.generations.push(Generation {
            api,
            _library: Some(ManuallyDrop::new(library)),
            _staged_path: Some(staged_path.clone()),
            activated: false,
            label: staged_path.display().to_string(),
        });

        self.activate_or_restore(index, &snapshot, cx)?;
        self.generations[index].activated = true;
        self.active = index;
        self.activation_history.push(index);
        eprintln!(
            "activated UI generation {} from {} ({} libraries retained)",
            self.next_generation,
            source.display(),
            self.generations.len().saturating_sub(1)
        );
        Ok(())
    }

    pub fn rollback(&mut self, cx: &mut App) -> Result<()> {
        let target = self
            .activation_history
            .get(self.activation_history.len().saturating_sub(2))
            .copied()
            .context("no earlier activated UI generation is available")?;
        let snapshot = self.snapshot_active(cx)?;
        let api = self.generations[target].api;
        if !api.accepts_state(snapshot.0) {
            bail!(
                "previous UI does not accept state schema {} (supports {}..={})",
                snapshot.0,
                api.minimum_state_schema,
                api.state_schema
            );
        }
        self.activate_or_restore(target, &snapshot, cx)?;
        self.active = target;
        self.activation_history.pop();
        eprintln!("rolled back UI to {}", self.generations[target].label);
        Ok(())
    }

    fn snapshot_active(&self, cx: &mut App) -> Result<(u32, Vec<u8>)> {
        self.host.clear_snapshot();
        let api = self.generations[self.active].api;
        let host_api = self.host.api();
        let result = self
            .window
            .update(cx, |_, window, cx| unsafe {
                (api.snapshot)(
                    window as *mut Window as *mut _,
                    cx as *mut App as *mut _,
                    &host_api,
                )
            })
            .context("update host window while snapshotting")?;
        if result != ACTIVATE_OK {
            bail!("active UI failed to snapshot; reload was cancelled")
        }
        self.host
            .take_snapshot()
            .context("active UI returned without storing a snapshot")
    }

    fn activate_generation(
        &self,
        index: usize,
        snapshot: Option<&(u32, Vec<u8>)>,
        cx: &mut App,
    ) -> Result<()> {
        let api = self.generations[index].api;
        let host_api = self.host.api();
        let (schema, bytes) = snapshot
            .map(|(schema, bytes)| (*schema, bytes.as_slice()))
            .unwrap_or((0, &[]));
        let result = self
            .window
            .update(cx, |_, window, cx| unsafe {
                (api.activate)(
                    window as *mut Window as *mut _,
                    cx as *mut App as *mut _,
                    &host_api,
                    bytes.as_ptr(),
                    bytes.len(),
                    schema,
                )
            })
            .context("update host window while activating UI")?;
        match result {
            ACTIVATE_OK => Ok(()),
            ACTIVATE_STATE_INCOMPATIBLE => {
                bail!("UI rejected state schema {schema}; existing root was preserved")
            }
            other => bail!("UI activation failed with status {other}; existing root was preserved"),
        }
    }

    /// Activate a replacement as a transaction from the user's perspective.
    ///
    /// Plugins validate and build their root before `Window::replace_root`, so
    /// ordinary failures leave the current root alone. A panic after that call
    /// can still report failure after changing the root, however. Re-activating
    /// the known-good generation from the same snapshot makes that edge case a
    /// real rollback instead of leaving the manager and window on different
    /// generations.
    fn activate_or_restore(
        &self,
        target: usize,
        snapshot: &(u32, Vec<u8>),
        cx: &mut App,
    ) -> Result<()> {
        let Err(activation_error) = self.activate_generation(target, Some(snapshot), cx) else {
            return Ok(());
        };

        let active = self.active;
        match self.activate_generation(active, Some(snapshot), cx) {
            Ok(()) => bail!(
                "UI activation failed: {activation_error:#}; restored {}",
                self.generations[active].label
            ),
            Err(restore_error) => bail!(
                "UI activation failed: {activation_error:#}; restoring {} also failed: {restore_error:#}",
                self.generations[active].label
            ),
        }
    }

    fn load_candidate(&mut self, source: &Path) -> Result<(Library, PathBuf, PluginApi)> {
        self.next_generation += 1;
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or(env::consts::DLL_EXTENSION);
        let staged_path = self.staging.path().join(format!(
            "jcode-desktop-ui-{:04}.{extension}",
            self.next_generation
        ));
        fs::copy(source, &staged_path).with_context(|| {
            format!(
                "copy UI plugin {} to {}",
                source.display(),
                staged_path.display()
            )
        })?;
        let library = unsafe { Library::new(&staged_path) }
            .with_context(|| format!("load {}", staged_path.display()))?;
        let api = {
            let entry: Symbol<EntryPoint> =
                unsafe { library.get(ENTRY_POINT) }.context("resolve jcode_desktop_ui_plugin")?;
            unsafe { entry() }
        };
        Ok((library, staged_path, api))
    }
}

fn validate_api(api: PluginApi) -> Result<()> {
    if let Some(error) = api.compatibility_error() {
        bail!("incompatible UI plugin: {error}")
    }
    Ok(())
}

pub fn default_plugin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join(format!(
            "{}jcode_desktop_ui{}",
            env::consts::DLL_PREFIX,
            env::consts::DLL_SUFFIX
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, TestAppContext, WindowOptions, div, prelude::*};
    use jcode_desktop_api::{ACTIVATE_FAILED, GPUI_REVISION, HostApi, STATE_SCHEMA_VERSION};
    use std::ffi::c_void;

    struct StableRoot;

    impl Render for StableRoot {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div().child("stable")
        }
    }

    struct FailedRoot;

    impl Render for FailedRoot {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div().child("failed")
        }
    }

    unsafe extern "C-unwind" fn stable_activate(
        window: *mut c_void,
        app: *mut c_void,
        _: *const HostApi,
        _: *const u8,
        _: usize,
        _: u32,
    ) -> i32 {
        let window = unsafe { &mut *window.cast::<Window>() };
        let app = unsafe { &mut *app.cast::<App>() };
        window.replace_root(app, |_, _| StableRoot);
        ACTIVATE_OK
    }

    unsafe extern "C-unwind" fn failed_after_replacing_root(
        window: *mut c_void,
        app: *mut c_void,
        _: *const HostApi,
        _: *const u8,
        _: usize,
        _: u32,
    ) -> i32 {
        let window = unsafe { &mut *window.cast::<Window>() };
        let app = unsafe { &mut *app.cast::<App>() };
        window.replace_root(app, |_, _| FailedRoot);
        ACTIVATE_FAILED
    }

    unsafe extern "C-unwind" fn no_snapshot(
        _: *mut c_void,
        _: *mut c_void,
        _: *const HostApi,
    ) -> i32 {
        ACTIVATE_FAILED
    }

    #[test]
    fn version_validation_rejects_wrong_abi_before_loading_code() {
        let mut api = jcode_desktop_ui::plugin_api();
        api.abi_version += 1;
        assert!(
            validate_api(api)
                .unwrap_err()
                .to_string()
                .contains("ABI version")
        );
    }

    #[test]
    fn linked_ui_matches_host_fingerprint_and_schema() {
        let api = jcode_desktop_ui::plugin_api();
        assert_eq!(api.gpui_revision, GPUI_REVISION);
        assert!(api.accepts_state(STATE_SCHEMA_VERSION));
        validate_api(api).unwrap();
    }

    #[gpui::test]
    fn failed_activation_restores_the_previous_root_in_the_same_window(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| StableRoot))
                .unwrap()
        });
        let mut manager = ReloadManager::new(
            PluginApi::new(stable_activate, no_snapshot),
            None,
            window.into(),
            Rc::new(HostState::default()),
        )
        .unwrap();
        cx.update(|cx| manager.activate_initial(cx)).unwrap();
        manager.generations.push(Generation {
            api: PluginApi::new(failed_after_replacing_root, no_snapshot),
            _library: None,
            _staged_path: None,
            activated: false,
            label: "failing test UI".into(),
        });

        let error = cx
            .update(|cx| {
                manager.activate_or_restore(1, &(STATE_SCHEMA_VERSION, b"workspace".to_vec()), cx)
            })
            .unwrap_err();

        assert!(error.to_string().contains("restored linked release UI"));
        window
            .update(cx, |_: &mut StableRoot, _, _| {})
            .expect("the stable root must be restored in the original native window");
    }
}
