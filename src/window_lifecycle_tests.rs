//! Headless checks of the actual host close callback and restore path.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use jcode_desktop_api::{ACTIVATE_FAILED, ACTIVATE_OK, HostApi, HostHandle, PluginApi};
use std::ffi::c_void;

#[derive(Default)]
struct Failures {
    activate: bool,
    snapshot: bool,
}
impl gpui::Global for Failures {}

struct DraftRoot(String);
impl Render for DraftRoot {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().child(self.0.clone())
    }
}

unsafe extern "C-unwind" fn activate(
    window: *mut c_void,
    app: *mut c_void,
    _: *const HostApi,
    bytes: *const u8,
    len: usize,
    _: u32,
) -> i32 {
    let window = unsafe { &mut *window.cast::<Window>() };
    let cx = unsafe { &mut *app.cast::<App>() };
    if cx.global::<Failures>().activate {
        return ACTIVATE_FAILED;
    }
    let draft = if len == 0 {
        "unsent draft".into()
    } else {
        String::from_utf8(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()).unwrap()
    };
    window.replace_root(cx, |_, _| DraftRoot(draft));
    ACTIVATE_OK
}

unsafe extern "C-unwind" fn snapshot(
    window: *mut c_void,
    app: *mut c_void,
    host: *const HostApi,
) -> i32 {
    let window = unsafe { &mut *window.cast::<Window>() };
    let cx = unsafe { &mut *app.cast::<App>() };
    if cx.global::<Failures>().snapshot {
        return ACTIVATE_FAILED;
    }
    let Some(Some(root)) = window.root::<DraftRoot>() else {
        return ACTIVATE_FAILED;
    };
    let host = unsafe { HostHandle::new(host) }.unwrap();
    if host.store_snapshot(root.read(cx).0.as_bytes(), 1) {
        ACTIVATE_OK
    } else {
        ACTIVATE_FAILED
    }
}

type CurrentWindow = Rc<RefCell<Option<gpui::AnyWindowHandle>>>;
fn setup(cx: &mut TestAppContext) -> (Rc<RefCell<ReloadManager>>, CurrentWindow) {
    cx.update(|cx| {
        cx.set_global(Failures::default());
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| cx.new(|_| HostFallback))
            .unwrap();
        let current = Rc::new(RefCell::new(Some(window.into())));
        let manager = Rc::new(RefCell::new(
            ReloadManager::new(
                PluginApi::new(activate, snapshot),
                None,
                window.into(),
                Rc::new(HostState::default()),
            )
            .unwrap(),
        ));
        window
            .update(cx, |_, window, cx| {
                install_close_handler(window, cx, manager.clone(), current.clone())
            })
            .unwrap();
        manager.borrow_mut().activate_initial(cx).unwrap();
        observe_closed_window(cx, current.clone());
        gpui::AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window
                    .root::<DraftRoot>()
                    .flatten()
                    .unwrap()
                    .update(cx, |root, _| {
                        root.0 = "edited draft survives closing".into();
                    });
            })
            .unwrap();
        (manager, current)
    })
}

#[gpui::test]
fn close_button_survives_root_replacement_and_repeated_restores(cx: &mut TestAppContext) {
    let (manager, current) = setup(cx);
    for _ in 0..3 {
        let handle = current.borrow().unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        // GPUI's helper invokes the real should-close callback but does not
        // destroy the platform window. Complete that step as AppKit would.
        assert!(
            visual.simulate_close(),
            "red-close callback must permit closing"
        );
        assert!(current.borrow().is_none());
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        assert!(cx.windows().is_empty());
        cx.update(|cx| restore_window(&manager, &current, cx))
            .unwrap();
        let restored = current.borrow().unwrap();
        restored
            .update(cx, |_, window, cx| {
                assert_eq!(
                    window.root::<DraftRoot>().flatten().unwrap().read(cx).0,
                    "edited draft survives closing"
                );
            })
            .unwrap();
        cx.update(|cx| restore_window(&manager, &current, cx))
            .unwrap();
        assert_eq!(cx.windows().len(), 1, "show must reuse the existing window");
        assert_eq!(current.borrow().unwrap().window_id(), restored.window_id());
    }
}

#[gpui::test]
fn failed_restore_removes_uncloseable_fallback_and_preserves_draft_for_retry(
    cx: &mut TestAppContext,
) {
    let (manager, current) = setup(cx);
    let handle = current.borrow().unwrap();
    assert!(VisualTestContext::from_window(handle, cx).simulate_close());
    handle
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.update(|cx| cx.global_mut::<Failures>().activate = true);
    assert!(
        cx.update(|cx| restore_window(&manager, &current, cx))
            .is_err()
    );
    assert!(current.borrow().is_none());
    assert!(
        cx.windows().is_empty(),
        "failed reopen must not strand an uncloseable fallback"
    );
    cx.update(|cx| cx.global_mut::<Failures>().activate = false);
    cx.update(|cx| restore_window(&manager, &current, cx))
        .unwrap();
    current
        .borrow()
        .unwrap()
        .update(cx, |_, window, cx| {
            assert_eq!(
                window.root::<DraftRoot>().flatten().unwrap().read(cx).0,
                "edited draft survives closing"
            );
        })
        .unwrap();
}

#[gpui::test]
fn snapshot_failure_keeps_live_draft_and_close_can_be_retried(cx: &mut TestAppContext) {
    let (_, current) = setup(cx);
    let handle = current.borrow().unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    cx.update(|cx| cx.global_mut::<Failures>().snapshot = true);
    assert!(
        !visual.simulate_close(),
        "do not discard unsaved state on snapshot failure"
    );
    assert!(current.borrow().is_some());
    assert_eq!(cx.windows().len(), 1);
    cx.update(|cx| cx.global_mut::<Failures>().snapshot = false);
    assert!(visual.simulate_close());
}

#[gpui::test]
fn closing_an_unrelated_window_preserves_the_tracked_workspace(cx: &mut TestAppContext) {
    let (manager, current) = setup(cx);
    let original = current.borrow().unwrap().window_id();
    let other = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| HostFallback))
            .unwrap()
    });
    other
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    assert_eq!(current.borrow().unwrap().window_id(), original);
    cx.update(|cx| restore_window(&manager, &current, cx))
        .unwrap();
    assert_eq!(cx.windows().len(), 1);
}
