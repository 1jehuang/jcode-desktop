//! Host-owned close action shared by every reloadable tab bar.
use std::{cell::RefCell, rc::Rc};

use gpui::{AnyWindowHandle, App, actions};

use super::reload::ReloadManager;

actions!(jcode_desktop_host, [CloseWindow]);

pub fn install(
    cx: &mut App,
    manager: Rc<RefCell<ReloadManager>>,
    current: Rc<RefCell<Option<AnyWindowHandle>>>,
) {
    cx.on_action(move |_: &CloseWindow, cx| {
        let manager = manager.clone();
        let current = current.clone();
        // Action dispatch holds the window update. Release it before entering
        // the host's snapshot path, just as native close notifications do.
        cx.defer(move |cx| {
            let Some(handle) = *current.borrow() else {
                return;
            };
            let _ = handle.update(cx, |_, window, cx| {
                if let Err(error) = manager.borrow_mut().suspend(window, cx) {
                    eprintln!("failed to suspend desktop workspace: {error:#}");
                    return;
                }
                *current.borrow_mut() = None;
                window.remove_window();
            });
        });
    });
}

/// Shared single-panel hosts close the window whose tab bar asked, and never
/// suspend it. Other windows of the same process are unaffected.
pub fn install_close_only(cx: &mut App) {
    cx.on_action(move |_: &CloseWindow, cx| {
        let Some(handle) = cx.active_window() else {
            return;
        };
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    });
}
