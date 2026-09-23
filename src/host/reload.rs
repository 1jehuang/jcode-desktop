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
    /// Hot-reload staging is development-only. Avoid creating and later
    /// removing a temporary directory on every normal application launch.
    staging: Option<TempDir>,
    source: Option<PathBuf>,
    window: Option<AnyWindowHandle>,
    /// Further windows of a shared single-panel host. They reload together
    /// with `window`, but are closed rather than suspended.
    extra_windows: Vec<AnyWindowHandle>,
    suspended: Option<(u32, Vec<u8>)>,
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
            staging: source
                .as_deref()
                .map(create_staging_dir)
                .transpose()?,
            source,
            window: Some(window),
            extra_windows: Vec::new(),
            suspended: None,
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

    /// Snapshot the live workspace before its platform surface is closed.
    /// Host-owned PTYs and streams remain alive, while the serialized UI state
    /// is retained in memory for a fast restore into the next surface.
    pub fn suspend(&mut self, window: &mut Window, cx: &mut App) -> Result<()> {
        self.suspended = Some(self.snapshot_window(window, cx)?);
        self.window = None;
        Ok(())
    }

    /// Attach a replacement platform surface to the already-running host.
    /// Keep the suspended state and previous attachment until activation succeeds,
    /// so the caller can discard a failed replacement and safely retry.
    pub fn resume(&mut self, window: AnyWindowHandle, cx: &mut App) -> Result<()> {
        let previous_window = self.window.replace(window);
        if let Err(error) = self.activate_generation(self.active, self.suspended.as_ref(), cx) {
            self.window = previous_window;
            return Err(error);
        }
        self.suspended = None;
        Ok(())
    }

    /// Activate the current generation in another window of this host. The
    /// window starts fresh and from then on reloads with every other window.
    pub fn attach_window(&mut self, window: AnyWindowHandle, cx: &mut App) -> Result<()> {
        if self.window.is_none() {
            self.window = Some(window);
            if let Err(error) = self.activate_generation(self.active, None, cx) {
                self.window = None;
                return Err(error);
            }
            return Ok(());
        }
        self.activate_generation_in(window, self.active, None, cx)?;
        self.extra_windows.push(window);
        Ok(())
    }

    /// Stop tracking a window that closed. Another window takes over as the
    /// primary so reloads keep reaching every remaining window.
    pub fn forget_window(&mut self, window: gpui::WindowId) {
        self.extra_windows
            .retain(|handle| handle.window_id() != window);
        if self.window.is_some_and(|handle| handle.window_id() == window) {
            self.window = (!self.extra_windows.is_empty()).then(|| self.extra_windows.remove(0));
        }
    }

    /// Every attached window, primary first.
    pub fn windows(&self) -> Vec<AnyWindowHandle> {
        self.window
            .iter()
            .chain(self.extra_windows.iter())
            .copied()
            .collect()
    }

    pub fn reload(&mut self, cx: &mut App) -> Result<()> {
        let source = self
            .source
            .clone()
            .context("hot reload is disabled; launch with --hot-reload")?;
        let snapshots = self.snapshot_all(cx)?;
        let (library, staged_path, api) = self.load_candidate(&source)?;
        validate_api(api)?;
        for (_, snapshot) in &snapshots {
            if !api.accepts_state(snapshot.0) {
                bail!(
                    "UI state schema {} is outside candidate range {}..={}",
                    snapshot.0,
                    api.minimum_state_schema,
                    api.state_schema
                );
            }
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

        self.activate_all_or_restore(index, &snapshots, cx)?;
        self.generations[index].activated = true;
        self.active = index;
        self.activation_history.push(index);
        eprintln!(
            "activated UI generation {} from {} in {} window(s) ({} libraries retained)",
            self.next_generation,
            source.display(),
            snapshots.len(),
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
        let snapshots = self.snapshot_all(cx)?;
        let api = self.generations[target].api;
        for (_, snapshot) in &snapshots {
            if !api.accepts_state(snapshot.0) {
                bail!(
                    "previous UI does not accept state schema {} (supports {}..={})",
                    snapshot.0,
                    api.minimum_state_schema,
                    api.state_schema
                );
            }
        }
        self.activate_all_or_restore(target, &snapshots, cx)?;
        self.active = target;
        self.activation_history.pop();
        eprintln!("rolled back UI to {}", self.generations[target].label);
        Ok(())
    }

    /// Snapshot every attached window before any of them changes. One
    /// failure cancels the whole reload, so no window loses its state.
    fn snapshot_all(&self, cx: &mut App) -> Result<Vec<(AnyWindowHandle, (u32, Vec<u8>))>> {
        let windows = self.windows();
        anyhow::ensure!(!windows.is_empty(), "desktop window is not attached");
        windows
            .into_iter()
            .map(|handle| {
                let snapshot = handle
                    .update(cx, |_, window, cx| self.snapshot_window(window, cx))
                    .context("update host window while snapshotting")??;
                Ok((handle, snapshot))
            })
            .collect()
    }

    fn snapshot_window(&self, window: &mut Window, cx: &mut App) -> Result<(u32, Vec<u8>)> {
        self.host.clear_snapshot();
        let api = self.generations[self.active].api;
        let host_api = self.host.api();
        let result = unsafe {
            (api.snapshot)(
                window as *mut Window as *mut _,
                cx as *mut App as *mut _,
                &host_api,
            )
        };
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
        let window = self.window.context("desktop window is not attached")?;
        self.activate_generation_in(window, index, snapshot, cx)
    }

    fn activate_generation_in(
        &self,
        window: AnyWindowHandle,
        index: usize,
        snapshot: Option<&(u32, Vec<u8>)>,
        cx: &mut App,
    ) -> Result<()> {
        let api = self.generations[index].api;
        let host_api = self.host.api();
        let (schema, bytes) = snapshot
            .map(|(schema, bytes)| (*schema, bytes.as_slice()))
            .unwrap_or((0, &[]));
        let result = window
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
    #[cfg(test)]
    fn activate_or_restore(
        &self,
        target: usize,
        snapshot: &(u32, Vec<u8>),
        cx: &mut App,
    ) -> Result<()> {
        let window = self.window.context("desktop window is not attached")?;
        self.activate_all_or_restore(target, &[(window, snapshot.clone())], cx)
    }

    /// Every window moves to `target`, or every window stays on the active
    /// generation. Windows already switched when one fails are restored from
    /// their own snapshots, so one shared host never runs mixed generations.
    fn activate_all_or_restore(
        &self,
        target: usize,
        snapshots: &[(AnyWindowHandle, (u32, Vec<u8>))],
        cx: &mut App,
    ) -> Result<()> {
        let mut failure = None;
        for (index, (window, snapshot)) in snapshots.iter().enumerate() {
            if let Err(error) = self.activate_generation_in(*window, target, Some(snapshot), cx) {
                failure = Some((index, error));
                break;
            }
        }
        let Some((failed, activation_error)) = failure else {
            return Ok(());
        };

        let active = self.active;
        let mut restore_errors = Vec::new();
        for (window, snapshot) in &snapshots[..=failed] {
            if let Err(error) = self.activate_generation_in(*window, active, Some(snapshot), cx) {
                restore_errors.push(format!("{error:#}"));
            }
        }
        if restore_errors.is_empty() {
            bail!(
                "UI activation failed: {activation_error:#}; restored {}",
                self.generations[active].label
            )
        }
        bail!(
            "UI activation failed: {activation_error:#}; restoring {} also failed: {}",
            self.generations[active].label,
            restore_errors.join("; ")
        )
    }

    fn load_candidate(&mut self, source: &Path) -> Result<(Library, PathBuf, PluginApi)> {
        self.next_generation += 1;
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or(env::consts::DLL_EXTENSION);
        let staging = self
            .staging
            .as_ref()
            .context("hot-reload staging directory is unavailable")?;
        let staged_path = staging.path().join(format!(
            "jcode-desktop-ui-{:04}.{extension}",
            self.next_generation
        ));
        let staged_bytes = fs::copy(source, &staged_path).with_context(|| {
            format!(
                "copy UI plugin {} to {}",
                source.display(),
                staged_path.display()
            )
        })?;
        eprintln!(
            "staged UI generation {} ({:.1} MiB) at {}",
            self.next_generation,
            staged_bytes as f64 / (1024.0 * 1024.0),
            staged_path.display()
        );
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

const STAGING_DIR_NAME: &str = "ui-staging";
const STAGING_PREFIX: &str = "jcode-desktop-ui-";

/// Stage retained UI copies beside the build output instead of the system
/// temporary directory. `/tmp` is commonly RAM-backed tmpfs, where every
/// retained 100-500 MiB generation silently consumed physical memory, and
/// directories from crashed or killed hosts were never reclaimed. Staging on
/// the build filesystem also lets `fs::copy` use reflinks where supported.
fn create_staging_dir(source: &Path) -> Result<TempDir> {
    let root = source
        .parent()
        .map(|parent| parent.join(STAGING_DIR_NAME))
        .unwrap_or_else(|| env::temp_dir().join(STAGING_DIR_NAME));
    fs::create_dir_all(&root)
        .with_context(|| format!("create UI staging root {}", root.display()))?;
    let removed = sweep_stale_staging(&root);
    if removed > 0 {
        eprintln!(
            "removed {removed} stale UI staging directories from {}",
            root.display()
        );
    }
    tempfile::Builder::new()
        .prefix(&format!("{STAGING_PREFIX}{}-", std::process::id()))
        .tempdir_in(&root)
        .with_context(|| format!("create UI staging directory in {}", root.display()))
}

/// Remove staging directories whose owning host process no longer exists.
/// Normal exit drops the `TempDir`, but crashes, SIGKILL, and `exit()` paths
/// skip destructors, so each launch reclaims what earlier hosts left behind.
fn sweep_stale_staging(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(staging_owner_pid) else {
            continue;
        };
        if pid == std::process::id() || process_alive(pid) {
            continue;
        }
        if fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn staging_owner_pid(name: &str) -> Option<u32> {
    name.strip_prefix(STAGING_PREFIX)?
        .split_once('-')?
        .0
        .parse()
        .ok()
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 performs only the existence and permission check.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    alive
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_: u32) -> bool {
    true
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
        // Keep the dynamically loaded GPUI build profile identical to the
        // host's. These crates exchange Rust-owned state across the ABI.
        .join(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        })
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

    struct ReloadedRoot;

    impl Render for ReloadedRoot {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div().child("reloaded")
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

    unsafe extern "C-unwind" fn reload_activate(
        window: *mut c_void,
        app: *mut c_void,
        _: *const HostApi,
        _: *const u8,
        _: usize,
        _: u32,
    ) -> i32 {
        let window = unsafe { &mut *window.cast::<Window>() };
        let app = unsafe { &mut *app.cast::<App>() };
        window.replace_root(app, |_, _| ReloadedRoot);
        ACTIVATE_OK
    }

    unsafe extern "C-unwind" fn no_snapshot(
        _: *mut c_void,
        _: *mut c_void,
        _: *const HostApi,
    ) -> i32 {
        ACTIVATE_FAILED
    }

    unsafe extern "C-unwind" fn activate_expected_snapshot(
        window: *mut c_void,
        app: *mut c_void,
        host: *const HostApi,
        snapshot: *const u8,
        snapshot_len: usize,
        snapshot_schema: u32,
    ) -> i32 {
        if snapshot_schema != STATE_SCHEMA_VERSION
            || snapshot_len != b"workspace".len()
            || unsafe { std::slice::from_raw_parts(snapshot, snapshot_len) } != b"workspace"
        {
            return ACTIVATE_FAILED;
        }
        unsafe { stable_activate(window, app, host, snapshot, snapshot_len, snapshot_schema) }
    }

    #[gpui::test]
    fn successful_resume_consumes_snapshot_and_attaches_replacement(cx: &mut TestAppContext) {
        let replacement = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| FailedRoot))
                .unwrap()
        });
        let mut manager = ReloadManager::new(
            PluginApi::new(activate_expected_snapshot, no_snapshot),
            None,
            replacement.into(),
            Rc::new(HostState::default()),
        )
        .unwrap();
        manager.window = None;
        manager.suspended = Some((STATE_SCHEMA_VERSION, b"workspace".to_vec()));

        cx.update(|cx| manager.resume(replacement.into(), cx))
            .unwrap();

        assert!(manager.suspended.is_none());
        assert_eq!(manager.window.unwrap().window_id(), replacement.window_id());
        manager
            .window
            .unwrap()
            .update(cx, |_, window, _| {
                assert!(matches!(window.root::<StableRoot>(), Some(Some(_))));
            })
            .expect("replacement must contain the resumed root");
    }

    #[gpui::test]
    fn failed_resume_preserves_snapshot_and_previous_attachment_for_retry(cx: &mut TestAppContext) {
        for previously_attached in [false, true] {
            let previous = cx.update(|cx| {
                cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| StableRoot))
                    .unwrap()
            });
            let replacement = cx.update(|cx| {
                cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| FailedRoot))
                    .unwrap()
            });
            let mut manager = ReloadManager::new(
                PluginApi::new(failed_after_replacing_root, no_snapshot),
                None,
                previous.into(),
                Rc::new(HostState::default()),
            )
            .unwrap();
            manager.window = previously_attached.then_some(previous.into());
            let previous_window = manager.window;
            let snapshot = (STATE_SCHEMA_VERSION, b"workspace".to_vec());
            manager.suspended = Some(snapshot.clone());

            let error = cx
                .update(|cx| manager.resume(replacement.into(), cx))
                .unwrap_err();

            assert!(error.to_string().contains("UI activation failed"));
            assert_eq!(manager.suspended.as_ref(), Some(&snapshot));
            assert_eq!(
                manager.window.map(|window| window.window_id()),
                previous_window.map(|window| window.window_id())
            );
            previous
                .update(cx, |_: &mut StableRoot, _, _| {})
                .expect("failed resume must not replace the prior window's root");

            manager.generations[manager.active].api =
                PluginApi::new(activate_expected_snapshot, no_snapshot);
            cx.update(|cx| manager.resume(replacement.into(), cx))
                .unwrap();
            assert!(manager.suspended.is_none());
            assert_eq!(manager.window.unwrap().window_id(), replacement.window_id());
        }
    }

    #[test]
    fn staging_is_beside_source_and_sweeps_only_dead_owners() {
        let build = tempfile::tempdir().unwrap();
        let source = build.path().join("libjcode_desktop_ui.so");
        let root = build.path().join(STAGING_DIR_NAME);
        fs::create_dir_all(&root).unwrap();
        // PIDs above the kernel maximum can never be alive.
        let dead = root.join(format!("{STAGING_PREFIX}4294967294-abc"));
        let foreign = root.join("unrelated");
        fs::create_dir_all(&dead).unwrap();
        fs::create_dir_all(&foreign).unwrap();

        let staging = create_staging_dir(&source).unwrap();

        assert_eq!(staging.path().parent(), Some(root.as_path()));
        assert_eq!(
            staging_owner_pid(staging.path().file_name().unwrap().to_str().unwrap()),
            Some(std::process::id())
        );
        assert!(!dead.exists());
        assert!(foreign.exists());
        // A second host must not remove a live host's directory.
        let second = create_staging_dir(&source).unwrap();
        assert!(staging.path().exists() && second.path().exists());
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

    /// This is the headless native-window regression harness for self-development.
    /// It exercises the same root handoff used after Ctrl+Shift+R and retains the
    /// original window handle. Reimplementing reload by quitting or opening a new
    /// OS window makes the original handle update or the window-count assertion fail.
    #[gpui::test]
    fn successful_reload_reuses_the_original_native_window(cx: &mut TestAppContext) {
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
            api: PluginApi::new(reload_activate, no_snapshot),
            _library: None,
            _staged_path: None,
            activated: false,
            label: "reloaded test UI".into(),
        });

        cx.update(|cx| {
            manager.activate_or_restore(1, &(STATE_SCHEMA_VERSION, b"workspace".to_vec()), cx)
        })
        .unwrap();

        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        manager
            .window
            .expect("the reloaded root must remain attached")
            .update(cx, |_, _, _| {})
            .expect("the reloaded root must occupy the original native window");
    }

    /// A shared single-panel host reloads every window together and never
    /// leaves them on different generations when one window rejects the new UI.
    #[gpui::test]
    fn shared_host_reloads_all_windows_or_none(cx: &mut TestAppContext) {
        let open = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| StableRoot))
                    .unwrap()
            })
        };
        let (first, second, third) = (open(cx), open(cx), open(cx));
        let mut manager = ReloadManager::new(
            PluginApi::new(stable_activate, no_snapshot),
            None,
            first.into(),
            Rc::new(HostState::default()),
        )
        .unwrap();
        cx.update(|cx| manager.activate_initial(cx)).unwrap();
        cx.update(|cx| manager.attach_window(second.into(), cx))
            .unwrap();
        cx.update(|cx| manager.attach_window(third.into(), cx))
            .unwrap();
        assert_eq!(manager.windows().len(), 3);
        let snapshots: Vec<_> = manager
            .windows()
            .into_iter()
            .map(|window| (window, (STATE_SCHEMA_VERSION, b"workspace".to_vec())))
            .collect();

        manager.generations.push(Generation {
            api: PluginApi::new(reload_activate, no_snapshot),
            _library: None,
            _staged_path: None,
            activated: false,
            label: "reloaded".into(),
        });
        cx.update(|cx| manager.activate_all_or_restore(1, &snapshots, cx))
            .unwrap();
        for window in [first, second, third] {
            AnyWindowHandle::from(window)
                .update(cx, |_, window, _| {
                    assert!(matches!(window.root::<ReloadedRoot>(), Some(Some(_))))
                })
                .unwrap();
        }
        manager.active = 1;

        // The third window rejects generation 2 after the first two switched.
        // Every window must return to generation 1 from its own snapshot.
        unsafe extern "C-unwind" fn fail_in_third(
            window: *mut c_void,
            app: *mut c_void,
            host: *const HostApi,
            snapshot: *const u8,
            len: usize,
            schema: u32,
        ) -> i32 {
            thread_local!(static CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });
            let call = CALLS.with(|calls| {
                calls.set(calls.get() + 1);
                calls.get()
            });
            if call == 3 {
                return unsafe { failed_after_replacing_root(window, app, host, snapshot, len, schema) };
            }
            unsafe { stable_activate(window, app, host, snapshot, len, schema) }
        }
        manager.generations.push(Generation {
            api: PluginApi::new(fail_in_third, no_snapshot),
            _library: None,
            _staged_path: None,
            activated: false,
            label: "broken".into(),
        });
        let error = cx
            .update(|cx| manager.activate_all_or_restore(2, &snapshots, cx))
            .unwrap_err();
        assert!(error.to_string().contains("restored reloaded"), "{error:#}");
        for window in [first, second, third] {
            AnyWindowHandle::from(window)
                .update(cx, |_, window, _| {
                    assert!(matches!(window.root::<ReloadedRoot>(), Some(Some(_))))
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn closing_the_primary_window_promotes_another_for_future_reloads(cx: &mut TestAppContext) {
        let open = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| StableRoot))
                    .unwrap()
            })
        };
        let (first, second) = (open(cx), open(cx));
        let mut manager = ReloadManager::new(
            PluginApi::new(stable_activate, no_snapshot),
            None,
            first.into(),
            Rc::new(HostState::default()),
        )
        .unwrap();
        cx.update(|cx| manager.activate_initial(cx)).unwrap();
        cx.update(|cx| manager.attach_window(second.into(), cx))
            .unwrap();
        manager.forget_window(first.window_id());
        assert_eq!(
            manager.windows().iter().map(|w| w.window_id()).collect::<Vec<_>>(),
            vec![second.window_id()]
        );
        manager.forget_window(second.window_id());
        assert!(manager.windows().is_empty());
        // A later window becomes the primary again.
        let third = open(cx);
        cx.update(|cx| manager.attach_window(third.into(), cx))
            .unwrap();
        assert_eq!(manager.windows().len(), 1);
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
